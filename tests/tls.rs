mod common;

use std::sync::Arc;

use actix_web::http::StatusCode;
use actix_web::test;

use common::{
    complete_setup, extract_csrf, form_request, get_body, location, run_url,
    test_state_with_shared_client,
};

use dokku_ui::dokku::{DokkuOutput, MockClient};
use dokku_ui::domain::AppName;
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::web::{AppState, build_app};

const LETSENCRYPT_LIST: &str = include_str!("fixtures/letsencrypt_list.txt");
const LETSENCRYPT_ACTIVE_TRUE: &str = include_str!("fixtures/letsencrypt_active_true.txt");
const CERTS_REPORT_APP: &str = include_str!("fixtures/certs_report_app.txt");
const PLUGIN_LIST: &str = include_str!("fixtures/plugin_list.txt");

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
            DokkuCommand::CertsReport {
                app: Some(app_name("alpha")),
            },
            Ok(DokkuOutput::ok(CERTS_REPORT_APP)),
        )
        .stub(
            DokkuCommand::LetsencryptActive {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok(LETSENCRYPT_ACTIVE_TRUE)),
        )
        .stub(
            DokkuCommand::LetsencryptList,
            Ok(DokkuOutput::ok(LETSENCRYPT_LIST)),
        )
}

fn with_capabilities(client: MockClient) -> MockClient {
    client
        .stub(
            DokkuCommand::DokkuVersion,
            Ok(DokkuOutput::ok("dokku version 0.38.4\n")),
        )
        .stub(DokkuCommand::PluginList, Ok(DokkuOutput::ok(PLUGIN_LIST)))
}

async fn harness(client: MockClient) -> (AppState, Arc<MockClient>, tempfile::TempDir) {
    test_state_with_shared_client(client).await
}

fn hx_form_request(path: &str, body: String) -> actix_web::test::TestRequest {
    form_request(path, body).insert_header(("HX-Request", "true"))
}

async fn sse_events<B, E>(
    app: &impl actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse<B>,
        Error = E,
    >,
    uri: &str,
    cookie: &actix_web::cookie::Cookie<'static>,
) -> (StatusCode, String)
where
    B: actix_web::body::MessageBody,
    E: std::fmt::Debug,
{
    let resp = test::call_service(
        app,
        test::TestRequest::get()
            .uri(uri)
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let status = resp.status();
    (status, get_body(resp).await)
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
            .uri("/apps/alpha/tls")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    extract_csrf(&get_body(resp).await)
}

#[tokio::test]
async fn tls_routes_require_login() {
    let (state, _client, _dir) = test_state_with_shared_client(seeded_app_client()).await;
    common::seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;

    for path in ["/apps/alpha/tls", "/apps/alpha/partials/tls"] {
        let resp = test::call_service(&app, test::TestRequest::get().uri(path).to_request()).await;
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT, "{path}");
        assert_eq!(location(&resp), "/login", "{path}");
    }
}

