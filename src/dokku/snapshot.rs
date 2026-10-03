use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::future::join_all;
use tokio::sync::{Mutex, RwLock, Semaphore};

use crate::domain::AppName;
use crate::domain::command::DokkuCommand;
use crate::domain::parse::{parse_apps_list, parse_apps_report, parse_ps_report};
use crate::domain::types::{AppInfo, PsReport};

use super::client::{DokkuClient, DokkuError};

const MAX_CONCURRENT_REPORTS: usize = 4;

/// In-memory, parsed view of the dokku host. A background task keeps this warm so
/// request handlers never block on SSH; reads are an `Arc` clone.
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

/// Owns the published snapshot. All mutations are serialized through `write_lock` so a
/// background full refresh and a per-app refresh can never interleave and lose an update.
pub struct SnapshotStore {
    client: Arc<dyn DokkuClient>,
    current: RwLock<Option<Arc<Snapshot>>>,
    write_lock: Mutex<()>,
}

impl SnapshotStore {
    pub fn new(client: Arc<dyn DokkuClient>) -> Self {
        Self {
            client,
            current: RwLock::new(None),
            write_lock: Mutex::new(()),
        }
    }

    pub async fn current(&self) -> Option<Arc<Snapshot>> {
        self.current.read().await.clone()
    }

    pub async fn refresh(&self) -> Result<Arc<Snapshot>, DokkuError> {
        let _guard = self.write_lock.lock().await;
        self.refresh_locked().await
    }

    /// Cold-start path: if nothing is published yet, build one synchronously so the first
    /// request still works (and still surfaces ssh failures like before).
    pub async fn ensure_loaded(&self) -> Result<Arc<Snapshot>, DokkuError> {
        if let Some(snapshot) = self.current().await {
            return Ok(snapshot);
        }
        let _guard = self.write_lock.lock().await;
        if let Some(snapshot) = self.current().await {
            return Ok(snapshot);
        }
        self.refresh_locked().await
    }

    /// Refreshes a single app (or removes it if dokku no longer lists it). Used after
    /// mutating actions and as the live fallback when an app is not in the snapshot.
    pub async fn refresh_app(&self, name: &str) -> Result<(), DokkuError> {
        let _guard = self.write_lock.lock().await;

        let output = self.client.exec(&DokkuCommand::AppsList).await?;
        let names = parse_apps_list(&output.stdout);

        if !names.iter().any(|listed| listed == name) {
            self.mutate(|snapshot| {
                snapshot.apps.retain(|app| app != name);
                snapshot.ps_reports.remove(name);
                snapshot.apps_reports.remove(name);
            })
            .await;
            return Ok(());
        }

        let (ps, info) = match AppName::try_from(name.to_owned()) {
            Ok(app) => {
                let ps = self
                    .client
                    .exec(&DokkuCommand::PsReport { app: app.clone() })
                    .await
                    .ok()
                    .and_then(|output| parse_ps_report(&output.stdout).ok());
                let info = self
                    .client
                    .exec(&DokkuCommand::AppsReport { app })
                    .await
                    .ok()
                    .and_then(|output| parse_apps_report(&output.stdout, name));
                (ps, info)
            }
            Err(_) => (None, None),
        };

        self.mutate(|snapshot| {
            if !snapshot.contains(name) {
                snapshot.apps.push(name.to_owned());
                snapshot.apps.sort();
            }
            snapshot.ps_reports.insert(name.to_owned(), ps);
            snapshot.apps_reports.insert(name.to_owned(), info);
        })
        .await;
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

    async fn refresh_locked(&self) -> Result<Arc<Snapshot>, DokkuError> {
        let snapshot = Arc::new(build_snapshot(self.client.as_ref()).await?);
        *self.current.write().await = Some(snapshot.clone());
        Ok(snapshot)
    }

    async fn mutate<F>(&self, edit: F)
    where
        F: FnOnce(&mut Snapshot),
    {
        let mut guard = self.current.write().await;
        let mut next = match guard.as_ref() {
            Some(snapshot) => (**snapshot).clone(),
            None => Snapshot {
                fetched_at: Instant::now(),
                apps: Vec::new(),
                ps_reports: HashMap::new(),
                apps_reports: HashMap::new(),
            },
        };
        edit(&mut next);
        *guard = Some(Arc::new(next));
    }
}

fn listed_app(snapshot: &Snapshot, name: &str) -> Option<AppName> {
    if !snapshot.contains(name) {
        return None;
    }
    AppName::try_from(name.to_owned()).ok()
}

/// Refresh forever, sleeping `interval` after each attempt. Failures keep the last good
/// snapshot; the loop logs and retries rather than taking the UI down.
pub fn spawn_refresher(
    store: Arc<SnapshotStore>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if let Err(err) = store.refresh().await {
                tracing::warn!(error = %err, "snapshot refresh failed; serving last good data");
            }
            tokio::time::sleep(interval).await;
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

    fn app(name: &str) -> AppName {
        AppName::try_from(name).expect("valid app name")
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
        let store = SnapshotStore::new(client.clone());

        assert!(store.current().await.is_none());
        let first = store.ensure_loaded().await.expect("first");
        let second = store.ensure_loaded().await.expect("second");
        assert!(Arc::ptr_eq(&first, &second));
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
        let store = SnapshotStore::new(client);

        store.refresh().await.expect("first refresh");
        let before = store.current().await.expect("snapshot");

        assert!(store.refresh().await.is_err(), "second refresh fails");
        let after = store.current().await.expect("still published");
        assert!(Arc::ptr_eq(&before, &after), "last good snapshot retained");
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
        let store = SnapshotStore::new(client.clone());
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
        let store = SnapshotStore::new(client);
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
        let store = SnapshotStore::new(client);

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
        let store = SnapshotStore::new(client.clone());
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
        let store = Arc::new(SnapshotStore::new(client));
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
