use crate::domain::command::DokkuCommand;
use crate::domain::parse::{parse_service_stats, parse_storage_entries, parse_volume_usage};
use crate::domain::service_name::ServiceName;
use crate::domain::service_plugin::ServicePlugin;
use crate::domain::types::{ServiceStats, StorageEntry, VolumeUsage};

use super::client::{DokkuClient, DokkuError};

/// Live container stats via the fixed `<plugin>:enter` script. `None` when
/// the output held nothing recognisable (e.g. only a plugin banner).
pub async fn service_stats(
    client: &dyn DokkuClient,
    plugin: ServicePlugin,
    service: &ServiceName,
) -> Result<Option<ServiceStats>, DokkuError> {
    let output = client
        .exec(&DokkuCommand::ServiceStats {
            plugin,
            service: service.clone(),
        })
        .await?;
    Ok(parse_service_stats(&output.stdout))
}

/// Registered storage entries, for mapping a mount's host path onto the entry
/// name `storage:exec` needs. A failure yields an empty list so the volumes
/// page still renders (per-row usage degrades to an em dash).
pub async fn storage_entries(client: &dyn DokkuClient) -> Vec<StorageEntry> {
    client
        .exec(&DokkuCommand::StorageListEntries)
        .await
        .ok()
        .map(|output| parse_storage_entries(&output.stdout))
        .unwrap_or_default()
}

/// Disk usage for one storage entry, from the throwaway-container script.
pub async fn volume_usage(
    client: &dyn DokkuClient,
    entry: &str,
) -> Result<Option<VolumeUsage>, DokkuError> {
    let output = client
        .exec(&DokkuCommand::StorageUsage {
            entry: entry.to_owned(),
        })
        .await?;
    Ok(parse_volume_usage(&output.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dokku::{DokkuOutput, MockClient};

    const REDIS_STATS: &str = include_str!("../../tests/fixtures/redis_stats.txt");
    const VOLUME_USAGE: &str = include_str!("../../tests/fixtures/volume_usage.txt");
    const LIST_ENTRIES: &str = include_str!("../../tests/fixtures/list_entries.json");

    fn redis() -> ServicePlugin {
        ServicePlugin::try_from("redis").expect("plugin")
    }

    fn candid() -> ServiceName {
        ServiceName::try_from("candid").expect("service")
    }

    #[tokio::test]
    async fn service_stats_parses_fixture() {
        let client = MockClient::new().stub(
            DokkuCommand::ServiceStats {
                plugin: redis(),
                service: candid(),
            },
            Ok(DokkuOutput::ok(REDIS_STATS)),
        );

        let stats = service_stats(&client, redis(), &candid())
            .await
            .expect("stats")
            .expect("parsed");

        assert_eq!(stats.cpus, Some(4));
        assert_eq!(stats.data_kb, Some(12));
        assert_eq!(stats.memory_used_label(), "6.7 MiB");
    }

    #[tokio::test]
    async fn service_stats_error_propagates() {
        let client = MockClient::new().stub(
            DokkuCommand::ServiceStats {
                plugin: redis(),
                service: candid(),
            },
            Err(DokkuError::Exit {
                code: 1,
                stderr: "Service container is not running".into(),
            }),
        );

        assert!(service_stats(&client, redis(), &candid()).await.is_err());
    }

    #[tokio::test]
    async fn storage_entries_parses_fixture() {
        let client = MockClient::new().stub(
            DokkuCommand::StorageListEntries,
            Ok(DokkuOutput::ok(LIST_ENTRIES)),
        );

        let entries = storage_entries(&client).await;
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].name, "legacy-151e4f1a23");
    }

    #[tokio::test]
    async fn storage_entries_failure_degrades_to_empty() {
        let client = MockClient::new().stub(
            DokkuCommand::StorageListEntries,
            Err(DokkuError::Exit {
                code: 1,
                stderr: "boom".into(),
            }),
        );

        assert!(storage_entries(&client).await.is_empty());
    }

    #[tokio::test]
    async fn volume_usage_parses_fixture() {
        let client = MockClient::new().stub(
            DokkuCommand::StorageUsage {
                entry: "legacy-90db719326".into(),
            },
            Ok(DokkuOutput::ok(VOLUME_USAGE)),
        );

        let usage = volume_usage(&client, "legacy-90db719326")
            .await
            .expect("usage")
            .expect("parsed");
        assert_eq!(usage.used_kb, Some(164));
        assert_eq!(usage.used_label(), "164 KiB");
    }

    #[tokio::test]
    async fn volume_usage_error_propagates() {
        let client = MockClient::new().stub(
            DokkuCommand::StorageUsage {
                entry: "legacy-90db719326".into(),
            },
            Err(DokkuError::Exit {
                code: 1,
                stderr: "docker: not found".into(),
            }),
        );

        assert!(volume_usage(&client, "legacy-90db719326").await.is_err());
    }
}