#[tokio::test]
async fn tls_partial_renders_letsencrypt_and_cert_status() {
    let client = with_capabilities(seeded_app_client()).stub(
        DokkuCommand::LetsencryptList,
        Ok(DokkuOutput::ok(
            "-----> App name           Certificate Expiry        Time before expiry        Time before renewal      \n\
             alpha                    2027-01-01 11:30:37       87d, 1h, 8m, 10s           57d, 1h, 8m, 10s         \n",
        )),
    );
    let (state, _client, _dir) = harness(client).await;
    state
        .capabilities
        .ensure_loaded()
        .await
        .expect("capabilities");
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/tls")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("Active"), "status badge: {body}");
    assert!(body.contains("2027-01-01 11:30:37"), "expiry row: {body}");
    assert!(body.contains("Renews in"), "{body}");
    assert!(
        body.contains(r#"hx-post="/apps/alpha/tls/disable""#),
        "active app offers disable: {body}"
    );
    assert!(
        body.contains(r#"hx-post="/apps/alpha/tls/cron-job/add""#),
        "server-wide cron card: {body}"
    );
    assert!(
        body.contains("dokku.re-cycledair.com"),
        "certificate hostnames: {body}"
    );
    assert!(body.contains("Let's Encrypt"), "issuer shown: {body}");
}

#[tokio::test]
async fn tls_partial_explains_a_missing_plugin() {
    let client = seeded_app_client().stub(
        DokkuCommand::PluginList,
        Ok(DokkuOutput::ok(
            "  git 0.38.4 enabled dokku core git plugin\n",
        )),
    );
    let client = client.stub(
        DokkuCommand::DokkuVersion,
        Ok(DokkuOutput::ok("dokku version 0.38.4\n")),
    );
    let (state, client, _dir) = harness(client).await;
    state
        .capabilities
        .ensure_loaded()
        .await
        .expect("capabilities");
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/tls")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("requires the `letsencrypt` plugin"), "{body}");
    assert!(
        !body.contains(r#"hx-post="/apps/alpha/tls/enable""#),
        "no enable form when the plugin is missing: {body}"
    );
    assert!(
        client
            .calls()
            .iter()
            .all(|call| !matches!(call, DokkuCommand::LetsencryptActive { .. })),
        "LE commands are not run when the plugin is missing"
    );
    assert!(
        body.contains("dokku.re-cycledair.com"),
        "core certificate status still renders: {body}"
    );
}

#[tokio::test]
async fn hx_letsencrypt_actions_stream_with_audit() {
    let client = with_capabilities(seeded_app_client())
        .stub(
            DokkuCommand::LetsencryptEnable {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok("-----> Enabling letsencrypt\n")),
        )
        .stub(
            DokkuCommand::LetsencryptRevoke {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok("")),
        );
    let (state, _client, _dir) = harness(client).await;
    state
        .capabilities
        .ensure_loaded()
        .await
        .expect("capabilities");
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    for path in ["/apps/alpha/tls/enable", "/apps/alpha/tls/revoke"] {
        let resp = test::call_service(
            &app,
            hx_form_request(path, format!("csrf_token={csrf}"))
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let html = get_body(resp).await;
        assert!(
            html.contains(r#"data-refresh="/apps/alpha/partials/tls""#),
            "{path}: {html}"
        );
        let (_, events) = sse_events(&app, &run_url(&html), &cookie).await;
        assert!(events.contains(r#""ok":true"#), "{path}: {events}");
    }
}

#[tokio::test]
async fn hx_letsencrypt_set_streams_and_rejects_bad_input() {
    let client = with_capabilities(seeded_app_client())
        .stub(
            DokkuCommand::LetsencryptSet {
                app: app_name("alpha"),
                property: "email".into(),
                value: Some("ops@example.com".into()),
            },
            Ok(DokkuOutput::ok("")),
        )
        .stub(
            DokkuCommand::LetsencryptSet {
                app: app_name("alpha"),
                property: "staging".into(),
                value: Some("true".into()),
            },
            Ok(DokkuOutput::ok("")),
        );
    let (state, client, _dir) = harness(client).await;
    state
        .capabilities
        .ensure_loaded()
        .await
        .expect("capabilities");
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    for (body, expected) in [
        (
            format!("csrf_token={csrf}&email=not-an-email"),
            "valid email",
        ),
        (
            format!("csrf_token={csrf}&email=o%27brien%40example.com"),
            "valid email",
        ),
        (
            format!("csrf_token={csrf}&email=&staging="),
            "Provide an email",
        ),
        (
            format!("csrf_token={csrf}&email=&staging=yes"),
            "valid staging",
        ),
    ] {
        let resp = test::call_service(
            &app,
            hx_form_request("/apps/alpha/tls/set", body)
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let html = get_body(resp).await;
        assert!(html.contains(expected), "{expected}: {html}");
    }
    assert!(
        client
            .calls()
            .iter()
            .all(|call| !matches!(call, DokkuCommand::LetsencryptSet { .. })),
        "invalid input never reaches dokku"
    );

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/tls/set",
            format!("csrf_token={csrf}&email=ops%40example.com&staging=true"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let html = get_body(resp).await;
    assert!(
        html.contains(r#"data-refresh="/apps/alpha/partials/tls""#),
        "{html}"
    );
    let (_, events) = sse_events(&app, &run_url(&html), &cookie).await;
    assert!(events.contains(r#""ok":true"#), "{events}");
    assert!(
        client.calls().contains(&DokkuCommand::LetsencryptSet {
            app: app_name("alpha"),
            property: "email".into(),
            value: Some("ops@example.com".into()),
        }) && client.calls().contains(&DokkuCommand::LetsencryptSet {
            app: app_name("alpha"),
            property: "staging".into(),
            value: Some("true".into()),
        }),
        "both properties reach dokku"
    );
}

#[tokio::test]
async fn hx_letsencrypt_set_rejects_invalid_app_name() {
    let client = with_capabilities(seeded_app_client());
    let (state, client, _dir) = harness(client).await;
    state
        .capabilities
        .ensure_loaded()
        .await
        .expect("capabilities");
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/Bad_App/tls/set",
            format!("csrf_token={csrf}&staging=true"),
        )
        .cookie(cookie)
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("Invalid app name"), "{body}");
    assert!(
        client
            .calls()
            .iter()
            .all(|call| !matches!(call, DokkuCommand::LetsencryptSet { .. })),
        "an invalid app name never reaches dokku"
    );
}

#[tokio::test]
async fn hx_cron_job_add_and_remove_stream() {
    let client = with_capabilities(seeded_app_client())
        .stub(
            DokkuCommand::LetsencryptCronJob { add: true },
            Ok(DokkuOutput::ok("")),
        )
        .stub(
            DokkuCommand::LetsencryptCronJob { add: false },
            Ok(DokkuOutput::ok("")),
        );
    let (state, _client, _dir) = harness(client).await;
    state
        .capabilities
        .ensure_loaded()
        .await
        .expect("capabilities");
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    for path in [
        "/apps/alpha/tls/cron-job/add",
        "/apps/alpha/tls/cron-job/remove",
    ] {
        let resp = test::call_service(
            &app,
            hx_form_request(path, format!("csrf_token={csrf}"))
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let html = get_body(resp).await;
        let (_, events) = sse_events(&app, &run_url(&html), &cookie).await;
        assert!(events.contains(r#""ok":true"#), "{path}: {events}");
    }
}

#[tokio::test]
async fn tls_actions_reject_when_plugin_missing() {
    let client = seeded_app_client()
        .stub(
            DokkuCommand::DokkuVersion,
            Ok(DokkuOutput::ok("dokku version 0.38.4\n")),
        )
        .stub(
            DokkuCommand::PluginList,
            Ok(DokkuOutput::ok(
                "  git 0.38.4 enabled dokku core git plugin\n",
            )),
        );
    let (state, _client, _dir) = harness(client).await;
    state
        .capabilities
        .ensure_loaded()
        .await
        .expect("capabilities");
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request("/apps/alpha/tls/enable", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("not available on this host"), "{body}");

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/tls/set",
            format!("csrf_token={csrf}&staging=true"),
        )
        .cookie(cookie)
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("not available on this host"), "{body}");
}
