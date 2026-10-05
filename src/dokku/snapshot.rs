use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::sync::{Mutex, Semaphore};

use crate::domain::AppName;
use crate::domain::command::DokkuCommand;
use crate::domain::parse::{
    parse_app_links, parse_apps_list, parse_apps_report, parse_build_info, parse_domains_report,
    parse_ps_report, parse_service_plugins,
};
use crate::domain::types::{AppInfo, BuildInfo, ImageStatus, PsReport, ServiceLink};
use crate::storage::snapshots::{SqliteSnapshots, StoredSnapshot, now_ms};

use super::client::{DokkuClient, DokkuError};
use super::dns::{DnsResolver, TokioResolver, dns_record_status};

const MAX_CONCURRENT_REPORTS: usize = 4;

/// Parsed view of the dokku host, published to a shared SQLite row so every
/// process serves the same data. A background task keeps it warm so request
/// handlers never block on SSH; reads load and parse the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub fetched_at: Instant,
    pub apps: Vec<String>,
    pub ps_reports: HashMap<String, Option<PsReport>>,
    pub apps_reports: HashMap<String, Option<AppInfo>>,
}

impl Snapshot {
    pub fn age(&self) -> Duration {
        self.fetched_at.elapsed()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.apps.iter().any(|app| app == name)
    }

    pub fn ps_report(&self, name: &str) -> Option<&PsReport> {
        self.ps_reports.get(name).and_then(Option::as_ref)
    }

    pub fn app_info(&self, name: &str) -> Option<&AppInfo> {
        self.apps_reports.get(name).and_then(Option::as_ref)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error(transparent)]
    Dokku(#[from] DokkuError),
    #[error("app `{0}` was not found")]
    AppNotFound(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("snapshot serialization error: {0}")]
    Serialize(#[from] serde_json::Error),
}

impl SnapshotError {
    pub fn is_not_found(&self) -> bool {
        matches!(self, SnapshotError::AppNotFound(_))
    }
}

/// Fetches every app plus its `ps:report` and `apps:report`, with bounded concurrency.
/// Per-app failures degrade to `None` (rendered as `Unknown`); only `apps:list` failing
/// aborts the whole build.
pub async fn build_snapshot(client: &dyn DokkuClient) -> Result<Snapshot, DokkuError> {
    let output = client.exec(&DokkuCommand::AppsList).await?;
    let names = parse_apps_list(&output.stdout);
    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_REPORTS));

    let results: Vec<(String, Option<PsReport>, Option<AppInfo>)> =
        join_all(names.into_iter().map(|name| {
            let permits = permits.clone();
            async move {
                let app = match AppName::try_from(name.clone()) {
                    Ok(app) => app,
                    Err(_) => return (name, None, None),
                };
                let _permit = permits.acquire().await.ok();

                let ps = client
                    .exec(&DokkuCommand::PsReport { app: app.clone() })
                    .await
                    .ok()
                    .and_then(|output| parse_ps_report(&output.stdout).ok());
                let info = client
                    .exec(&DokkuCommand::AppsReport { app })
                    .await
                    .ok()
                    .and_then(|output| parse_apps_report(&output.stdout, &name));
                (name, ps, info)
            }
        }))
        .await;

    let mut apps = Vec::with_capacity(results.len());
    let mut ps_reports = HashMap::with_capacity(results.len());
    let mut apps_reports = HashMap::with_capacity(results.len());
    for (name, ps, info) in results {
        apps.push(name.clone());
        ps_reports.insert(name.clone(), ps);
        apps_reports.insert(name, info);
    }
    apps.sort();

    Ok(Snapshot {
        fetched_at: Instant::now(),
        apps,
        ps_reports,
        apps_reports,
    })
}

/// The build/domain/link details shown on an app's overview page. Fetched on
/// demand when an app page is opened, never by the background refresher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDetails {
    pub image_status: Option<ImageStatus>,
    pub last_build: Option<BuildInfo>,
    pub links: Option<Vec<ServiceLink>>,
    pub domains: Vec<String>,
    pub dns_record_exists: Option<bool>,
}

/// Fetches build/domain/link details for a single app (several SSH execs).
async fn fetch_one_details(
    client: &dyn DokkuClient,
    dns: &dyn DnsResolver,
    app: &AppName,
    plugins: Option<&[String]>,
) -> AppDetails {
    let (last_build, image_status) = fetch_build(client, app).await;
    let links = fetch_service_links(client, app, plugins).await;
    let (domains, dns_record_exists) = fetch_domains(client, dns, app).await;
    AppDetails {
        image_status,
        last_build,
        links,
        domains,
        dns_record_exists,
    }
}

async fn fetch_build(
    client: &dyn DokkuClient,
    app: &AppName,
) -> (Option<BuildInfo>, Option<ImageStatus>) {
    let Ok(output) = client
        .exec(&DokkuCommand::BuildsReport { app: app.clone() })
        .await
    else {
        return (None, None);
    };
    match parse_build_info(&output.stdout) {
        Some(build) => {
            let status = build.image_status();
            (Some(build), Some(status))
        }
        None => (None, None),
    }
}

