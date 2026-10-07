mod common;

use std::sync::Arc;

use actix_web::http::StatusCode;
use actix_web::test;

use common::{
    complete_setup, extract_csrf, form_request, get_body, login, run_url, seed_user,
    seed_user_with_role, test_state_with_shared_client,
};

use dokku_ui::auth::rbac::Role;
use dokku_ui::dokku::{DokkuOutput, MockClient};
use dokku_ui::domain::AppName;
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::web::build_app;

const PORTS_REPORT: &str = include_str!("fixtures/ports_report.txt");
const PROXY_REPORT: &str = include_str!("fixtures/proxy_report.txt");

fn app_name(name: &str) -> AppName {
    AppName::try_from(name).expect("valid app name")
}

fn seeded_app_client() -> MockClient {
    MockClient::new()
        .stub(
            DokkuCommand::AppsList,
            Ok(DokkuOutput::ok("=====> My Apps\nalpha")),
        )
        .stub(
            DokkuCommand::AppsReport {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok(
                r#"{"app-created-at": "1791023796", "app-locked": "false"}"#,
            )),
        )
        .stub(
            DokkuCommand::PsReport {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok(
                r#"{"deployed": "true", "running": "true", "processes": "1"}"#,
            )),
        )
        .stub(
            DokkuCommand::PortsReport {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok(PORTS_REPORT)),
        )
        .stub(
            DokkuCommand::ProxyReport {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok(PROXY_REPORT)),
        )
}

fn hx_form_request(path: &str, body: String) -> actix_web::test::TestRequest {
    form_request(path, body).insert_header(("HX-Request", "true"))
}

async fn wait_for_call(client: &Arc<MockClient>, command: &DokkuCommand) {
    for _ in 0..200 {
        if client.calls().contains(command) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("command never called: {command:?}");
}

async fn shell_csrf<B, E>(
    app: &impl actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse<B>,
        Error = E,
    >,
    cookie: &actix_web::cookie::Cookie<'static>,
) -> String
where
    B: actix_web::body::MessageBody,
    E: std::fmt::Debug,
{
    let resp = test::call_service(
        app,
        test::TestRequest::get()
            .uri("/apps/alpha/ports")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    extract_csrf(&get_body(resp).await)
}

#[tokio::test]
async fn ports_partial_renders_mappings_and_proxy_status() {
    let (state, _client, _dir) = test_state_with_shared_client(seeded_app_client()).await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;
    let cookie = login(&app, "admin@example.com", "correct-horse-battery").await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/ports")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("http:80:5000"), "{body}");
    assert!(body.contains("Proxy enabled"), "{body}");
    assert!(body.contains("nginx"), "{body}");
    assert!(body.contains("Add mappings"), "{body}");
}

#[tokio::test]
async fn ports_add_and_set_enqueue_validated_runs() {
    let (state, client, _dir) = test_state_with_shared_client(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/ports/add",
            format!("csrf_token={csrf}&mappings=http%3A8080%3A5000+tcp%3A5432%3A5432"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(run_url(&body).starts_with("/actions/runs/"), "{body}");
    wait_for_call(
        &client,
        &DokkuCommand::PortsAdd {
            app: app_name("alpha"),
            mappings: vec!["http:8080:5000".into(), "tcp:5432:5432".into()],
        },
    )
    .await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/ports/set",
            format!("csrf_token={csrf}&mappings=8080"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    wait_for_call(
        &client,
        &DokkuCommand::PortsSet {
            app: app_name("alpha"),
            mappings: vec!["8080".into()],
        },
    )
    .await;

    // Invalid mappings are rejected before any command is built.
    let calls_before = client.calls().len();
    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/ports/add",
            format!("csrf_token={csrf}&mappings=http%3A80%3A5000+extra"),
        )
        .cookie(cookie)
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "htmx errors stay 200");
    let body = get_body(resp).await;
    assert!(body.contains("data-modal-error"), "{body}");
    assert_eq!(client.calls().len(), calls_before, "nothing ran");
}

#[tokio::test]
async fn ports_remove_and_clear_enqueue_runs() {
    let (state, client, _dir) = test_state_with_shared_client(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/ports/remove",
            format!("csrf_token={csrf}&mapping=http%3A80%3A5000"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    wait_for_call(
        &client,
        &DokkuCommand::PortsRemove {
            app: app_name("alpha"),
            mappings: vec!["http:80:5000".into()],
        },
    )
    .await;

    let resp = test::call_service(
        &app,
        hx_form_request("/apps/alpha/ports/clear", format!("csrf_token={csrf}"))
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    wait_for_call(
        &client,
        &DokkuCommand::PortsClear {
            app: app_name("alpha"),
        },
    )
    .await;
}

#[tokio::test]
async fn viewers_do_not_see_ports_mutation_controls() {
    let (state, _client, _dir) = test_state_with_shared_client(seeded_app_client()).await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    seed_user_with_role(
        &state,
        "viewer@example.com",
        "correct-horse-battery",
        Role::Viewer,
    )
    .await;
    let app = test::init_service(build_app(state)).await;

    let viewer = login(&app, "viewer@example.com", "correct-horse-battery").await;
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/ports")
            .cookie(viewer)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("http:80:5000"), "read-only view remains");
    assert!(!body.contains("Add mappings"), "{body}");
    assert!(!body.contains("Clear all mappings"), "{body}");
    assert!(!body.contains("ports/remove"), "{body}");
}

#[tokio::test]
async fn ports_requires_login() {
    let (state, _client, _dir) = test_state_with_shared_client(seeded_app_client()).await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;

    for path in ["/apps/alpha/ports", "/apps/alpha/partials/ports"] {
        let resp = test::call_service(&app, test::TestRequest::get().uri(path).to_request()).await;
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT, "{path}");
    }
}
