use crate::domain::command::DokkuCommand;
use crate::domain::parse::{parse_app_links, parse_logs, parse_service_list};
use crate::domain::service_name::ServiceName;
use crate::domain::service_plugin::ServicePlugin;
use crate::domain::types::{LogLines, ServiceInfo};

use super::client::{DokkuClient, DokkuError};

/// Lists a plugin's services with a live `<plugin>:info` fetch per service so
/// the list page can show status/version. Only the `<plugin>:list` failure
/// propagates (the caller renders a retry card); an unreadable service degrades
/// to [`ServiceInfo::unknown`].
pub async fn plugin_services(
    client: &dyn DokkuClient,
    plugin: ServicePlugin,
) -> Result<Vec<ServiceInfo>, DokkuError> {
    let output = client.exec(&DokkuCommand::ServiceList { plugin }).await?;
    let names = parse_service_list(&output.stdout);

    let mut services = Vec::with_capacity(names.len());
    for name in names {
        let info = match super::app_pages::service_info(client, plugin.as_str(), &name).await {
            Ok(Some(info)) => info,
            _ => ServiceInfo::unknown(plugin.as_str(), name),
        };
        services.push(info);
    }
    Ok(services)
}

/// `<plugin>:links <service>` -> the apps the service is linked to.
pub async fn service_linked_apps(
    client: &dyn DokkuClient,
    plugin: ServicePlugin,
    service: &ServiceName,
) -> Result<Vec<String>, DokkuError> {
    let output = client
        .exec(&DokkuCommand::ServiceLinks {
            plugin,
            service: service.clone(),
        })
        .await?;
    Ok(parse_app_links(&output.stdout))
}

/// Fetches the service container's most recent log lines, bounded to
/// `num_lines` for display (the plugin's own tail flag either follows forever
/// or is not accepted by all installed versions).
pub async fn service_logs(
    client: &dyn DokkuClient,
    plugin: ServicePlugin,
    service: &ServiceName,
    num_lines: u32,
) -> Result<LogLines, DokkuError> {
    let output = client
        .exec(&DokkuCommand::ServiceLogs {
            plugin,
            service: service.clone(),
            num_lines,
        })
        .await?;
    Ok(parse_logs(&output.stdout).tail(num_lines as usize))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dokku::{DokkuOutput, MockClient};

    const REDIS_LIST: &str = include_str!("../../tests/fixtures/redis_list.txt");
    const REDIS_INFO: &str = include_str!("../../tests/fixtures/redis_info.txt");
    const REDIS_LOGS: &str = include_str!("../../tests/fixtures/redis_logs.txt");

    fn redis() -> ServicePlugin {
        ServicePlugin::try_from("redis").expect("plugin")
    }

    #[tokio::test]
    async fn plugin_services_fetches_info_per_service() {
        let client = MockClient::new()
            .stub(
                DokkuCommand::ServiceList { plugin: redis() },
                Ok(DokkuOutput::ok(REDIS_LIST)),
            )
            .stub(
                DokkuCommand::ServiceInfo {
                    plugin: "redis".into(),
                    service: "candid".into(),
                },
                Ok(DokkuOutput::ok(REDIS_INFO)),
            );

        let services = plugin_services(&client, redis()).await.expect("services");

        assert_eq!(services.len(), 1);
        assert_eq!(services[0].service, "candid");
        assert_eq!(services[0].plugin, "redis");
        assert_eq!(services[0].status, "running");
        assert_eq!(services[0].version, "redis:7.2.4");
        assert!(
            !format!("{:?}", services[0]).contains("redis://"),
            "the DSN never reaches the parsed model"
        );
    }

    #[tokio::test]
    async fn plugin_services_degrades_unreadable_info_to_unknown() {
        let client = MockClient::new()
            .stub(
                DokkuCommand::ServiceList { plugin: redis() },
                Ok(DokkuOutput::ok(REDIS_LIST)),
            )
            .stub(
                DokkuCommand::ServiceInfo {
                    plugin: "redis".into(),
                    service: "candid".into(),
                },
                Err(DokkuError::Exit {
                    code: 1,
                    stderr: "boom".into(),
                }),
            );

        let services = plugin_services(&client, redis()).await.expect("services");

        assert_eq!(services.len(), 1);
        assert_eq!(services[0].status, "");
        assert_eq!(services[0].status_label(), "unknown");
    }

    #[tokio::test]
    async fn plugin_services_empty_list_is_empty() {
        let client = MockClient::new().stub(
            DokkuCommand::ServiceList { plugin: redis() },
            Ok(DokkuOutput::ok(" !     There are no Redis services\n")),
        );

        assert!(
            plugin_services(&client, redis())
                .await
                .expect("services")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn plugin_services_propagates_list_failure() {
        let client = MockClient::new().stub(
            DokkuCommand::ServiceList { plugin: redis() },
            Err(DokkuError::Exit {
                code: 1,
                stderr: "plugin not installed".into(),
            }),
        );

        assert!(plugin_services(&client, redis()).await.is_err());
    }

    #[tokio::test]
    async fn service_linked_apps_parses_names() {
        let service = ServiceName::try_from("candid").expect("service");
        let client = MockClient::new().stub(
            DokkuCommand::ServiceLinks {
                plugin: redis(),
                service: service.clone(),
            },
            Ok(DokkuOutput::ok("roboswarm-server\n")),
        );

        assert_eq!(
            service_linked_apps(&client, redis(), &service)
                .await
                .expect("links"),
            vec!["roboswarm-server"]
        );
    }

    #[tokio::test]
    async fn service_linked_apps_empty_output_means_no_links() {
        let service = ServiceName::try_from("candid").expect("service");
        let client = MockClient::new().stub(
            DokkuCommand::ServiceLinks {
                plugin: redis(),
                service: service.clone(),
            },
            Ok(DokkuOutput::ok("")),
        );

        assert!(
            service_linked_apps(&client, redis(), &service)
                .await
                .expect("links")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn service_logs_tails_to_the_requested_line_count() {
        let service = ServiceName::try_from("candid").expect("service");
        let client = MockClient::new().stub(
            DokkuCommand::ServiceLogs {
                plugin: redis(),
                service: service.clone(),
                num_lines: 2,
            },
            Ok(DokkuOutput::ok(REDIS_LOGS)),
        );

        let logs = service_logs(&client, redis(), &service, 2)
            .await
            .expect("logs");

        assert_eq!(logs.len(), 2);
        assert_eq!(
            logs.as_slice()[0],
            "1:M 29 Sep 2026 10:00:00.101 # Server initialized"
        );
        assert_eq!(
            logs.as_slice()[1],
            "1:M 29 Sep 2026 10:00:00.102 * Ready to accept connections tcp"
        );
    }

    #[tokio::test]
    async fn service_logs_strips_ansi() {
        let service = ServiceName::try_from("candid").expect("service");
        let client = MockClient::new().stub(
            DokkuCommand::ServiceLogs {
                plugin: redis(),
                service: service.clone(),
                num_lines: 10,
            },
            Ok(DokkuOutput::ok("\u{1b}[32mready\u{1b}[0m\n")),
        );

        let logs = service_logs(&client, redis(), &service, 10)
            .await
            .expect("logs");

        assert_eq!(logs.as_slice(), &["ready"]);
    }
}