pub(super) async fn fetch_service_links(
    client: &dyn DokkuClient,
    app: &AppName,
    plugins: Option<&[String]>,
) -> Option<Vec<ServiceLink>> {
    let plugins = plugins?;
    let mut links = Vec::new();
    let mut all_ok = true;
    for plugin in plugins {
        match client
            .exec(&DokkuCommand::AppLinks {
                plugin: plugin.clone(),
                app: app.clone(),
            })
            .await
        {
            Ok(output) => {
                for service in parse_app_links(&output.stdout) {
                    links.push(ServiceLink {
                        plugin: plugin.clone(),
                        service,
                    });
                }
            }
            Err(_) => all_ok = false,
        }
    }
    all_ok.then_some(links)
}

async fn fetch_domains(
    client: &dyn DokkuClient,
    dns: &dyn DnsResolver,
    app: &AppName,
) -> (Vec<String>, Option<bool>) {
    let Ok(output) = client
        .exec(&DokkuCommand::DomainsReport { app: app.clone() })
        .await
    else {
        return (Vec::new(), None);
    };
    let vhosts = parse_domains_report(&output.stdout);
    if vhosts.is_empty() {
        return (vhosts, None);
    }
    let mut resolved = Vec::with_capacity(vhosts.len());
    for host in &vhosts {
        resolved.push(dns.resolves(host).await);
    }
    let status = dns_record_status(&vhosts, &resolved);
    (vhosts, status)
}

pub(super) async fn fetch_service_plugins(client: &dyn DokkuClient) -> Option<Vec<String>> {
    client
        .exec(&DokkuCommand::PluginList)
        .await
        .ok()
        .map(|output| parse_service_plugins(&output.stdout))
}

/// JSON payload persisted in the singleton `snapshots` row. `fetched_at`
/// travels as a separate epoch column so `age()` survives the round trip.
#[derive(Debug, Serialize, Deserialize)]
struct SnapshotPayload {
    apps: Vec<String>,
    ps_reports: HashMap<String, Option<PsReport>>,
    apps_reports: HashMap<String, Option<AppInfo>>,
}

fn now_epoch() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

fn stored_from_snapshot(snapshot: &Snapshot) -> Result<StoredSnapshot, serde_json::Error> {
    let payload = SnapshotPayload {
        apps: snapshot.apps.clone(),
        ps_reports: snapshot.ps_reports.clone(),
        apps_reports: snapshot.apps_reports.clone(),
    };
    let fetched_at = now_epoch().saturating_sub(snapshot.fetched_at.elapsed().as_secs() as i64);
    Ok(StoredSnapshot::new(
        serde_json::to_string(&payload)?,
        fetched_at,
    ))
}

fn snapshot_from_stored(stored: &StoredSnapshot) -> Result<Snapshot, serde_json::Error> {
    let payload: SnapshotPayload = serde_json::from_str(&stored.data)?;
    let elapsed = (now_epoch() - stored.fetched_at).max(0) as u64;
    let fetched_at = Instant::now()
        .checked_sub(Duration::from_secs(elapsed))
        .unwrap_or_else(Instant::now);
    Ok(Snapshot {
        fetched_at,
        apps: payload.apps,
        ps_reports: payload.ps_reports,
        apps_reports: payload.apps_reports,
    })
}

fn empty_snapshot() -> Snapshot {
    Snapshot {
        fetched_at: Instant::now(),
        apps: Vec::new(),
        ps_reports: HashMap::new(),
        apps_reports: HashMap::new(),
    }
}

/// Publishes the snapshot to a singleton SQLite row every process reads, so a
/// mutation on one container is visible to all. All SSH work happens before
/// the row write; per-app patches run under a short `BEGIN IMMEDIATE` txn so
/// concurrent patches from different processes never lose an update.
pub struct SnapshotStore {
    client: Arc<dyn DokkuClient>,
    dns: Arc<dyn DnsResolver>,
    snapshots: SqliteSnapshots,
    write_lock: Mutex<()>,
}

impl SnapshotStore {
    pub fn new(client: Arc<dyn DokkuClient>, pool: SqlitePool) -> Self {
        Self::with_resolver(client, Arc::new(TokioResolver::default()), pool)
    }

    pub fn with_resolver(
        client: Arc<dyn DokkuClient>,
        dns: Arc<dyn DnsResolver>,
        pool: SqlitePool,
    ) -> Self {
        Self {
            client,
            dns,
            snapshots: SqliteSnapshots::new(pool),
            write_lock: Mutex::new(()),
        }
    }

