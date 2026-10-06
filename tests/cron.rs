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

const CRON_JSON: &str = r#"[
  {"id":"a1b2c3","schedule":"0 * * * *","command":"echo hourly","concurrency_policy":"allow","maintenance":false,"task-in-maintenance":false},
  {"id":"d4e5f6","schedule":"30 2 * * *","command":"backup","concurrency_policy":"forbid","maintenance":true,"task-in-maintenance":true}
]"#;

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
            DokkuCommand::CronList {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok(CRON_JSON)),
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
            .uri("/apps/alpha/cron")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    extract_csrf(&get_body(resp).await)
}

#[tokio::test]
async fn cron_routes_require_login() {
    let (state, _dir) = test_state().await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;

    for path in ["/apps/alpha/cron", "/apps/alpha/partials/cron"] {
        let resp = test::call_service(&app, test::TestRequest::get().uri(path).to_request()).await;
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT, "{path}");
        assert_eq!(location(&resp), "/login", "{path}");
    }
}

#[tokio::test]
async fn cron_partial_renders_schedules_and_suspend_state() {
    let (state, _client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/cron")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("0 * * * *"), "{body}");
    assert!(body.contains("echo hourly"), "{body}");
    assert!(body.contains("backup"), "{body}");
    assert!(body.contains("suspended"), "{body}");
    assert!(body.contains(r#"hx-post="/apps/alpha/cron/run""#), "{body}");
    assert!(
        body.contains(r#"hx-post="/apps/alpha/cron/resume""#),
        "{body}"
    );
    assert!(
        body.contains(r#"hx-post="/apps/alpha/cron/suspend""#),
        "{body}"
    );
    assert!(body.contains(r#"value="a1b2c3""#), "{body}");
}

#[tokio::test]
async fn hx_cron_run_streams_and_runs_the_task() {
    let client = seeded_app_client().stub(
        DokkuCommand::CronRun {
            app: app_name("alpha"),
            cron_id: "a1b2c3".into(),
        },
        Ok(DokkuOutput::ok("-----> running echo hourly\n")),
    );
    let (state, client, _dir) = harness(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/cron/run",
            format!("csrf_token={csrf}&cron_id=a1b2c3"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("Running cron action for alpha"), "{body}");
    assert!(
        body.contains(r#"data-refresh="/apps/alpha/partials/cron""#),
        "{body}"
    );

    let (_, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(events.contains("running echo hourly"), "{events}");
    assert!(events.contains(r#""ok":true"#), "{events}");
    assert!(
        client.calls().contains(&DokkuCommand::CronRun {
            app: app_name("alpha"),
            cron_id: "a1b2c3".into(),
        }),
        "run executed"
    );
}

#[tokio::test]
async fn hx_cron_suspend_and_resume_stream_and_run() {
    let client = seeded_app_client()
        .stub(
            DokkuCommand::CronSuspend {
                app: app_name("alpha"),
                cron_id: "a1b2c3".into(),
            },
            Ok(DokkuOutput::ok("")),
        )
        .stub(
            DokkuCommand::CronResume {
                app: app_name("alpha"),
                cron_id: "d4e5f6".into(),
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
            "/apps/alpha/cron/suspend",
            format!("csrf_token={csrf}&cron_id=a1b2c3"),
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
            "/apps/alpha/cron/resume",
            format!("csrf_token={csrf}&cron_id=d4e5f6"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    let (_, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(events.contains(r#""ok":true"#), "{events}");

    assert!(
        client.calls().contains(&DokkuCommand::CronSuspend {
            app: app_name("alpha"),
            cron_id: "a1b2c3".into(),
        }),
        "suspend ran"
    );
    assert!(
        client.calls().contains(&DokkuCommand::CronResume {
            app: app_name("alpha"),
            cron_id: "d4e5f6".into(),
        }),
        "resume ran"
    );
}

#[tokio::test]
async fn cron_rejects_unknown_task_ids_without_dokku() {
    let (state, client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/cron/run",
            format!("csrf_token={csrf}&cron_id=bad id"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("Unknown cron task id."), "{body}");
    assert!(
        client.calls().iter().all(|call| !matches!(
            call,
            DokkuCommand::CronRun { .. }
                | DokkuCommand::CronSuspend { .. }
                | DokkuCommand::CronResume { .. }
        )),
        "no cron commands for rejected ids"
    );
}

#[tokio::test]
async fn non_htmx_cron_run_queues_flashes_and_is_audited() {
    let client = seeded_app_client().stub(
        DokkuCommand::CronRun {
            app: app_name("alpha"),
            cron_id: "a1b2c3".into(),
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
            "/apps/alpha/cron/run",
            format!("csrf_token={csrf}&cron_id=a1b2c3"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/alpha/cron");
    let cookie = common::response_cookie(&resp).unwrap_or(cookie);
    wait_for_call(
        &client,
        &DokkuCommand::CronRun {
            app: app_name("alpha"),
            cron_id: "a1b2c3".into(),
        },
    )
    .await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/cron")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Queued: run cron task a1b2c3."), "{body}");

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/activity")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("cron.run"), "{body}");
}
