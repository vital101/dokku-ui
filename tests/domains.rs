mod common;

use std::collections::HashMap;
use std::sync::Arc;

use actix_web::http::StatusCode;
use actix_web::test;

use common::{
    complete_setup, extract_csrf, form_request, get_body, location, run_url, seed_user, test_state,
};

use dokku_ui::dokku::{DokkuClient, DokkuOutput, FakeResolver, MockClient, SnapshotStore};
use dokku_ui::domain::AppName;
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::settings::Settings;
use dokku_ui::storage;
use dokku_ui::storage::jobs::SqliteJobsRepo;
use dokku_ui::storage::runs::SqliteRunsRepo;
use dokku_ui::web::{AppState, build_app};

fn app_name(name: &str) -> AppName {
    AppName::try_from(name).expect("valid app name")
}

const DOMAINS_REPORT: &str = r#"{"app-enabled":"true","app-vhosts":"alpha.example.com beta.example.com","global-enabled":"true","global-vhosts":"example.com"}"#;

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
            DokkuCommand::DomainsReport {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok(DOMAINS_REPORT)),
        )
}

async fn harness(client: MockClient) -> (AppState, Arc<MockClient>, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let database_url = format!("sqlite://{}/test.db", dir.path().display());
    let pool = storage::connect(&database_url).await.expect("connect db");
    let settings = Settings::from_map(&HashMap::new()).expect("default settings");
    let client_arc = Arc::new(client);
    let dokku: Arc<dyn DokkuClient> = client_arc.clone();
    let snapshot = Arc::new(SnapshotStore::with_resolver(
        dokku.clone(),
        Arc::new(FakeResolver::all()),
        pool.clone(),
    ));
    (
        AppState {
            action_runs: Arc::new(SqliteRunsRepo::new(pool.clone())),
            jobs: Arc::new(SqliteJobsRepo::new(pool.clone())),
            capabilities: Arc::new(dokku_ui::dokku::CapabilitiesStore::new(
                dokku.clone(),
                pool.clone(),
            )),
            db: pool,
            settings,
            dokku,
            snapshot,
        },
        client_arc,
        dir,
    )
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
            .uri("/apps/alpha/domains")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    extract_csrf(&get_body(resp).await)
}

#[tokio::test]
async fn domains_routes_require_login() {
    let (state, _dir) = test_state().await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;

    for path in ["/apps/alpha/domains", "/apps/alpha/partials/domains"] {
        let resp = test::call_service(&app, test::TestRequest::get().uri(path).to_request()).await;
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT, "{path}");
        assert_eq!(location(&resp), "/login", "{path}");
    }
}

