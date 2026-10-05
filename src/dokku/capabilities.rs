use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::sync::Mutex;

use crate::domain::capabilities::{
    Capabilities, CapabilityFamily, parse_dokku_version, parse_help_supports_tail,
    parse_plugin_names,
};
use crate::domain::command::DokkuCommand;
use crate::storage::capabilities::{SqliteCapabilities, StoredCapabilities};
use crate::storage::snapshots::now_ms;

use super::client::{DokkuClient, DokkuError};

#[derive(Debug, thiserror::Error)]
pub enum CapabilitiesError {
    #[error(transparent)]
    Dokku(#[from] DokkuError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("capabilities serialization error: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// JSON payload persisted in the singleton `capabilities` row.
#[derive(Debug, Serialize, Deserialize)]
struct CapabilitiesPayload {
    version: Option<String>,
    plugins: Vec<String>,
    log_sources: Vec<String>,
}

fn payload_from(capabilities: &Capabilities) -> CapabilitiesPayload {
    CapabilitiesPayload {
        version: capabilities
            .dokku_version
            .map(|version| version.to_string()),
        plugins: capabilities.enabled_plugins.clone(),
        log_sources: capabilities.log_sources.clone(),
    }
}

fn capabilities_from(payload: &CapabilitiesPayload) -> Capabilities {
    Capabilities {
        dokku_version: payload
            .version
            .as_ref()
            .and_then(|raw| parse_dokku_version(raw)),
        enabled_plugins: payload.plugins.clone(),
        log_sources: payload.log_sources.clone(),
    }
}

/// Read-only probe of the host: dokku version, enabled plugins, and the
/// live-tail support of each log family (from `<family> --help`). The version
/// command is the liveness signal — its failure aborts the probe so the last
/// good row is kept. Plugin and family failures degrade: a missing version
/// stays `None` and a failing family probe is simply not in `log_sources`.
pub async fn probe_capabilities(client: &dyn DokkuClient) -> Result<Capabilities, DokkuError> {
    let output = client.exec(&DokkuCommand::DokkuVersion).await?;
    let dokku_version = parse_dokku_version(&output.stdout);
    let enabled_plugins = client
        .exec(&DokkuCommand::PluginList)
        .await
        .ok()
        .map(|output| parse_plugin_names(&output.stdout))
        .unwrap_or_default();

    let mut log_sources = Vec::new();
    for family in CapabilityFamily::all() {
        match client.exec(&DokkuCommand::Help { family }).await {
            Ok(output) => {
                if parse_help_supports_tail(&output.stdout, family.command_name()) {
                    log_sources.push(family.command_name().to_owned());
                }
            }
            // A transient failure would mark a healthy family unsupported for
            // a whole refresh interval — abort instead, keeping the last good
            // row (matching the version command's liveness semantics).
            Err(err) if err.is_transient() => return Err(err),
            // A real rejection (e.g. unknown command) means unsupported.
            Err(_) => {}
        }
    }
    log_sources.sort();

    Ok(Capabilities {
        dokku_version,
        enabled_plugins,
        log_sources,
    })
}

/// Publishes the probed capabilities to a singleton SQLite row every process
/// reads, so one probe answers for every container and a cold start costs zero
/// SSH. All SSH work happens before the row write.
pub struct CapabilitiesStore {
    client: Arc<dyn DokkuClient>,
    store: SqliteCapabilities,
    write_lock: Mutex<()>,
}

impl CapabilitiesStore {
    pub fn new(client: Arc<dyn DokkuClient>, pool: SqlitePool) -> Self {
        Self {
            client,
            store: SqliteCapabilities::new(pool),
            write_lock: Mutex::new(()),
        }
    }

    /// Reads the shared row. A corrupt row degrades to `None` (the next
    /// `ensure_loaded` rebuilds it) and a load failure logs and degrades the
    /// same way rather than taking the UI down.
    pub async fn current(&self) -> Option<Arc<Capabilities>> {
        match self.store.load().await {
            Ok(Some(stored)) => match serde_json::from_str::<CapabilitiesPayload>(&stored.data) {
                Ok(payload) => Some(Arc::new(capabilities_from(&payload))),
                Err(err) => {
                    tracing::warn!(error = %err, "capabilities row is corrupt; it will be rebuilt");
                    None
                }
            },
            Ok(None) => None,
            Err(err) => {
                tracing::warn!(error = %err, "capabilities load failed");
                None
            }
        }
    }

    /// Cold-start path: serve the shared row if any container has published
    /// one (no SSH at all), otherwise probe and publish.
    pub async fn ensure_loaded(&self) -> Result<Arc<Capabilities>, CapabilitiesError> {
        if let Some(capabilities) = self.current().await {
            return Ok(capabilities);
        }
        let _guard = self.write_lock.lock().await;
        if let Some(capabilities) = self.current().await {
            return Ok(capabilities);
        }
        let build_started = now_ms();
        let capabilities = probe_capabilities(self.client.as_ref()).await?;
        let stored = stored_from(&capabilities)?;
        if !self
            .store
            .save_if_unmodified_since(&stored, build_started)
            .await?
        {
            // Another container published (or healed the row) while we probed;
            // serve theirs.
            if let Some(capabilities) = self.current().await {
                return Ok(capabilities);
            }
        }
        Ok(Arc::new(capabilities))
    }

    /// Fresh probe, published to the shared row unless the row was modified
    /// after this probe started. Used by the background refresher; a published
    /// pass is immediately visible to every process.
    pub async fn refresh(&self) -> Result<Arc<Capabilities>, CapabilitiesError> {
        let _guard = self.write_lock.lock().await;
        let build_started = now_ms();
        let capabilities = probe_capabilities(self.client.as_ref()).await?;
        let stored = stored_from(&capabilities)?;
        if !self
            .store
            .save_if_unmodified_since(&stored, build_started)
            .await?
        {
            tracing::debug!("capabilities row was modified while probing; keeping the newer row");
        }
        Ok(Arc::new(capabilities))
    }
}

fn stored_from(capabilities: &Capabilities) -> Result<StoredCapabilities, serde_json::Error> {
    let payload = payload_from(capabilities);
    Ok(StoredCapabilities::new(serde_json::to_string(&payload)?))
}

/// Re-probe forever on `interval`. The first probe runs immediately so a
/// fresh deploy has a warm row (otherwise the logs badge reads "unknown" and
/// the stream gate is inert for up to a full interval). Failures keep the
/// last good row; the loop logs and retries rather than taking the UI down.
pub fn spawn_capabilities_refresher(
    store: Arc<CapabilitiesStore>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if let Err(err) = store.refresh().await {
                tracing::warn!(error = %err, "capabilities refresh failed; serving last good data");
            }
            tokio::time::sleep(interval).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dokku::{DokkuOutput, MockClient};

    const DOKKU_VERSION: &str = include_str!("../../tests/fixtures/dokku_version.txt");
    const PLUGIN_LIST: &str = include_str!("../../tests/fixtures/plugin_list.txt");
    const LOGS_HELP: &str = include_str!("../../tests/fixtures/logs_help.txt");
    const NGINX_HELP: &str = include_str!("../../tests/fixtures/nginx_help.txt");

    fn logs_help() -> DokkuCommand {
        DokkuCommand::Help {
            family: CapabilityFamily::Logs,
        }
    }

    fn access_help() -> DokkuCommand {
        DokkuCommand::Help {
            family: CapabilityFamily::NginxAccessLogs,
        }
    }

    fn error_help() -> DokkuCommand {
        DokkuCommand::Help {
            family: CapabilityFamily::NginxErrorLogs,
        }
    }

    /// A client covering every probe command with fixture output.
    fn probed_client() -> MockClient {
        MockClient::new()
            .stub(
                DokkuCommand::DokkuVersion,
                Ok(DokkuOutput::ok(DOKKU_VERSION)),
            )
            .stub(DokkuCommand::PluginList, Ok(DokkuOutput::ok(PLUGIN_LIST)))
            .stub(logs_help(), Ok(DokkuOutput::ok(LOGS_HELP)))
            .stub(access_help(), Ok(DokkuOutput::ok(NGINX_HELP)))
            .stub(error_help(), Ok(DokkuOutput::ok(NGINX_HELP)))
    }

    async fn store_with(client: Arc<dyn DokkuClient>) -> (CapabilitiesStore, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let url = format!("sqlite://{}/capabilities.db", dir.path().display());
        let pool = crate::storage::connect(&url).await.expect("connect");
        (CapabilitiesStore::new(client, pool), dir)
    }

    async fn second_store(
        client: Arc<dyn DokkuClient>,
        dir: &tempfile::TempDir,
    ) -> CapabilitiesStore {
        let url = format!("sqlite://{}/capabilities.db", dir.path().display());
        let pool = crate::storage::connect(&url).await.expect("connect");
        CapabilitiesStore::new(client, pool)
    }

    #[tokio::test]
    async fn probe_parses_version_plugins_and_log_sources() {
        let client = probed_client();
        let caps = probe_capabilities(&client).await.expect("probe");
        assert_eq!(
            caps.dokku_version.map(|version| version.to_string()),
            Some("0.38.4".to_owned())
        );
        assert!(caps.enabled_plugins.contains(&"nginx-vhosts".into()));
        assert!(caps.enabled_plugins.contains(&"redis".into()));
        assert!(caps.enabled_plugins.contains(&"00_dokku-standard".into()));
        assert_eq!(
            caps.log_sources,
            vec!["logs", "nginx:access-logs", "nginx:error-logs"]
        );
    }

    #[tokio::test]
    async fn probe_degrades_missing_families_and_unparseable_version() {
        let client = MockClient::new()
            .stub(
                DokkuCommand::DokkuVersion,
                Ok(DokkuOutput::ok("dokku version latest\n")),
            )
            .stub(DokkuCommand::PluginList, Ok(DokkuOutput::ok(PLUGIN_LIST)))
            .stub(logs_help(), Ok(DokkuOutput::ok(LOGS_HELP)))
            .stub(
                access_help(),
                Err(DokkuError::Exit {
                    code: 1,
                    stderr: "unknown command".into(),
                }),
            )
            .stub(
                error_help(),
                Ok(DokkuOutput::ok("Usage: dokku nginx:error-logs <app>")),
            );

        let caps = probe_capabilities(&client).await.expect("probe");

        assert_eq!(caps.dokku_version, None);
        assert_eq!(
            caps.log_sources,
            vec!["logs"],
            "only the probed families are listed"
        );
    }

    #[tokio::test]
    async fn probe_propagates_version_command_failure() {
        let client = MockClient::with_default(Err(DokkuError::Connect("down".into())));
        assert!(matches!(
            probe_capabilities(&client).await,
            Err(DokkuError::Connect(_))
        ));
    }

    #[tokio::test]
    async fn ensure_loaded_fetches_only_once() {
        let client = Arc::new(probed_client());
        let (store, _dir) = store_with(client.clone()).await;

        let first = store.ensure_loaded().await.expect("first");
        let second = store.ensure_loaded().await.expect("second");

        assert_eq!(
            first.log_sources, second.log_sources,
            "second load served the shared row"
        );
        assert_eq!(
            client
                .calls()
                .iter()
                .filter(|call| matches!(call, DokkuCommand::DokkuVersion))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn capabilities_survive_a_storage_round_trip() {
        let client = Arc::new(probed_client());
        let (store, _dir) = store_with(client.clone()).await;
        let first = store.refresh().await.expect("refresh");
        let second = store.current().await.expect("reload");
        assert_eq!(first, second);
        assert_eq!(
            second.dokku_version.map(|version| version.to_string()),
            Some("0.38.4".to_owned())
        );
    }

    #[tokio::test]
    async fn shared_row_is_visible_to_a_second_store() {
        let client_a = Arc::new(probed_client());
        let (store_a, dir) = store_with(client_a.clone()).await;
        store_a.refresh().await.expect("refresh");

        let client_b = Arc::new(MockClient::new());
        let store_b = second_store(client_b.clone(), &dir).await;

        let caps = store_b.current().await.expect("shared row");
        assert!(caps.enabled_plugins.contains(&"nginx-vhosts".into()));
        assert!(
            client_b.calls().is_empty(),
            "second store served the row without any dokku calls"
        );
    }

    #[tokio::test]
    async fn corrupt_row_is_rebuilt_on_next_load() {
        let client = Arc::new(probed_client());
        let (store, dir) = store_with(client.clone()).await;
        store.refresh().await.expect("refresh");

        let url = format!("sqlite://{}/capabilities.db", dir.path().display());
        let pool = crate::storage::connect(&url).await.expect("connect");
        sqlx::query("UPDATE capabilities SET data = 'not json' WHERE id = 1")
            .execute(&pool)
            .await
            .expect("corrupt row");

        assert!(
            store.current().await.is_none(),
            "corrupt row degrades to none"
        );
        let caps = store.ensure_loaded().await.expect("rebuild");
        assert!(caps.enabled_plugins.contains(&"nginx-vhosts".into()));
        assert!(store.current().await.is_some(), "row healed");
    }

    #[tokio::test]
    async fn failed_refresh_keeps_last_good_row() {
        let good = Arc::new(probed_client());
        let client = Arc::new(FailSecondProbe {
            probes: Mutex::new(0),
            good: good.clone(),
        });
        let (store, _dir) = store_with(client).await;

        store.refresh().await.expect("first refresh");
        let before = store.current().await.expect("capabilities");

        assert!(store.refresh().await.is_err(), "second refresh fails");
        let after = store.current().await.expect("still published");
        assert_eq!(before, after, "last good row retained");
    }

    /// Fails every probe command after the first pass, to exercise keep-last-good.
    struct FailSecondProbe {
        probes: Mutex<usize>,
        good: Arc<MockClient>,
    }

    #[async_trait::async_trait]
    impl DokkuClient for FailSecondProbe {
        async fn exec(&self, command: &DokkuCommand) -> Result<DokkuOutput, DokkuError> {
            if matches!(command, DokkuCommand::DokkuVersion) {
                let mut probes = self.probes.lock().await;
                *probes += 1;
                if *probes > 1 {
                    return Err(DokkuError::Connect("down".into()));
                }
            }
            self.good.exec(command).await
        }
    }
}
