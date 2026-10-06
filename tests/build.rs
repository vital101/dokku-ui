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
use dokku_ui::storage::webhooks::SqliteWebhooksRepo;
use dokku_ui::web::{AppState, build_app};

fn app_name(name: &str) -> AppName {
    AppName::try_from(name).expect("valid app name")
}

const BUILDPACKS: &str = "-----> alpha buildpack urls\nhttps://github.com/heroku/heroku-buildpack-nodejs\nhttps://github.com/example/custom-bp\n";
const BUILDER_REPORT: &str = "=====> alpha builder information\n       Builder build dir:             \n       Builder computed build dir:    \n       Builder computed selected:     dockerfile\n       Builder detected:              dockerfile\n       Builder global build dir:      \n       Builder global selected:       \n       Builder selected:              dockerfile\n";

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
            DokkuCommand::BuildpacksList {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok(BUILDPACKS)),
        )
        .stub(
            DokkuCommand::BuilderReport {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok(BUILDER_REPORT)),
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
            webhooks: Arc::new(SqliteWebhooksRepo::new(pool.clone())),
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
            .uri("/apps/alpha/build")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    extract_csrf(&get_body(resp).await)
}

#[tokio::test]
async fn build_routes_require_login() {
    let (state, _dir) = test_state().await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;

    for path in ["/apps/alpha/build", "/apps/alpha/partials/build"] {
        let resp = test::call_service(&app, test::TestRequest::get().uri(path).to_request()).await;
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT, "{path}");
        assert_eq!(location(&resp), "/login", "{path}");
    }
}

