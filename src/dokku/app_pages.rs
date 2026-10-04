use crate::domain::AppName;
use crate::domain::command::DokkuCommand;
use crate::domain::parse::{
    parse_config_show, parse_logs, parse_ps_inspect, parse_ps_scale, parse_resource_report,
    parse_service_info,
};
use crate::domain::types::{
    ContainerDetails, EnvVar, LogLines, ResourceReport, ScaleEntry, ServiceInfo, ServiceLink,
};

use super::client::{DokkuClient, DokkuError};

pub async fn app_config(client: &dyn DokkuClient, app: AppName) -> Result<Vec<EnvVar>, DokkuError> {
    let output = client.exec(&DokkuCommand::ConfigShow { app }).await?;
    Ok(parse_config_show(&output.stdout))
}

pub async fn app_logs(
    client: &dyn DokkuClient,
    app: AppName,
    num_lines: u32,
) -> Result<LogLines, DokkuError> {
    let output = client.exec(&DokkuCommand::Logs { app, num_lines }).await?;
    Ok(parse_logs(&output.stdout))
}

pub async fn app_formation(
    client: &dyn DokkuClient,
    app: AppName,
) -> Result<Vec<ScaleEntry>, DokkuError> {
    let output = client.exec(&DokkuCommand::PsScaleGet { app }).await?;
    Ok(parse_ps_scale(&output.stdout))
}

pub async fn app_containers(
    client: &dyn DokkuClient,
    app: AppName,
) -> Result<Vec<ContainerDetails>, DokkuError> {
    let output = client.exec(&DokkuCommand::PsInspect { app }).await?;
    Ok(parse_ps_inspect(&output.stdout))
}

pub async fn app_resources(
    client: &dyn DokkuClient,
    app: AppName,
) -> Result<Vec<ResourceReport>, DokkuError> {
    let output = client.exec(&DokkuCommand::ResourceReport { app }).await?;
    Ok(parse_resource_report(&output.stdout))
}

/// Fetches the service links for a single app, on demand. `None` means at least
/// one plugin could not be queried (the caller renders an unknown-status card).
pub async fn app_service_links(client: &dyn DokkuClient, app: AppName) -> Option<Vec<ServiceLink>> {
    let plugins = super::snapshot::fetch_service_plugins(client).await;
    super::snapshot::fetch_service_links(client, &app, plugins.as_deref()).await
}

