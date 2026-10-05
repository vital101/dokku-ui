mod common;

use actix_web::http::StatusCode;
use actix_web::test;

use common::{
    complete_setup, get_body, location, seed_user, test_state, test_state_with_shared_client,
};

use dokku_ui::dokku::{DokkuOutput, MockClient};
use dokku_ui::domain::AppName;
use dokku_ui::domain::capabilities::CapabilityFamily;
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::web::build_app;

fn app_name(name: &str) -> AppName {
    AppName::try_from(name).expect("valid app name")
}

const DOKKU_VERSION: &str = include_str!("fixtures/dokku_version.txt");
const LOGS_HELP: &str = include_str!("fixtures/logs_help.txt");

fn live_logs_client() -> MockClient {
    MockClient::new()
        .stub(
            DokkuCommand::Logs {
                app: app_name("alpha"),
                num_lines: 200,
                follow: true,
            },
            Ok(DokkuOutput::ok("2026-01-01T00:00:00Z app[web.1]: booting\n2026-01-01T00:00:01Z app[web.1]: ready\n")),
        )
        .stub(
            DokkuCommand::Logs {
                app: app_name("alpha"),
                num_lines: 200,
                follow: false,
            },
            Ok(DokkuOutput::ok("2026-01-01T00:00:00Z app[web.1]: booting\n")),
        )
        .stub(
            DokkuCommand::DokkuVersion,
            Ok(DokkuOutput::ok(DOKKU_VERSION)),
        )
        .stub(
            DokkuCommand::PluginList,
            Ok(DokkuOutput::ok(
                "=====> Plugins\n  apps 0.38.4 enabled dokku core apps plugin\n  logs 0.38.4 enabled dokku core logs plugin\n  nginx-vhosts 0.38.4 enabled dokku core nginx-vhosts plugin\n",
            )),
        )
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
            Err(dokku_ui::dokku::DokkuError::Exit {
                code: 1,
                stderr: "unknown command".into(),
            }),
        )
        .stub(
            DokkuCommand::Help {
                family: CapabilityFamily::NginxErrorLogs,
            },
            Err(dokku_ui::dokku::DokkuError::Exit {
                code: 1,
                stderr: "unknown command".into(),
            }),
        )
}

#[tokio::test]
async fn log_stream_requires_login() {
    let (state, _dir) = test_state().await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/logs/stream?source=logs&tail=1")
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(location(&resp), "/login");
}

#[tokio::test]
async fn log_stream_follows_app_logs_over_sse() {
    let (state, client, _dir) = test_state_with_shared_client(live_logs_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/logs/stream?source=logs&tail=1")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap_or("")),
        Some("text/event-stream")
    );
    let body = get_body(resp).await;

    assert!(
        body.contains("event: line\ndata: 2026-01-01T00:00:00Z app[web.1]: booting"),
        "{body}"
    );
    assert!(
        body.contains("event: line\ndata: 2026-01-01T00:00:01Z app[web.1]: ready"),
        "{body}"
    );
    assert!(
        client.calls().contains(&DokkuCommand::Logs {
            app: app_name("alpha"),
            num_lines: 200,
            follow: true,
        }),
        "the follow command ran"
    );
}

#[tokio::test]
async fn log_stream_gates_unsupported_sources_with_an_explanatory_event() {
    let (state, _client, _dir) = test_state_with_shared_client(live_logs_client()).await;
    state.capabilities.ensure_loaded().await.expect("probe");
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/logs/stream?source=nginx:access-logs&tail=1")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "SSE errors stay 200");
    let body = get_body(resp).await;
    assert!(body.starts_with("event: error\ndata: "), "{body}");
    assert!(body.contains("nginx:access-logs"), "{body}");
}

#[tokio::test]
async fn log_stream_without_probe_data_serves_without_gating() {
    let (state, client, _dir) = test_state_with_shared_client(live_logs_client()).await;
    // No capabilities row: the gate is skipped and the stream just runs.
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/logs/stream?source=logs&tail=1")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("booting"), "{body}");
    assert!(client.calls().contains(&DokkuCommand::Logs {
        app: app_name("alpha"),
        num_lines: 200,
        follow: true,
    }));
}

#[tokio::test]
async fn log_stream_tail_zero_is_a_bounded_snapshot() {
    let (state, client, _dir) = test_state_with_shared_client(live_logs_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/logs/stream?source=logs&tail=0")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("booting"), "{body}");
    assert!(client.calls().contains(&DokkuCommand::Logs {
        app: app_name("alpha"),
        num_lines: 200,
        follow: false,
    }));
}
