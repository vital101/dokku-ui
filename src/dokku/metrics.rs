use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;

use crate::domain::capabilities::Support;
use crate::domain::command::DokkuCommand;
use crate::domain::metrics::{align_bucket, data_size_sample, sample_from_stats};
use crate::domain::parse::parse_service_list;
use crate::domain::service_name::ServiceName;
use crate::domain::service_plugin::ServicePlugin;
use crate::storage::metrics::SqliteMetrics;
use crate::storage::snapshots::now_ms;

use super::capabilities::CapabilitiesStore;
use super::client::{DokkuClient, DokkuError};
use super::stats_pages::{service_data_usage, service_stats};

#[derive(Debug, thiserror::Error)]
pub enum MetricsError {
    #[error(transparent)]
    Dokku(#[from] DokkuError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

/// Samples service resource metrics into shared SQLite. The poll lease makes
/// the loop safe in a multi-container deploy: only one container claims each
/// interval, and the unique bucket indexes collapse any overlapping writes.
pub struct MetricsStore {
    client: Arc<dyn DokkuClient>,
    capabilities: Arc<CapabilitiesStore>,
    metrics: SqliteMetrics,
    refresh_secs: u64,
    du_every_secs: u64,
    ttl_secs: u64,
}

impl MetricsStore {
    pub fn new(
        client: Arc<dyn DokkuClient>,
        capabilities: Arc<CapabilitiesStore>,
        pool: SqlitePool,
        refresh_secs: u64,
        du_every_secs: u64,
        ttl_secs: u64,
    ) -> Self {
        Self {
            client,
            capabilities,
            metrics: SqliteMetrics::new(pool),
            refresh_secs,
            du_every_secs,
            ttl_secs,
        }
    }

    pub async fn poll_once(&self) -> Result<(), MetricsError> {
        if self.refresh_secs == 0 {
            return Ok(());
        }

        let now_ms = now_ms();
        let interval_ms = self
            .refresh_secs
            .saturating_mul(1_000)
            .try_into()
            .unwrap_or(i64::MAX);
        if !self.metrics.claim_poll(now_ms, interval_ms).await? {
            return Ok(());
        }

        let capabilities = match self.capabilities.ensure_loaded().await {
            Ok(capabilities) => capabilities,
            Err(err) => {
                tracing::warn!(error = %err, "metrics poll skipped; capabilities unavailable");
                return Ok(());
            }
        };
        let bucket_ms = align_bucket(now_ms, interval_ms);

        for plugin in ServicePlugin::all() {
            if let Support::PluginMissing { .. } = capabilities.supports_plugin(plugin.as_str()) {
                continue;
            }
            if let Err(err) = self.poll_plugin(plugin, now_ms, bucket_ms).await {
                tracing::warn!(plugin = plugin.as_str(), error = %err, "metrics poll failed");
            }
        }

        if self.ttl_secs > 0 {
            let ttl_ms = self.ttl_secs.saturating_mul(1_000).min(i64::MAX as u64) as i64;
            self.metrics.prune(now_ms.saturating_sub(ttl_ms)).await?;
        }

        Ok(())
    }

    async fn poll_plugin(
        &self,
        plugin: ServicePlugin,
        now_ms: i64,
        stats_bucket_ms: i64,
    ) -> Result<(), MetricsError> {
        let output = self
            .client
            .exec(&DokkuCommand::ServiceList { plugin })
            .await?;

        for raw in parse_service_list(&output.stdout) {
            let Ok(service) = ServiceName::try_from(raw) else {
                continue;
            };
            self.poll_service(plugin, &service, now_ms, stats_bucket_ms)
                .await?;
        }
        Ok(())
    }

    async fn poll_service(
        &self,
        plugin: ServicePlugin,
        service: &ServiceName,
        now_ms: i64,
        stats_bucket_ms: i64,
    ) -> Result<(), MetricsError> {
        match service_stats(self.client.as_ref(), plugin, service).await {
            Ok(Some(stats)) if !stats.is_empty() => {
                let sample = sample_from_stats(plugin, service, &stats, now_ms, stats_bucket_ms);
                self.metrics.record_sample(&sample).await?;
            }
            Ok(Some(_)) | Ok(None) => {}
            Err(err) if err.is_not_running() => return Ok(()),
            Err(err) => {
                tracing::warn!(
                    plugin = plugin.as_str(),
                    service = service.as_str(),
                    error = %err,
                    "service stats poll failed"
                );
            }
        }

        if self.du_every_secs == 0 || !plugin.supports_data_size() {
            return Ok(());
        }
        let du_every_ms = self
            .du_every_secs
            .saturating_mul(1_000)
            .try_into()
            .unwrap_or(i64::MAX);
        if !self
            .metrics
            .claim_data_attempt(plugin, service, now_ms, du_every_ms)
            .await?
        {
            return Ok(());
        }

        match service_data_usage(self.client.as_ref(), plugin, service).await {
            Ok(Some(data_kb)) => {
                let bucket_ms = align_bucket(now_ms, du_every_ms);
                let sample = data_size_sample(plugin, service, data_kb, now_ms, bucket_ms);
                self.metrics.record_data_size(&sample).await?;
            }
            Ok(None) => {}
            Err(err) if err.is_not_running() => {}
            Err(err) => {
                tracing::warn!(
                    plugin = plugin.as_str(),
                    service = service.as_str(),
                    error = %err,
                    "service data-size poll failed"
                );
            }
        }
        Ok(())
    }
}

/// Polls forever, immediately first. Failures are logged and do not stop the
/// loop; each container competes through the singleton poll lease, so N
/// containers still produce one SSH poll per interval.
pub fn spawn_metrics_refresher(
    store: Arc<MetricsStore>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if let Err(err) = store.poll_once().await {
                tracing::warn!(error = %err, "metrics refresh failed; keeping last stored samples");
            }
            tokio::time::sleep(interval).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dokku::{CapabilitiesStore, DokkuOutput, MockClient};
    use crate::domain::capabilities::CapabilityFamily;

    const DOKKU_VERSION: &str = include_str!("../../tests/fixtures/dokku_version.txt");
    const PLUGIN_LIST: &str = include_str!("../../tests/fixtures/plugin_list.txt");
    const LOGS_HELP: &str = include_str!("../../tests/fixtures/logs_help.txt");
    const NGINX_HELP: &str = include_str!("../../tests/fixtures/nginx_help.txt");
    const REDIS_LIST: &str = include_str!("../../tests/fixtures/redis_list.txt");
    const REDIS_STATS: &str = include_str!("../../tests/fixtures/redis_stats.txt");
    const REDIS_DU: &str = include_str!("../../tests/fixtures/redis_du.txt");

    fn redis() -> ServicePlugin {
        ServicePlugin::try_from("redis").expect("redis plugin")
    }

    fn candid() -> ServiceName {
        ServiceName::try_from("candid").expect("service name")
    }

    fn stats_command() -> DokkuCommand {
        DokkuCommand::ServiceStats {
            plugin: redis(),
            service: candid(),
        }
    }

    fn du_command() -> DokkuCommand {
        DokkuCommand::ServiceDataUsage {
            plugin: redis(),
            service: candid(),
        }
    }

    fn capabilities_commands(client: MockClient) -> MockClient {
        client
            .stub(
                DokkuCommand::DokkuVersion,
                Ok(DokkuOutput::ok(DOKKU_VERSION)),
            )
            .stub(DokkuCommand::PluginList, Ok(DokkuOutput::ok(PLUGIN_LIST)))
            .stub(
                DokkuCommand::Help {
                    family: CapabilityFamily::Logs,
                },
                Ok(DokkuOutput::ok(LOGS_HELP)),
            )
            .stub(
                DokkuCommand::Help {
                    family: CapabilityFamily::NginxAccessLogs,
                },
                Ok(DokkuOutput::ok(NGINX_HELP)),
            )
            .stub(
                DokkuCommand::Help {
                    family: CapabilityFamily::NginxErrorLogs,
                },
                Ok(DokkuOutput::ok(NGINX_HELP)),
            )
    }

    fn polling_client() -> MockClient {
        capabilities_commands(
            MockClient::new()
                .stub(
                    DokkuCommand::ServiceList { plugin: redis() },
                    Ok(DokkuOutput::ok(REDIS_LIST)),
                )
                .stub(stats_command(), Ok(DokkuOutput::ok(REDIS_STATS)))
                .stub(du_command(), Ok(DokkuOutput::ok(REDIS_DU))),
        )
    }

    async fn store_with(
        client: Arc<dyn DokkuClient>,
    ) -> (MetricsStore, SqliteMetrics, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let url = format!("sqlite://{}/metrics.db", dir.path().display());
        let pool = crate::storage::connect(&url).await.expect("connect");
        let capabilities = Arc::new(CapabilitiesStore::new(client.clone(), pool.clone()));
        let metrics = SqliteMetrics::new(pool.clone());
        (
            MetricsStore::new(client, capabilities, pool, 60, 900, 0),
            metrics,
            dir,
        )
    }

    fn count(client: &MockClient, command: &DokkuCommand) -> usize {
        client
            .calls()
            .iter()
            .filter(|call| *call == command)
            .count()
    }

    #[tokio::test]
    async fn poll_once_records_service_stats_and_data_size() {
        let client = Arc::new(polling_client());
        let (store, metrics_repo, _dir) = store_with(client.clone()).await;

        store.poll_once().await.expect("poll");

        assert_eq!(count(&client, &stats_command()), 1);
        assert_eq!(count(&client, &du_command()), 1);

        let aggregates = metrics_repo
            .aggregates(redis(), &candid(), 0, i64::MAX)
            .await
            .expect("aggregates");
        assert_eq!(aggregates.len(), 1);
        assert!(
            aggregates[0]
                .cpu_percent
                .is_some_and(|cpu| (cpu - 0.127).abs() < 0.001),
            "{:?}",
            aggregates[0]
        );

        let data = metrics_repo
            .data_aggregates(redis(), &candid(), 0, i64::MAX)
            .await
            .expect("data");
        assert_eq!(data.len(), 1);
        assert_eq!(data[0].data_bytes, Some(12.0 * 1024.0));
    }

    #[tokio::test]
    async fn poll_once_skips_stopped_services_without_data_size() {
        let inner = Arc::new(polling_client());
        let client = Arc::new(StoppedStatsClient {
            inner: inner.clone(),
        });
        let (store, _metrics, _dir) = store_with(client).await;

        store.poll_once().await.expect("poll");

        assert_eq!(
            count(
                &inner,
                &DokkuCommand::ServiceDataUsage {
                    plugin: redis(),
                    service: candid(),
                }
            ),
            0
        );
    }

    #[tokio::test]
    async fn second_poll_inside_the_interval_does_not_call_ssh() {
        let client = Arc::new(polling_client());
        let (store, _metrics, _dir) = store_with(client.clone()).await;

        store.poll_once().await.expect("first poll");
        store.poll_once().await.expect("second poll");

        assert_eq!(count(&client, &stats_command()), 1);
    }

    #[tokio::test]
    async fn poll_service_spaces_failed_data_size_attempts() {
        let client = Arc::new(
            MockClient::new()
                .stub(stats_command(), Ok(DokkuOutput::ok(REDIS_STATS)))
                .stub(du_command(), Err(DokkuError::Timeout { secs: 300 })),
        );
        let (store, _metrics, _dir) = store_with(client.clone()).await;

        store
            .poll_service(redis(), &candid(), 1_000, 1_000)
            .await
            .expect("first poll service");
        store
            .poll_service(redis(), &candid(), 10_000, 1_000)
            .await
            .expect("second poll service");

        assert_eq!(count(&client, &stats_command()), 2);
        assert_eq!(count(&client, &du_command()), 1);
    }

    #[tokio::test]
    async fn poll_service_skips_data_usage_for_root_data_dirs() {
        let client = Arc::new(MockClient::new().stub(
            DokkuCommand::ServiceStats {
                plugin: ServicePlugin::try_from("memcached").expect("memcached"),
                service: candid(),
            },
            Ok(DokkuOutput::ok(REDIS_STATS)),
        ));
        let (store, _metrics, _dir) = store_with(client.clone()).await;

        store
            .poll_service(
                ServicePlugin::try_from("memcached").expect("memcached"),
                &candid(),
                1_000,
                1_000,
            )
            .await
            .expect("root data dir poll");

        assert_eq!(
            count(
                &client,
                &DokkuCommand::ServiceDataUsage {
                    plugin: ServicePlugin::try_from("memcached").expect("memcached"),
                    service: candid(),
                }
            ),
            0
        );
    }

    struct StoppedStatsClient {
        inner: Arc<MockClient>,
    }

    #[async_trait::async_trait]
    impl DokkuClient for StoppedStatsClient {
        async fn exec(&self, command: &DokkuCommand) -> Result<DokkuOutput, DokkuError> {
            if matches!(command, DokkuCommand::ServiceStats { .. }) {
                return Err(DokkuError::Exit {
                    code: 1,
                    stderr: "Service container is not running".into(),
                });
            }
            self.inner.exec(command).await
        }
    }
}