    /// Reads the shared row. A corrupt row degrades to `None` (the next
    /// `ensure_loaded` rebuilds it) and a load failure logs and degrades the
    /// same way rather than taking the UI down.
    pub async fn current(&self) -> Option<Arc<Snapshot>> {
        match self.snapshots.load().await {
            Ok(Some(stored)) => match snapshot_from_stored(&stored) {
                Ok(snapshot) => Some(Arc::new(snapshot)),
                Err(err) => {
                    tracing::warn!(error = %err, "snapshot row is corrupt; it will be rebuilt");
                    None
                }
            },
            Ok(None) => None,
            Err(err) => {
                tracing::warn!(error = %err, "snapshot load failed");
                None
            }
        }
    }

    /// Full rebuild, published to the shared row unless the row was modified
    /// after this build started (a per-app patch from another process —
    /// e.g. a restart that just completed — must not be clobbered with the
    /// pre-action state this pass observed). Used by the background refresher
    /// and the manual refresh button; a published pass is immediately
    /// visible to every process.
    pub async fn refresh(&self) -> Result<Arc<Snapshot>, SnapshotError> {
        let _guard = self.write_lock.lock().await;
        let build_started = now_ms();
        let snapshot = build_snapshot(self.client.as_ref()).await?;
        let stored = stored_from_snapshot(&snapshot)?;
        if !self
            .snapshots
            .save_if_unmodified_since(&stored, build_started)
            .await?
        {
            tracing::debug!("snapshot row was modified while refreshing; keeping the newer row");
        }
        Ok(Arc::new(snapshot))
    }

    /// Cold-start path: serve the shared row if any container has published
    /// one (no SSH at all), otherwise build and publish.
    pub async fn ensure_loaded(&self) -> Result<Arc<Snapshot>, SnapshotError> {
        if let Some(snapshot) = self.current().await {
            return Ok(snapshot);
        }
        let _guard = self.write_lock.lock().await;
        if let Some(snapshot) = self.current().await {
            return Ok(snapshot);
        }
        let build_started = now_ms();
        let snapshot = build_snapshot(self.client.as_ref()).await?;
        let stored = stored_from_snapshot(&snapshot)?;
        if !self
            .snapshots
            .save_if_unmodified_since(&stored, build_started)
            .await?
        {
            // Another container published (or healed the row) while we built;
            // serve theirs.
            if let Some(snapshot) = self.current().await {
                return Ok(snapshot);
            }
        }
        Ok(Arc::new(snapshot))
    }

    /// Refreshes a single app (or removes it if dokku no longer lists it), including
    /// the build/domain/link detail pass. Used by the on-demand overview fragment and
    /// as the live fallback when an app is not in the snapshot. The patch applies to
    /// the latest shared row, so sibling apps refreshed by other processes survive.
    pub async fn refresh_app(&self, name: &str) -> Result<(), SnapshotError> {
        self.refresh_app_inner(name, true).await
    }

    /// Cheap variant for mutating actions: syncs only `ps:report`/`apps:report` state
    /// so the dashboard reflects the change. The heavy per-app details are fetched by
    /// the overview fragment when the page is (re)loaded, not here.
    pub async fn refresh_app_reports(&self, name: &str) -> Result<(), SnapshotError> {
        self.refresh_app_inner(name, false).await
    }

    async fn refresh_app_inner(&self, name: &str, details: bool) -> Result<(), SnapshotError> {
        let _guard = self.write_lock.lock().await;

        let output = self.client.exec(&DokkuCommand::AppsList).await?;
        let names = parse_apps_list(&output.stdout);
        let still_listed = names.iter().any(|listed| listed == name);

        let (ps, info) = if still_listed {
            match AppName::try_from(name.to_owned()) {
                Ok(app) => {
                    let ps = self
                        .client
                        .exec(&DokkuCommand::PsReport { app: app.clone() })
                        .await
                        .ok()
                        .and_then(|output| parse_ps_report(&output.stdout).ok());
                    let mut info = self
                        .client
                        .exec(&DokkuCommand::AppsReport { app: app.clone() })
                        .await
                        .ok()
                        .and_then(|output| parse_apps_report(&output.stdout, name));
                    if details {
                        if let Some(info) = info.as_mut() {
                            let plugins = fetch_service_plugins(self.client.as_ref()).await;
                            let details = fetch_one_details(
                                self.client.as_ref(),
                                self.dns.as_ref(),
                                &app,
                                plugins.as_deref(),
                            )
                            .await;
                            info.image_status = details.image_status;
                            info.last_build = details.last_build;
                            info.links = details.links;
                            info.domains = details.domains;
                            info.dns_record_exists = details.dns_record_exists;
                        }
                    }
                    (ps, info)
                }
                Err(_) => (None, None),
            }
        } else {
            (None, None)
        };

        self.snapshots
            .patch(|current| {
                let mut snapshot = match current {
                    Some(stored) => snapshot_from_stored(&stored).unwrap_or_else(|err| {
                        tracing::warn!(error = %err, "snapshot row is corrupt; patching from empty");
                        empty_snapshot()
                    }),
                    None => empty_snapshot(),
                };
                if still_listed {
                    if !snapshot.contains(name) {
                        snapshot.apps.push(name.to_owned());
                        snapshot.apps.sort();
                    }
                    snapshot.ps_reports.insert(name.to_owned(), ps);
                    snapshot.apps_reports.insert(name.to_owned(), info);
                } else {
                    snapshot.apps.retain(|app| app != name);
                    snapshot.ps_reports.remove(name);
                    snapshot.apps_reports.remove(name);
                }
                stored_from_snapshot(&snapshot)
                    .map(|stored| (stored, ()))
                    .map_err(|err| sqlx::Error::AnyDriverError(Box::new(err)))
            })
            .await?;
        Ok(())
    }