/// Fetches `<plugin>:info <service> --format json`. A response that does not
/// parse yields `None` (the caller renders an unknown-status card).
pub async fn service_info(
    client: &dyn DokkuClient,
    plugin: &str,
    service: &str,
) -> Result<Option<ServiceInfo>, DokkuError> {
    let output = client
        .exec(&DokkuCommand::ServiceInfo {
            plugin: plugin.to_owned(),
            service: service.to_owned(),
        })
        .await?;
    Ok(parse_service_info(&output.stdout, plugin, service))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dokku::{DokkuOutput, MockClient};

    fn app(name: &str) -> AppName {
        AppName::try_from(name).expect("valid app name")
    }

    const CONFIG_SHOW: &str = include_str!("../../tests/fixtures/config_show.txt");
    const LOGS: &str = include_str!("../../tests/fixtures/logs.txt");
    const PS_SCALE: &str = include_str!("../../tests/fixtures/ps_scale.txt");
    const PS_INSPECT: &str = include_str!("../../tests/fixtures/ps_inspect.json");
    const RESOURCE_REPORT: &str = include_str!("../../tests/fixtures/resource_report.txt");
    const REDIS_INFO: &str = include_str!("../../tests/fixtures/redis_info.txt");

    #[tokio::test]
    async fn app_config_parses_fixture_output() {
        let client = MockClient::new().stub(
            DokkuCommand::ConfigShow { app: app("alpha") },
            Ok(DokkuOutput::ok(CONFIG_SHOW)),
        );

        let vars = app_config(&client, app("alpha")).await.expect("config");
        assert_eq!(vars.len(), 3);
        assert_eq!(vars[0].key, "DATABASE_URL");
        assert_eq!(vars[0].value, "postgres://user:pass@host/db");
    }

    #[tokio::test]
    async fn app_config_fetch_error_propagates() {
        let client = MockClient::new().stub(
            DokkuCommand::ConfigShow { app: app("alpha") },
            Err(DokkuError::Exit {
                code: 1,
                stderr: "boom".into(),
            }),
        );

        assert!(matches!(
            app_config(&client, app("alpha")).await,
            Err(DokkuError::Exit { .. })
        ));
    }

    #[tokio::test]
    async fn app_logs_parses_and_strips_ansi() {
        let client = MockClient::new().stub(
            DokkuCommand::Logs {
                app: app("alpha"),
                num_lines: 200,
            },
            Ok(DokkuOutput::ok(LOGS)),
        );

        let logs = app_logs(&client, app("alpha"), 200).await.expect("logs");
        assert_eq!(logs.len(), 3);
        assert!(!logs.as_slice().iter().any(|line| line.contains('\x1b')));
    }

    #[tokio::test]
    async fn app_logs_passes_num_lines_through() {
        let client = MockClient::new().stub(
            DokkuCommand::Logs {
                app: app("alpha"),
                num_lines: 50,
            },
            Ok(DokkuOutput::ok("")),
        );

        app_logs(&client, app("alpha"), 50).await.expect("logs");
        assert!(client.calls().contains(&DokkuCommand::Logs {
            app: app("alpha"),
            num_lines: 50
        }));
    }

    #[tokio::test]
    async fn app_formation_parses_fixture() {
        let client = MockClient::new().stub(
            DokkuCommand::PsScaleGet { app: app("alpha") },
            Ok(DokkuOutput::ok(PS_SCALE)),
        );

        let formation = app_formation(&client, app("alpha"))
            .await
            .expect("formation");
        assert_eq!(formation.len(), 3);
        assert_eq!(formation[1].process_type, "web");
        assert_eq!(formation[1].quantity, 1);
    }

    #[tokio::test]
    async fn app_formation_fetch_error_propagates() {
        let client = MockClient::new().stub(
            DokkuCommand::PsScaleGet { app: app("alpha") },
            Err(DokkuError::Exit {
                code: 1,
                stderr: "boom".into(),
            }),
        );

        assert!(app_formation(&client, app("alpha")).await.is_err());
    }

    #[tokio::test]
    async fn app_containers_parses_fixture() {
        let client = MockClient::new().stub(
            DokkuCommand::PsInspect { app: app("alpha") },
            Ok(DokkuOutput::ok(PS_INSPECT)),
        );

        let containers = app_containers(&client, app("alpha"))
            .await
            .expect("containers");
        assert_eq!(containers.len(), 2);
    }

    #[tokio::test]
    async fn app_resources_parses_fixture() {
        let client = MockClient::new().stub(
            DokkuCommand::ResourceReport { app: app("alpha") },
            Ok(DokkuOutput::ok(RESOURCE_REPORT)),
        );

        let resources = app_resources(&client, app("alpha"))
            .await
            .expect("resources");
        assert_eq!(resources.len(), 2);
    }

    #[tokio::test]
    async fn app_resources_fetch_error_propagates() {
        let client = MockClient::new().stub(
            DokkuCommand::ResourceReport { app: app("alpha") },
            Err(DokkuError::Exit {
                code: 1,
                stderr: "boom".into(),
            }),
        );

        assert!(app_resources(&client, app("alpha")).await.is_err());
    }

    #[tokio::test]
    async fn service_info_parses_fixture() {
        let client = MockClient::new().stub(
            DokkuCommand::ServiceInfo {
                plugin: "redis".into(),
                service: "candid".into(),
            },
            Ok(DokkuOutput::ok(REDIS_INFO)),
        );

        let info = service_info(&client, "redis", "candid")
            .await
            .expect("fetch")
            .expect("parsed");
        assert_eq!(info.service, "candid");
        assert_eq!(info.status, "running");
        assert_eq!(info.version, "redis:7.2.4");
        assert_eq!(info.linked_apps, vec!["candid"]);
    }

    #[tokio::test]
    async fn service_info_unparseable_yields_none() {
        let client = MockClient::new().stub(
            DokkuCommand::ServiceInfo {
                plugin: "redis".into(),
                service: "cache".into(),
            },
            Ok(DokkuOutput::ok("nonsense")),
        );

        assert_eq!(
            service_info(&client, "redis", "cache")
                .await
                .expect("fetch"),
            None
        );
    }
}