#[tokio::test]
async fn build_partial_renders_buildpacks_and_builder() {
    let (state, _client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/build")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("heroku-buildpack-nodejs"), "{body}");
    assert!(body.contains("custom-bp"), "{body}");
    assert!(
        body.contains(r#"hx-post="/apps/alpha/build/buildpacks/remove""#),
        "{body}"
    );
    assert!(
        body.contains(r#"hx-post="/apps/alpha/build/builder""#),
        "{body}"
    );
    assert!(body.contains("dockerfile"), "builder state shown: {body}");
    assert!(body.contains("Builder"), "{body}");
}

#[tokio::test]
async fn hx_buildpacks_add_streams_with_the_position() {
    let client = seeded_app_client().stub(
        DokkuCommand::BuildpacksAdd {
            app: app_name("alpha"),
            buildpack: "https://example.com/bp".into(),
            index: Some(2),
        },
        Ok(DokkuOutput::ok("-----> Added buildpack\n")),
    );
    let (state, client, _dir) = harness(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/build/buildpacks/add",
            format!("csrf_token={csrf}&buildpack=https%3A%2F%2Fexample.com%2Fbp&index=2"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("Adding buildpack to alpha"), "{body}");
    assert!(
        body.contains(r#"data-refresh="/apps/alpha/partials/build""#),
        "{body}"
    );

    let (_, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(events.contains(r#""ok":true"#), "{events}");
    assert!(
        client.calls().contains(&DokkuCommand::BuildpacksAdd {
            app: app_name("alpha"),
            buildpack: "https://example.com/bp".into(),
            index: Some(2),
        }),
        "add ran with the position"
    );
}

#[tokio::test]
async fn hx_buildpacks_set_remove_and_clear_stream() {
    let client = seeded_app_client()
        .stub(
            DokkuCommand::BuildpacksSet {
                app: app_name("alpha"),
                buildpack: "https://example.com/first".into(),
                index: None,
            },
            Ok(DokkuOutput::ok("")),
        )
        .stub(
            DokkuCommand::BuildpacksRemove {
                app: app_name("alpha"),
                buildpack: "https://github.com/example/custom-bp".into(),
            },
            Ok(DokkuOutput::ok("")),
        )
        .stub(
            DokkuCommand::BuildpacksClear {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok("")),
        );
    let (state, client, _dir) = harness(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    for (path, body) in [
        (
            "/apps/alpha/build/buildpacks/set",
            format!("csrf_token={csrf}&buildpack=https%3A%2F%2Fexample.com%2Ffirst"),
        ),
        (
            "/apps/alpha/build/buildpacks/remove",
            format!("csrf_token={csrf}&buildpack=https%3A%2F%2Fgithub.com%2Fexample%2Fcustom-bp"),
        ),
        (
            "/apps/alpha/build/buildpacks/clear",
            format!("csrf_token={csrf}"),
        ),
    ] {
        let resp = test::call_service(
            &app,
            hx_form_request(path, body)
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        let html = get_body(resp).await;
        let (_, events) = sse_events(&app, &run_url(&html), &cookie).await;
        assert!(events.contains(r#""ok":true"#), "{path}: {events}");
    }
    assert!(
        client.calls().contains(&DokkuCommand::BuildpacksClear {
            app: app_name("alpha"),
        }),
        "clear ran"
    );
}

#[tokio::test]
async fn buildpacks_forms_reject_invalid_input_without_dokku() {
    let (state, client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    for (body, expected) in [
        (
            format!("csrf_token={csrf}&buildpack=it%27s"),
            "without single quotes",
        ),
        (
            format!("csrf_token={csrf}&buildpack=https%3A%2F%2Fexample.com%2Fbp&index=0"),
            "between 1 and 1000",
        ),
        (
            format!("csrf_token={csrf}&buildpack=https%3A%2F%2Fexample.com%2Fbp&index=abc"),
            "must be a number",
        ),
    ] {
        let resp = test::call_service(
            &app,
            hx_form_request("/apps/alpha/build/buildpacks/add", body)
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let html = get_body(resp).await;
        assert!(html.contains(expected), "{html}");
    }
    assert!(
        client.calls().iter().all(|call| !matches!(
            call,
            DokkuCommand::BuildpacksAdd { .. } | DokkuCommand::BuildpacksSet { .. }
        )),
        "no buildpack commands for rejected input"
    );
}

#[tokio::test]
async fn hx_builder_set_streams_and_validates() {
    let client = seeded_app_client()
        .stub(
            DokkuCommand::BuilderSet {
                app: app_name("alpha"),
                property: "selected".into(),
                value: Some("herokuish".into()),
            },
            Ok(DokkuOutput::ok("")),
        )
        .stub(
            DokkuCommand::BuilderSet {
                app: app_name("alpha"),
                property: "build-dir".into(),
                value: Some("/app/sub".into()),
            },
            Ok(DokkuOutput::ok("")),
        );
    let (state, client, _dir) = harness(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    for (property, value) in [("selected", "herokuish"), ("build-dir", "%2Fapp%2Fsub")] {
        let resp = test::call_service(
            &app,
            hx_form_request(
                "/apps/alpha/build/builder",
                format!("csrf_token={csrf}&property={property}&value={value}"),
            )
            .cookie(cookie.clone())
            .to_request(),
        )
        .await;
        let html = get_body(resp).await;
        let (_, events) = sse_events(&app, &run_url(&html), &cookie).await;
        assert!(events.contains(r#""ok":true"#), "{property}: {events}");
    }

    // Unknown builders and read-only properties are rejected.
    for (property, value, expected) in [
        ("selected", "podman", "Invalid builder value"),
        ("detected", "dockerfile", "Unknown builder property"),
    ] {
        let resp = test::call_service(
            &app,
            hx_form_request(
                "/apps/alpha/build/builder",
                format!("csrf_token={csrf}&property={property}&value={value}"),
            )
            .cookie(cookie.clone())
            .to_request(),
        )
        .await;
        let html = get_body(resp).await;
        assert!(html.contains(expected), "{property}: {html}");
    }

    assert!(
        client.calls().contains(&DokkuCommand::BuilderSet {
            app: app_name("alpha"),
            property: "selected".into(),
            value: Some("herokuish".into()),
        }),
        "builder selection ran"
    );
}

#[tokio::test]
async fn non_htmx_buildpacks_add_queues_flashes_and_is_audited() {
    let client = seeded_app_client().stub(
        DokkuCommand::BuildpacksAdd {
            app: app_name("alpha"),
            buildpack: "https://example.com/bp".into(),
            index: None,
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
            "/apps/alpha/build/buildpacks/add",
            format!("csrf_token={csrf}&buildpack=https%3A%2F%2Fexample.com%2Fbp"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/alpha/build");
    let cookie = common::response_cookie(&resp).unwrap_or(cookie);
    wait_for_call(
        &client,
        &DokkuCommand::BuildpacksAdd {
            app: app_name("alpha"),
            buildpack: "https://example.com/bp".into(),
            index: None,
        },
    )
    .await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/build")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Queued: add buildpack to alpha."), "{body}");

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/activity")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("buildpacks.add"), "{body}");
}