    /// Reads the snapshot, verifying `name` is a listed app. On a miss it refreshes just
    /// that app once (covers apps created via the dokku CLI within the staleness window)
    /// before giving up.
    pub async fn resolve_app(&self, name: &str) -> Result<(Arc<Snapshot>, AppName), SnapshotError> {
        let snapshot = self.ensure_loaded().await?;
        if let Some(app) = listed_app(&snapshot, name) {
            return Ok((snapshot, app));
        }

        self.refresh_app(name).await?;
        let snapshot = self.ensure_loaded().await?;
        match listed_app(&snapshot, name) {
            Some(app) => Ok((snapshot, app)),
            None => Err(SnapshotError::AppNotFound(name.to_owned())),
        }
    }
}

fn listed_app(snapshot: &Snapshot, name: &str) -> Option<AppName> {
    if !snapshot.contains(name) {
        return None;
    }
    AppName::try_from(name.to_owned()).ok()
}

/// Refresh forever, sleeping `interval` after each attempt. The cheap ps/apps pass
/// is all that runs in the background; per-app build/domain/link details are fetched
/// on demand by the app pages. Failures keep the last good snapshot; the loop logs
/// and retries rather than taking the UI down.
pub fn spawn_refresher(
    store: Arc<SnapshotStore>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;
            if let Err(err) = store.refresh().await {
                tracing::warn!(error = %err, "snapshot refresh failed; serving last good data");
            }
        }
    })
}