#[tokio::test]
async fn domains_partial_renders_vhosts_with_dns_badges() {
    let (state, _client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/domains")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("alpha.example.com"), "{body}");
    assert!(body.contains("beta.example.com"), "{body}");
    assert!(body.contains("resolves"), "{body}");
    assert!(
        body.contains(r#"hx-post="/apps/alpha/domains/remove""#),
        "{body}"
    );
    assert!(body.contains("example.com"), "global vhosts shown: {body}");
    assert!(
        body.contains(r#"value="alpha.example.com beta.example.com""#),
        "set form prefilled: {body}"
    );
}

#[tokio::test]
async fn hx_domains_add_streams_and_runs() {
    let client = seeded_app_client().stub(
        DokkuCommand::DomainsAdd {
            app: app_name("alpha"),
            domains: vec![dokku_ui::domain::DomainName::try_from("new.example.com").expect("d")],
        },
        Ok(DokkuOutput::ok("-----> Added new.example.com\n")),
    );
    let (state, client, _dir) = harness(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/domains/add",
            format!("csrf_token={csrf}&domains=new.example.com"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("Updating domains for alpha"), "{body}");
    assert!(
        body.contains(r#"data-refresh="/apps/alpha/partials/domains""#),
        "{body}"
    );

    let (_, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(events.contains(r#""ok":true"#), "{events}");
    assert!(
        client.calls().contains(&DokkuCommand::DomainsAdd {
            app: app_name("alpha"),
            domains: vec![dokku_ui::domain::DomainName::try_from("new.example.com").expect("d")],
        }),
        "add ran"
    );
}

#[tokio::test]
async fn hx_domains_remove_and_set_stream_and_run() {
    let client = seeded_app_client()
        .stub(
            DokkuCommand::DomainsRemove {
                app: app_name("alpha"),
                domains: vec![
                    dokku_ui::domain::DomainName::try_from("beta.example.com").expect("d"),
                ],
            },
            Ok(DokkuOutput::ok("")),
        )
        .stub(
            DokkuCommand::DomainsSet {
                app: app_name("alpha"),
                domains: vec![
                    dokku_ui::domain::DomainName::try_from("only.example.com").expect("d"),
                ],
            },
            Ok(DokkuOutput::ok("")),
        );
    let (state, client, _dir) = harness(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/domains/remove",
            format!("csrf_token={csrf}&domain=beta.example.com"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    let (_, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(events.contains(r#""ok":true"#), "{events}");

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/domains/set",
            format!("csrf_token={csrf}&domains=only.example.com"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    let (_, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(events.contains(r#""ok":true"#), "{events}");

    assert!(
        client.calls().contains(&DokkuCommand::DomainsRemove {
            app: app_name("alpha"),
            domains: vec![dokku_ui::domain::DomainName::try_from("beta.example.com").expect("d")],
        }),
        "remove ran"
    );
    assert!(
        client.calls().contains(&DokkuCommand::DomainsSet {
            app: app_name("alpha"),
            domains: vec![dokku_ui::domain::DomainName::try_from("only.example.com").expect("d")],
        }),
        "set ran"
    );
}

#[tokio::test]
async fn domains_forms_reject_invalid_and_empty_input_without_dokku() {
    let (state, client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    for (path, body, expected) in [
        (
            "/apps/alpha/domains/add",
            "domains=bad_domain",
            "Invalid domain",
        ),
        ("/apps/alpha/domains/set", "domains=", "at least one domain"),
    ] {
        let resp = test::call_service(
            &app,
            hx_form_request(path, format!("csrf_token={csrf}&{body}"))
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK, "modal errors stay 200");
        let html = get_body(resp).await;
        assert!(html.contains(expected), "{path}: {html}");
    }
    assert!(
        client.calls().iter().all(|call| !matches!(
            call,
            DokkuCommand::DomainsAdd { .. }
                | DokkuCommand::DomainsRemove { .. }
                | DokkuCommand::DomainsSet { .. }
        )),
        "no domain commands for rejected input"
    );
}

#[tokio::test]
async fn non_htmx_domains_add_queues_flashes_and_is_audited() {
    let client = seeded_app_client().stub(
        DokkuCommand::DomainsAdd {
            app: app_name("alpha"),
            domains: vec![dokku_ui::domain::DomainName::try_from("new.example.com").expect("d")],
        },
        Ok(DokkuOutput::ok("")),
    );
    let (state, client, _dir) = harness(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        form_request(
            "/apps/alpha/domains/add",
            format!("csrf_token={csrf}&domains=new.example.com"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/alpha/domains");
    let cookie = response_cookie_or(cookie, &resp);
    wait_for_call(
        &client,
        &DokkuCommand::DomainsAdd {
            app: app_name("alpha"),
            domains: vec![dokku_ui::domain::DomainName::try_from("new.example.com").expect("d")],
        },
    )
    .await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/domains")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Queued: add domains for alpha."), "{body}");

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/activity")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("domains.add"), "{body}");
}

fn response_cookie_or<B>(
    fallback: actix_web::cookie::Cookie<'static>,
    resp: &actix_web::dev::ServiceResponse<B>,
) -> actix_web::cookie::Cookie<'static> {
    common::response_cookie(resp).unwrap_or(fallback)
}