/// Human-friendly age for the "updated X ago" chip. Pure so it is trivially testable.
pub fn format_age(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs == 0 {
        "just now".to_owned()
    } else if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dokku::DokkuOutput;

    const APPS_REPORT_JSON: &str = include_str!("../../tests/fixtures/apps_report.json");
    const BUILDS_REPORT_JSON: &str = include_str!("../../tests/fixtures/builds_report.json");
    const DOMAINS_REPORT_JSON: &str = include_str!("../../tests/fixtures/domains_report.json");
    const PLUGIN_LIST_TXT: &str = include_str!("../../tests/fixtures/plugin_list.txt");
    const APP_LINKS_TXT: &str = include_str!("../../tests/fixtures/app_links.txt");

    fn app(name: &str) -> AppName {
        AppName::try_from(name).expect("valid app name")
    }

    /// A MockClient covering the cheap pass plus every detail command for `alpha`.
    fn details_client() -> crate::dokku::MockClient {
        crate::dokku::MockClient::new()
            .stub(DokkuCommand::AppsList, Ok(apps_list(&["alpha"])))
            .stub(
                DokkuCommand::PsReport { app: app("alpha") },
                Ok(ps_report(true, true, 1)),
            )
            .stub(
                DokkuCommand::AppsReport { app: app("alpha") },
                Ok(DokkuOutput::ok(APPS_REPORT_JSON)),
            )
            .stub(
                DokkuCommand::BuildsReport { app: app("alpha") },
                Ok(DokkuOutput::ok(BUILDS_REPORT_JSON)),
            )
            .stub(
                DokkuCommand::DomainsReport { app: app("alpha") },
                Ok(DokkuOutput::ok(DOMAINS_REPORT_JSON)),
            )
            .stub(
                DokkuCommand::PluginList,
                Ok(DokkuOutput::ok(PLUGIN_LIST_TXT)),
            )
            .stub(
                DokkuCommand::AppLinks {
                    plugin: "postgres".into(),
                    app: app("alpha"),
                },
                Ok(DokkuOutput::ok(APP_LINKS_TXT)),
            )
    }

    fn apps_list(names: &[&str]) -> DokkuOutput {
        let mut output = String::from("=====> My Apps");
        for name in names {
            output.push('\n');
            output.push_str(name);
        }
        DokkuOutput::ok(output)
    }

    fn ps_report(running: bool, deployed: bool, processes: i64) -> DokkuOutput {
        DokkuOutput::ok(format!(
            r#"{{"deployed": "{deployed}", "running": "{running}", "processes": "{processes}"}}"#
        ))
    }

    fn apps_report() -> DokkuOutput {
        DokkuOutput::ok(r#"{"app-created-at": "1791023796", "app-locked": "false"}"#)
    }

    fn count(client: &crate::dokku::MockClient, command: &DokkuCommand) -> usize {
        client
            .calls()
            .iter()
            .filter(|call| *call == command)
            .count()
    }

    async fn store_with(client: Arc<dyn DokkuClient>) -> (SnapshotStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("temp dir");
        let url = format!("sqlite://{}/snapshot.db", dir.path().display());
        let pool = crate::storage::connect(&url).await.expect("connect");
        (
            SnapshotStore::with_resolver(client, Arc::new(crate::dokku::FakeResolver::all()), pool),
            dir,
        )
    }

    /// A second store over the same database file, standing in for another
    /// container sharing the mounted volume.
    async fn second_store(client: Arc<dyn DokkuClient>, dir: &tempfile::TempDir) -> SnapshotStore {
        let url = format!("sqlite://{}/snapshot.db", dir.path().display());
        let pool = crate::storage::connect(&url).await.expect("connect");
        SnapshotStore::with_resolver(client, Arc::new(crate::dokku::FakeResolver::all()), pool)
    }

    #[tokio::test]
    async fn build_snapshot_assembles_apps_and_reports() {
        let client = crate::dokku::MockClient::new()
            .stub(DokkuCommand::AppsList, Ok(apps_list(&["beta", "alpha"])))
            .stub(
                DokkuCommand::PsReport { app: app("alpha") },
                Ok(ps_report(true, true, 2)),
            )
            .stub(
                DokkuCommand::PsReport { app: app("beta") },
                Ok(ps_report(false, true, 1)),
            )
            .stub(
                DokkuCommand::AppsReport { app: app("alpha") },
                Ok(apps_report()),
            )
            .stub(
                DokkuCommand::AppsReport { app: app("beta") },
                Ok(apps_report()),
            );

        let snapshot = build_snapshot(&client).await.expect("snapshot");

        assert_eq!(snapshot.apps, vec!["alpha", "beta"]);
        assert_eq!(
            snapshot.ps_report("alpha").map(|r| r.process_count),
            Some(2)
        );
        assert_eq!(snapshot.ps_report("beta").map(|r| r.running), Some(false));
        assert!(snapshot.app_info("alpha").is_some());
        assert_eq!(
            snapshot.app_info("alpha").map(|i| i.created_at.as_str()),
            Some("2026-10-03 10:36 UTC")
        );
    }

    #[tokio::test]
    async fn build_snapshot_degrades_failing_report_to_none() {
        let client = crate::dokku::MockClient::new()
            .stub(DokkuCommand::AppsList, Ok(apps_list(&["good", "bad"])))
            .stub(
                DokkuCommand::PsReport { app: app("good") },
                Ok(ps_report(true, true, 1)),
            )
            .stub(
                DokkuCommand::PsReport { app: app("bad") },
                Err(DokkuError::Exit {
                    code: 1,
                    stderr: "boom".into(),
                }),
            );

        let snapshot = build_snapshot(&client).await.expect("snapshot");

        assert_eq!(snapshot.apps, vec!["bad", "good"]);
        assert!(snapshot.ps_report("bad").is_none());
        assert_eq!(snapshot.ps_report("good").map(|r| r.process_count), Some(1));
    }

    #[tokio::test]
    async fn build_snapshot_lists_invalid_names_without_reports() {
        let client = crate::dokku::MockClient::new()
            .stub(DokkuCommand::AppsList, Ok(apps_list(&["Bad_Name"])));

        let snapshot = build_snapshot(&client).await.expect("snapshot");

        assert_eq!(snapshot.apps, vec!["Bad_Name"]);
        assert!(snapshot.ps_report("Bad_Name").is_none());
        assert!(
            client
                .calls()
                .iter()
                .all(|call| !matches!(call, DokkuCommand::PsReport { .. })),
            "invalid names never trigger reports"
        );
    }

    #[tokio::test]
    async fn build_snapshot_propagates_apps_list_error() {
        let client =
            crate::dokku::MockClient::with_default(Err(DokkuError::Connect("refused".into())));

        assert!(matches!(
            build_snapshot(&client).await,
            Err(DokkuError::Connect(_))
        ));
    }

    #[tokio::test]
    async fn build_snapshot_empty_list() {
        let client =
            crate::dokku::MockClient::new().stub(DokkuCommand::AppsList, Ok(apps_list(&[])));

        let snapshot = build_snapshot(&client).await.expect("snapshot");

        assert!(snapshot.apps.is_empty());
        assert!(snapshot.ps_reports.is_empty());
    }

    #[tokio::test]
    async fn ensure_loaded_fetches_only_once() {
        let client = Arc::new(
            crate::dokku::MockClient::new().stub(DokkuCommand::AppsList, Ok(apps_list(&["alpha"]))),
        );
        let (store, _dir) = store_with(client.clone()).await;

        let first = store.ensure_loaded().await.expect("first");
        let second = store.ensure_loaded().await.expect("second");
        assert_eq!(first.apps, second.apps, "second load served the shared row");
        assert_eq!(count(&client, &DokkuCommand::AppsList), 1);
    }

    #[tokio::test]
    async fn failed_refresh_keeps_last_good_snapshot() {
        let good = Arc::new(
            crate::dokku::MockClient::new().stub(DokkuCommand::AppsList, Ok(apps_list(&["alpha"]))),
        );
        let client = Arc::new(FailSecondList {
            list_calls: Mutex::new(0),
            good: good.clone(),
        });
        let (store, _dir) = store_with(client).await;

        store.refresh().await.expect("first refresh");
        let before = store.current().await.expect("snapshot");

        assert!(store.refresh().await.is_err(), "second refresh fails");
        let after = store.current().await.expect("still published");
        assert_eq!(before.apps, after.apps, "last good snapshot retained");
    }

    #[tokio::test]
    async fn refresh_app_refetches_that_apps_reports() {
        let client = Arc::new(
            crate::dokku::MockClient::new()
                .stub(DokkuCommand::AppsList, Ok(apps_list(&["alpha"])))
                .stub(
                    DokkuCommand::PsReport { app: app("alpha") },
                    Ok(ps_report(true, true, 1)),
                )
                .stub(
                    DokkuCommand::AppsReport { app: app("alpha") },
                    Ok(apps_report()),
                ),
        );
        let (store, _dir) = store_with(client.clone()).await;
        store.ensure_loaded().await.expect("load");

        store.refresh_app("alpha").await.expect("refresh app");

        assert_eq!(
            count(&client, &DokkuCommand::PsReport { app: app("alpha") }),
            2
        );
        assert_eq!(count(&client, &DokkuCommand::AppsList), 2);
    }

    #[tokio::test]
    async fn refresh_app_adds_and_removes_apps() {
        let list = Arc::new(Mutex::new(apps_list(&["alpha"])));
        let reports = crate::dokku::MockClient::new()
            .stub(
                DokkuCommand::PsReport { app: app("alpha") },
                Ok(ps_report(true, true, 1)),
            )
            .stub(
                DokkuCommand::PsReport { app: app("beta") },
                Ok(ps_report(false, true, 1)),
            )
            .stub(
                DokkuCommand::AppsReport { app: app("alpha") },
                Ok(apps_report()),
            )
            .stub(
                DokkuCommand::AppsReport { app: app("beta") },
                Ok(apps_report()),
            );
        let client = Arc::new(MutableListClient {
            list: list.clone(),
            reports: Arc::new(reports),
            calls: Mutex::new(Vec::new()),
        });
        let (store, _dir) = store_with(client).await;
        store.ensure_loaded().await.expect("load");
        assert!(store.current().await.expect("snap").contains("alpha"));

        *list.lock().await = apps_list(&["alpha", "beta"]);
        store.refresh_app("beta").await.expect("add beta");
        let snapshot = store.current().await.expect("snap");
        assert!(snapshot.contains("beta"));
        assert!(snapshot.ps_report("beta").is_some());

        *list.lock().await = apps_list(&[]);
        store.refresh_app("alpha").await.expect("remove alpha");
        let snapshot = store.current().await.expect("snap");
        assert!(!snapshot.contains("alpha"));
        assert!(snapshot.ps_report("alpha").is_none());
    }

    #[tokio::test]
    async fn resolve_app_returns_listed_app() {
        let client = Arc::new(
            crate::dokku::MockClient::new().stub(DokkuCommand::AppsList, Ok(apps_list(&["alpha"]))),
        );
        let (store, _dir) = store_with(client).await;

        let (snapshot, resolved) = store.resolve_app("alpha").await.expect("resolved");

        assert!(snapshot.contains("alpha"));
        assert_eq!(resolved, app("alpha"));
    }

    #[tokio::test]
    async fn resolve_app_falls_back_to_live_check_then_not_found() {
        let list = Arc::new(Mutex::new(apps_list(&[])));
        let client = Arc::new(MutableListClient {
            list: list.clone(),
            reports: Arc::new(crate::dokku::MockClient::new()),
            calls: Mutex::new(Vec::new()),
        });
        let (store, _dir) = store_with(client.clone()).await;
        store.ensure_loaded().await.expect("load");

        let err = store.resolve_app("ghost").await.expect_err("not found");
        assert!(err.is_not_found());
        let list_calls = client
            .calls
            .lock()
            .await
            .iter()
            .filter(|call| matches!(call, DokkuCommand::AppsList))
            .count();
        assert_eq!(list_calls, 2, "one cold load + one live fallback");
    }

    #[tokio::test]
    async fn spawn_refresher_populates_the_store() {
        let client = Arc::new(
            crate::dokku::MockClient::new().stub(DokkuCommand::AppsList, Ok(apps_list(&["alpha"]))),
        );
        let (store, _dir) = store_with(client).await;
        let store = Arc::new(store);
        let handle = spawn_refresher(store.clone(), Duration::from_millis(10));

        for _ in 0..50 {
            if store.current().await.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(store.current().await.is_some(), "refresher warms the store");
        handle.abort();
    }

    #[tokio::test]
    async fn refresh_app_fetches_details_for_that_app() {
        let client = Arc::new(details_client());
        let (store, _dir) = store_with(client.clone()).await;
        store.ensure_loaded().await.expect("load");

        store.refresh_app("alpha").await.expect("refresh app");

        assert_eq!(
            count(&client, &DokkuCommand::BuildsReport { app: app("alpha") }),
            1
        );
        assert_eq!(
            count(
                &client,
                &DokkuCommand::AppLinks {
                    plugin: "postgres".into(),
                    app: app("alpha"),
                }
            ),
            1
        );
        let info = store
            .current()
            .await
            .expect("snapshot")
            .app_info("alpha")
            .cloned()
            .expect("app info");
        assert_eq!(info.image_status, Some(ImageStatus::Built));
        assert_eq!(
            info.links,
            Some(vec![ServiceLink {
                plugin: "postgres".into(),
                service: "roboswarm-db".into(),
            }])
        );
        assert_eq!(info.dns_record_exists, Some(true));
    }

    #[tokio::test]
    async fn refresh_app_reports_skips_the_detail_pass() {
        let client = Arc::new(details_client());
        let (store, _dir) = store_with(client.clone()).await;
        store.ensure_loaded().await.expect("load");

        store
            .refresh_app_reports("alpha")
            .await
            .expect("cheap refresh");

        assert_eq!(
            count(&client, &DokkuCommand::PsReport { app: app("alpha") }),
            2,
            "ps report re-fetched"
        );
        assert_eq!(
            count(&client, &DokkuCommand::BuildsReport { app: app("alpha") }),
            0,
            "no build report on the cheap pass"
        );
        assert_eq!(
            count(
                &client,
                &DokkuCommand::AppLinks {
                    plugin: "postgres".into(),
                    app: app("alpha"),
                }
            ),
            0,
            "no app links on the cheap pass"
        );
    }

    #[tokio::test]
    async fn shared_row_is_visible_to_a_second_store() {
        let client_a = Arc::new(
            crate::dokku::MockClient::new().stub(DokkuCommand::AppsList, Ok(apps_list(&["alpha"]))),
        );
        let (store_a, dir) = store_with(client_a.clone()).await;
        store_a.refresh().await.expect("refresh");

        let client_b = Arc::new(crate::dokku::MockClient::new());
        let store_b = second_store(client_b.clone(), &dir).await;

        let snapshot = store_b.current().await.expect("shared row");
        assert!(snapshot.contains("alpha"));
        assert!(
            client_b.calls().is_empty(),
            "second store served the row without any dokku calls"
        );
    }

    #[tokio::test]
    async fn refresh_app_patches_without_losing_sibling_apps() {
        let seeded = crate::dokku::MockClient::new()
            .stub(DokkuCommand::AppsList, Ok(apps_list(&["alpha", "beta"])))
            .stub(
                DokkuCommand::PsReport { app: app("alpha") },
                Ok(ps_report(true, true, 1)),
            )
            .stub(
                DokkuCommand::PsReport { app: app("beta") },
                Ok(ps_report(false, true, 1)),
            )
            .stub(
                DokkuCommand::AppsReport { app: app("alpha") },
                Ok(apps_report()),
            )
            .stub(
                DokkuCommand::AppsReport { app: app("beta") },
                Ok(apps_report()),
            );
        let (store_a, dir) = store_with(Arc::new(seeded)).await;
        store_a.refresh().await.expect("full refresh");

        let patcher = crate::dokku::MockClient::new()
            .stub(DokkuCommand::AppsList, Ok(apps_list(&["alpha", "beta"])))
            .stub(
                DokkuCommand::PsReport { app: app("alpha") },
                Ok(ps_report(false, true, 0)),
            )
            .stub(
                DokkuCommand::AppsReport { app: app("alpha") },
                Ok(apps_report()),
            );
        let store_b = second_store(Arc::new(patcher), &dir).await;
        store_b.refresh_app("alpha").await.expect("patch alpha");

        let snapshot = store_b.current().await.expect("row");
        assert!(snapshot.contains("beta"), "sibling app survives the patch");
        assert_eq!(
            snapshot.ps_report("alpha").map(|r| r.running),
            Some(false),
            "patched app reflects the new state"
        );
    }

    #[tokio::test]
    async fn full_refresh_yields_to_a_row_modified_after_its_build_started() {
        let running = crate::dokku::MockClient::new()
            .stub(DokkuCommand::AppsList, Ok(apps_list(&["alpha"])))
            .stub(
                DokkuCommand::PsReport { app: app("alpha") },
                Ok(ps_report(true, true, 1)),
            )
            .stub(
                DokkuCommand::AppsReport { app: app("alpha") },
                Ok(apps_report()),
            );
        let (store_a, dir) = store_with(Arc::new(running)).await;
        store_a.refresh().await.expect("full pass");

        let patcher = crate::dokku::MockClient::new()
            .stub(DokkuCommand::AppsList, Ok(apps_list(&["alpha"])))
            .stub(
                DokkuCommand::PsReport { app: app("alpha") },
                Ok(ps_report(false, true, 0)),
            )
            .stub(
                DokkuCommand::AppsReport { app: app("alpha") },
                Ok(apps_report()),
            );
        let store_b = second_store(Arc::new(patcher), &dir).await;
        store_b.refresh_app("alpha").await.expect("patch");

        // Forge the row as modified after any build that could still be
        // running, exactly as a patch landing mid-build would be.
        let url = format!("sqlite://{}/snapshot.db", dir.path().display());
        let pool = crate::storage::connect(&url).await.expect("connect");
        sqlx::query("UPDATE snapshots SET updated_at = ? WHERE id = 1")
            .bind(crate::storage::snapshots::now_ms() + 60_000)
            .execute(&pool)
            .await
            .expect("forge modification");

        store_a.refresh().await.expect("second full pass");
        let snapshot = store_a.current().await.expect("row");
        assert_eq!(
            snapshot.ps_report("alpha").map(|report| report.running),
            Some(false),
            "the patch survives the full refresh"
        );
    }

    #[tokio::test]
    async fn corrupt_row_is_rebuilt_on_next_load() {
        let client = Arc::new(
            crate::dokku::MockClient::new().stub(DokkuCommand::AppsList, Ok(apps_list(&["alpha"]))),
        );
        let (store, dir) = store_with(client).await;
        store.refresh().await.expect("refresh");

        let url = format!("sqlite://{}/snapshot.db", dir.path().display());
        let pool = crate::storage::connect(&url).await.expect("connect");
        sqlx::query("UPDATE snapshots SET data = 'not json' WHERE id = 1")
            .execute(&pool)
            .await
            .expect("corrupt row");

        assert!(
            store.current().await.is_none(),
            "corrupt row degrades to none"
        );
        let snapshot = store.ensure_loaded().await.expect("rebuild");
        assert!(snapshot.contains("alpha"));
        assert!(store.current().await.is_some(), "row healed");
    }

    #[tokio::test]
    async fn snapshot_survives_a_storage_round_trip() {
        let client = Arc::new(
            crate::dokku::MockClient::new()
                .stub(DokkuCommand::AppsList, Ok(apps_list(&["alpha"])))
                .stub(
                    DokkuCommand::PsReport { app: app("alpha") },
                    Ok(ps_report(true, true, 2)),
                )
                .stub(
                    DokkuCommand::AppsReport { app: app("alpha") },
                    Ok(apps_report()),
                ),
        );
        let (store, _dir) = store_with(client).await;
        let first = store.refresh().await.expect("refresh");
        let second = store.current().await.expect("reload");
        assert_eq!(first.apps, second.apps);
        assert_eq!(first.ps_reports, second.ps_reports);
        assert_eq!(first.apps_reports, second.apps_reports);
        assert!(
            second.age() < Duration::from_secs(5),
            "age is reconstructed from the stored fetch time"
        );
    }

    #[test]
    fn format_age_buckets() {
        assert_eq!(format_age(Duration::ZERO), "just now");
        assert_eq!(format_age(Duration::from_secs(5)), "5s ago");
        assert_eq!(format_age(Duration::from_secs(59)), "59s ago");
        assert_eq!(format_age(Duration::from_secs(60)), "1m ago");
        assert_eq!(format_age(Duration::from_secs(3599)), "59m ago");
        assert_eq!(format_age(Duration::from_secs(3600)), "1h ago");
        assert_eq!(format_age(Duration::from_secs(7200)), "2h ago");
    }

    /// Fails every `apps:list` after the first, to exercise keep-last-good.
    struct FailSecondList {
        list_calls: Mutex<usize>,
        good: Arc<crate::dokku::MockClient>,
    }

    #[async_trait::async_trait]
    impl DokkuClient for FailSecondList {
        async fn exec(&self, command: &DokkuCommand) -> Result<DokkuOutput, DokkuError> {
            if matches!(command, DokkuCommand::AppsList) {
                let mut calls = self.list_calls.lock().await;
                *calls += 1;
                if *calls > 1 {
                    return Err(DokkuError::Connect("down".into()));
                }
            }
            self.good.exec(command).await
        }
    }

    /// Serves `apps:list` from mutable state while delegating reports to a `MockClient`.
    struct MutableListClient {
        list: Arc<Mutex<DokkuOutput>>,
        reports: Arc<crate::dokku::MockClient>,
        calls: Mutex<Vec<DokkuCommand>>,
    }

    #[async_trait::async_trait]
    impl DokkuClient for MutableListClient {
        async fn exec(&self, command: &DokkuCommand) -> Result<DokkuOutput, DokkuError> {
            self.calls.lock().await.push(command.clone());
            if matches!(command, DokkuCommand::AppsList) {
                return Ok(self.list.lock().await.clone());
            }
            self.reports.exec(command).await
        }
    }
}
