mod common;

use std::collections::HashMap;
use std::sync::Arc;

use actix_web::body::MessageBody;
use actix_web::cookie::Cookie;
use actix_web::dev::ServiceResponse;
use actix_web::http::StatusCode;
use actix_web::test;

use common::{
    complete_setup, extract_csrf, form_request, get_body, location, response_cookie, run_url,
    seed_user, test_state, test_state_with_client,
};

use dokku_ui::dokku::{
    CapabilitiesStore, DokkuClient, DokkuError, DokkuOutput, FakeResolver, MockClient,
    SnapshotStore,
};
use dokku_ui::domain::AppName;
use dokku_ui::domain::capabilities::CapabilityFamily;
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

const CONFIG_FIXTURE: &str = include_str!("fixtures/config_show.txt");
const LOGS_FIXTURE: &str = include_str!("fixtures/logs.txt");
const PS_SCALE_FIXTURE: &str = include_str!("fixtures/ps_scale.txt");
const PS_INSPECT_FIXTURE: &str = include_str!("fixtures/ps_inspect.json");
const RESOURCE_REPORT_FIXTURE: &str = include_str!("fixtures/resource_report.txt");
const POSTGRES_INFO_FIXTURE: &str = include_str!("fixtures/postgres_info.txt");
const DOKKU_VERSION_FIXTURE: &str = include_str!("fixtures/dokku_version.txt");
const PLUGIN_LIST_FIXTURE: &str = include_str!("fixtures/plugin_list.txt");
const LOGS_HELP_FIXTURE: &str = include_str!("fixtures/logs_help.txt");

fn apps_report() -> DokkuOutput {
    DokkuOutput::ok(r#"{"app-created-at": "1791023796", "app-locked": "false"}"#.to_owned())
}

fn ps_report(running: bool, deployed: bool, processes: i64) -> DokkuOutput {
    DokkuOutput::ok(format!(
        r#"{{"deployed": "{deployed}", "running": "{running}", "processes": "{processes}"}}"#
    ))
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
            Ok(apps_report()),
        )
        .stub(
            DokkuCommand::PsReport {
                app: app_name("alpha"),
            },
            Ok(ps_report(true, true, 2)),
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
            capabilities: Arc::new(CapabilitiesStore::new(dokku.clone(), pool.clone())),
            db: pool,
            settings,
            dokku,
            snapshot,
        },
        client_arc,
        dir,
    )
}

fn exit_error(code: i32, stderr: &str) -> DokkuError {
    DokkuError::Exit {
        code,
        stderr: stderr.to_owned(),
    }
}

/// Polls a GET until `needle` appears in the body (the job executor finishes
/// the run asynchronously after the command itself has been called).
async fn wait_for_body<S, B, E>(
    app: &S,
    uri: &str,
    cookie: &Cookie<'static>,
    needle: &str,
) -> String
where
    S: actix_web::dev::Service<actix_http::Request, Response = ServiceResponse<B>, Error = E>,
    B: MessageBody,
    E: std::fmt::Debug,
{
    for _ in 0..200 {
        let resp = test::call_service(
            app,
            test::TestRequest::get()
                .uri(uri)
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        let body = get_body(resp).await;
        if body.contains(needle) {
            return body;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let resp = test::call_service(
        app,
        test::TestRequest::get()
            .uri(uri)
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    get_body(resp).await
}

/// Polls the mock client until the executor task has run `command` (jobs
/// execute asynchronously now), failing after a short budget.
async fn wait_for_call(client: &Arc<MockClient>, command: &DokkuCommand) {
    for _ in 0..200 {
        if client.calls().contains(command) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("command never called: {command:?}");
}

fn urlencode(input: &str) -> String {
    input
        .bytes()
        .flat_map(|byte| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'=' | b'\n' => {
                vec![byte as char]
            }
            other => format!("%{other:02X}").chars().collect(),
        })
        .collect()
}

#[tokio::test]
async fn app_routes_redirect_to_login_when_unauthenticated() {
    let (state, _dir) = test_state().await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;

    for path in [
        "/apps/new",
        "/apps/alpha",
        "/apps/alpha/processes",
        "/apps/alpha/services",
        "/apps/alpha/delete",
        "/apps/alpha/config",
        "/apps/alpha/logs",
        "/apps/alpha/partials/overview",
        "/apps/alpha/partials/processes",
        "/apps/alpha/partials/services",
        "/apps/alpha/partials/config",
        "/apps/alpha/partials/logs",
        "/apps/alpha/partials/delete-confirm",
        "/actions/runs/1/events",
        "/services/postgres",
        "/services/postgres/new",
        "/services/postgres/partials/list",
        "/services/postgres/cache",
        "/services/postgres/cache/links",
        "/services/postgres/cache/logs",
        "/services/postgres/cache/partials/overview",
        "/services/postgres/cache/partials/links",
        "/services/postgres/cache/partials/logs",
        "/services/postgres/cache/partials/delete-confirm",
        "/services/postgres/cache/partials/stats",
        "/services/postgres/cache/delete",
        "/activity",
        "/apps/alpha/config/edit",
        "/apps/alpha/settings",
        "/apps/alpha/partials/settings",
        "/apps/alpha/activity",
        "/apps/alpha/config/edit",
        "/apps/alpha/settings",
        "/apps/alpha/partials/settings",
        "/services/postgres/cache/activity",
        "/volumes",
        "/volumes/partials/list",
        "/volumes/partials/usage?entry=legacy-90db719326",
        "/volumes/partials/disk-summary",
    ] {
        let resp = test::call_service(&app, test::TestRequest::get().uri(path).to_request()).await;
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT, "{path}");
        assert_eq!(location(&resp), "/login", "{path}");
    }

    for (path, body) in [
        ("/apps", "name=alpha"),
        ("/apps/alpha/delete", "name=alpha"),
        ("/apps/alpha/start", ""),
        ("/apps/alpha/stop", ""),
        ("/apps/alpha/restart", ""),
        ("/apps/alpha/rebuild", ""),
        ("/apps/alpha/scale", "scale_web=2"),
        ("/refresh", ""),
        ("/services/postgres", "name=cache"),
        ("/services/postgres/cache/start", ""),
        ("/services/postgres/cache/stop", ""),
        ("/services/postgres/cache/restart", ""),
        ("/services/postgres/cache/destroy", "name=cache"),
        ("/services/postgres/cache/expose", "ports=5432"),
        ("/services/postgres/cache/unexpose", ""),
        ("/services/postgres/cache/link", "app=alpha"),
        ("/services/postgres/cache/unlink", "app=alpha"),
        ("/volumes/mount", "app=alpha&host=/h&container=/c"),
        ("/volumes/unmount", "app=alpha&spec=/h:/c"),
    ] {
        let resp = test::call_service(&app, form_request(path, body.to_owned()).to_request()).await;
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT, "{path}");
        assert_eq!(location(&resp), "/login", "{path}");
    }
}

#[tokio::test]
async fn create_app_redirects_to_show_with_success_flash() {
    let (state, _dir) = test_state_with_client(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/new")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let csrf = extract_csrf(&get_body(resp).await);

    let resp = test::call_service(
        &app,
        form_request("/apps", format!("csrf_token={csrf}&name=alpha"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/alpha");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("App &#39;alpha&#39; created."));
}

#[tokio::test]
async fn create_app_with_invalid_name_flashes_and_skips_dokku() {
    let (state, client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/new")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let csrf = extract_csrf(&get_body(resp).await);

    let resp = test::call_service(
        &app,
        form_request("/apps", format!("csrf_token={csrf}&name=Bad_App"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/new");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/new")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Invalid app name"));
    assert!(
        client
            .calls()
            .iter()
            .all(|c| !matches!(c, DokkuCommand::AppsCreate { .. })),
        "no AppsCreate call for invalid name"
    );
}

#[tokio::test]
async fn create_app_error_flashes_and_returns_to_form() {
    let (state, _dir) = test_state_with_client(MockClient::new().stub(
        DokkuCommand::AppsCreate {
            app: app_name("alpha"),
        },
        Err(exit_error(1, "already exists")),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/new")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let csrf = extract_csrf(&get_body(resp).await);

    let resp = test::call_service(
        &app,
        form_request("/apps", format!("csrf_token={csrf}&name=alpha"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/new");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/new")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Failed to create app"));
    assert!(body.contains("already exists"));
}

#[tokio::test]
async fn show_shell_renders_htmx_panel_tabs_and_actions() {
    let (state, _dir) = test_state_with_client(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("alpha"));
    assert!(
        body.contains(r#"hx-get="/apps/alpha/partials/overview""#),
        "overview panel wired to htmx"
    );
    assert!(body.contains(r#"hx-trigger="load""#), "panel loads on load");
    assert!(body.contains("Loading"), "skeleton shown before swap");
    assert!(
        body.contains(r#"href="/apps/alpha/processes""#),
        "tab links present"
    );
    assert!(
        body.contains("border-emerald-500"),
        "active tab highlighted"
    );
    assert!(
        body.contains(r#"hx-post="/apps/alpha/restart""#),
        "actions post via htmx"
    );
    assert!(body.contains(r##"hx-target="#modal-content""##));
    assert!(body.contains("data-action-form"));
    assert!(body.contains("data-modal-overlay"));
    assert!(body.contains("data-modal-card"));
    assert!(body.contains("data-modal-body"));
    assert!(body.contains(r#"id="modal-content""#));
    assert!(
        body.contains(r#"hx-get="/apps/alpha/partials/delete-confirm""#),
        "delete opens the confirmation modal"
    );
    assert!(body.contains("/static/js/actions.js"));
}

#[tokio::test]
async fn overview_partial_renders_app_data() {
    let (state, _dir) = test_state_with_client(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/overview")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("Running"));
    assert!(body.contains(r#">2</p>"#), "process count");
    assert!(body.contains("Yes"), "deployed flag");
    assert!(body.contains("2026-10-03 10:36 UTC"), "created at");
    assert!(body.contains("no"), "locked label");
    assert!(body.contains("Updated"), "data age chip");
    assert!(
        !body.contains("<!doctype html>"),
        "fragment, not a full page"
    );
}

#[tokio::test]
async fn show_renders_populated_app_details() {
    let client = seeded_app_client()
        .stub(
            DokkuCommand::BuildsReport {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok(include_str!("fixtures/builds_report.json"))),
        )
        .stub(
            DokkuCommand::DomainsReport {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok(include_str!(
                "fixtures/domains_report.json"
            ))),
        )
        .stub(
            DokkuCommand::PluginList,
            Ok(DokkuOutput::ok(include_str!("fixtures/plugin_list.txt"))),
        )
        .stub(
            DokkuCommand::AppLinks {
                plugin: "postgres".into(),
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok(include_str!("fixtures/app_links.txt"))),
        );
    let (state, _client, _dir) = harness(client).await;

    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/overview")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains(">built</dd>"), "image status: {body}");
    assert!(body.contains(">yes</dd>"), "link/dns status: {body}");
}

#[tokio::test]
async fn show_unknown_app_falls_back_to_live_list() {
    let (state, client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/nope")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let list_calls = client
        .calls()
        .iter()
        .filter(|call| matches!(call, DokkuCommand::AppsList))
        .count();
    assert_eq!(list_calls, 2, "one cold load plus one live fallback");
}

#[tokio::test]
async fn restart_refreshes_that_apps_report() {
    let (state, client, _dir) = harness(seeded_app_client().stub(
        DokkuCommand::PsRestart {
            app: app_name("alpha"),
        },
        Ok(DokkuOutput::ok("")),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let csrf = extract_csrf(&get_body(resp).await);

    let resp = test::call_service(
        &app,
        form_request("/apps/alpha/restart", format!("csrf_token={csrf}"))
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    wait_for_call(
        &client,
        &DokkuCommand::PsRestart {
            app: app_name("alpha"),
        },
    )
    .await;
    let report_calls = || {
        client
            .calls()
            .iter()
            .filter(|call| {
                matches!(
                    call,
                    DokkuCommand::PsReport { app } if app.as_str() == "alpha"
                )
            })
            .count()
    };
    for _ in 0..200 {
        if report_calls() == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(report_calls(), 2, "initial load plus post-action refresh");
}

#[tokio::test]
async fn action_refreshes_only_cheap_reports() {
    let (state, client, _dir) = harness(
        seeded_app_client()
            .stub(
                DokkuCommand::PsRestart {
                    app: app_name("alpha"),
                },
                Ok(DokkuOutput::ok("")),
            )
            .stub(
                DokkuCommand::BuildsReport {
                    app: app_name("alpha"),
                },
                Ok(DokkuOutput::ok(include_str!("fixtures/builds_report.json"))),
            ),
    )
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let csrf = extract_csrf(&get_body(resp).await);

    let resp = test::call_service(
        &app,
        form_request("/apps/alpha/restart", format!("csrf_token={csrf}"))
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    assert!(
        client
            .calls()
            .iter()
            .all(|c| !matches!(c, DokkuCommand::BuildsReport { .. })),
        "actions sync only ps/apps state; the overview fragment fetches details on demand"
    );
}

#[tokio::test]
async fn show_unknown_app_renders_404() {
    let (state, _dir) = test_state_with_client(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/nope")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = get_body(resp).await;
    assert!(body.contains("404"));
}

#[tokio::test]
async fn delete_confirm_renders_name_echo_field() {
    let (state, _dir) = test_state_with_client(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/delete")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("Type <span class=\"font-mono text-white\">alpha</span> to confirm"));
    assert!(body.contains(r#"action="/apps/alpha/delete""#));
    assert!(body.contains(r#"name="name" type="text""#));
}

#[tokio::test]
async fn destroy_with_matching_echo_queues_destroy_and_redirects_home() {
    let (state, client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/delete")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let csrf = extract_csrf(&get_body(resp).await);

    let resp = test::call_service(
        &app,
        form_request(
            "/apps/alpha/delete",
            format!("csrf_token={csrf}&name=alpha"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let destroy = DokkuCommand::AppsDestroy {
        app: app_name("alpha"),
        force: true,
    };
    wait_for_call(&client, &destroy).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Queued: destroy alpha."));
}

#[tokio::test]
async fn destroy_with_mismatched_echo_redirects_back_without_calling_dokku() {
    let (state, client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/delete")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let csrf = extract_csrf(&get_body(resp).await);

    let resp = test::call_service(
        &app,
        form_request("/apps/alpha/delete", format!("csrf_token={csrf}&name=beta"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/alpha/delete");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    assert!(
        client
            .calls()
            .iter()
            .all(|c| !matches!(c, DokkuCommand::AppsDestroy { .. })),
        "no AppsDestroy call on echo mismatch"
    );

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/delete")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Type `alpha` to confirm deletion."));
}

#[tokio::test]
async fn destroy_error_is_queued_and_fails_in_the_audit_trail() {
    let (state, client, _dir) = harness(MockClient::new().stub(
        DokkuCommand::AppsDestroy {
            app: app_name("alpha"),
            force: true,
        },
        Err(exit_error(1, "gone")),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/delete")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let csrf = extract_csrf(&get_body(resp).await);

    let resp = test::call_service(
        &app,
        form_request(
            "/apps/alpha/delete",
            format!("csrf_token={csrf}&name=alpha"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    wait_for_call(
        &client,
        &DokkuCommand::AppsDestroy {
            app: app_name("alpha"),
            force: true,
        },
    )
    .await;

    let body = wait_for_body(&app, "/activity", &cookie, "failed").await;
    assert!(body.contains("app.destroy"), "{body}");
}

#[tokio::test]
async fn app_posts_without_valid_csrf_are_rejected() {
    let (state, _dir) = test_state_with_client(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    for (path, body) in [
        ("/apps", "name=alpha"),
        ("/apps/alpha/delete", "name=alpha"),
        ("/apps/alpha/start", ""),
        ("/apps/alpha/stop", ""),
        ("/apps/alpha/restart", ""),
        ("/apps/alpha/rebuild", ""),
        ("/apps/alpha/scale", "scale_web=2"),
        ("/services/postgres", "name=cache"),
        ("/services/postgres/cache/start", ""),
        ("/services/postgres/cache/stop", ""),
        ("/services/postgres/cache/restart", ""),
        ("/services/postgres/cache/destroy", "name=cache"),
        ("/services/postgres/cache/expose", "ports=5432"),
        ("/services/postgres/cache/unexpose", ""),
        ("/services/postgres/cache/link", "app=alpha"),
        ("/services/postgres/cache/unlink", "app=alpha"),
        ("/volumes/mount", "app=alpha&host=/h&container=/c"),
        ("/volumes/unmount", "app=alpha&spec=/h:/c"),
    ] {
        let resp = test::call_service(
            &app,
            form_request(path, body.to_owned())
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "{path} missing token"
        );

        let resp = test::call_service(
            &app,
            form_request(path, format!("csrf_token={}&{body}", "f".repeat(64)))
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{path} wrong token");
    }
}

#[tokio::test]
async fn actions_are_queued_flash_and_redirect_to_show() {
    for (path, command, operation, flash) in [
        (
            "/apps/alpha/start",
            DokkuCommand::PsStart {
                app: app_name("alpha"),
            },
            "app.start",
            "Queued: start alpha.",
        ),
        (
            "/apps/alpha/stop",
            DokkuCommand::PsStop {
                app: app_name("alpha"),
            },
            "app.stop",
            "Queued: stop alpha.",
        ),
        (
            "/apps/alpha/restart",
            DokkuCommand::PsRestart {
                app: app_name("alpha"),
            },
            "app.restart",
            "Queued: restart alpha.",
        ),
        (
            "/apps/alpha/rebuild",
            DokkuCommand::PsRebuild {
                app: app_name("alpha"),
            },
            "app.rebuild",
            "Queued: rebuild alpha.",
        ),
    ] {
        let (state, client, _dir) =
            harness(seeded_app_client().stub(command.clone(), Ok(DokkuOutput::ok("")))).await;
        let app = test::init_service(build_app(state)).await;
        let cookie = complete_setup(&app).await;

        let resp = test::call_service(
            &app,
            test::TestRequest::get()
                .uri("/apps/alpha")
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        let csrf = extract_csrf(&get_body(resp).await);

        let resp = test::call_service(
            &app,
            form_request(path, format!("csrf_token={csrf}"))
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER, "{path}");
        assert_eq!(location(&resp), "/apps/alpha", "{path}");
        let cookie = response_cookie(&resp).unwrap_or(cookie);

        wait_for_call(&client, &command).await;

        let resp = test::call_service(
            &app,
            test::TestRequest::get()
                .uri("/apps/alpha")
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        let body = get_body(resp).await;
        assert!(body.contains(flash), "{path} flash");

        let body = wait_for_body(&app, "/apps/alpha/activity", &cookie, "succeeded").await;
        assert!(body.contains(operation), "{path} audit trail");
    }
}

#[tokio::test]
async fn action_error_is_queued_and_fails_in_the_audit_trail() {
    let (state, client, _dir) = harness(seeded_app_client().stub(
        DokkuCommand::PsStart {
            app: app_name("alpha"),
        },
        Err(exit_error(1, "no such app")),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let csrf = extract_csrf(&get_body(resp).await);

    let resp = test::call_service(
        &app,
        form_request("/apps/alpha/start", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/alpha");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    wait_for_call(
        &client,
        &DokkuCommand::PsStart {
            app: app_name("alpha"),
        },
    )
    .await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Queued: start alpha."), "{body}");

    let body = wait_for_body(&app, "/apps/alpha/activity", &cookie, "failed").await;
    assert!(body.contains("app.start"), "{body}");
}

#[tokio::test]
async fn action_with_invalid_app_name_flashes_and_skips_dokku() {
    let (state, client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let csrf = extract_csrf(&get_body(resp).await);

    let resp = test::call_service(
        &app,
        form_request("/apps/Bad_App/start", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    assert!(
        client
            .calls()
            .iter()
            .all(|c| !matches!(c, DokkuCommand::PsStart { .. })),
        "no PsStart call for invalid name"
    );

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Invalid app name"));
}

#[tokio::test]
async fn action_buttons_render_on_show_page() {
    let (state, _dir) = test_state_with_client(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;

    assert!(body.contains(r#"action="/apps/alpha/start""#));
    assert!(body.contains(r#"action="/apps/alpha/stop""#));
    assert!(body.contains(r#"action="/apps/alpha/restart""#));
    assert!(body.contains(r#"action="/apps/alpha/rebuild""#));
}

#[tokio::test]
async fn config_shell_renders_tabs_and_panel() {
    let (state, _dir) = test_state_with_client(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/config")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(
        body.contains(r#"hx-get="/apps/alpha/partials/config""#),
        "config panel wired to htmx"
    );
    assert!(body.contains(r#"href="/apps/alpha/config""#), "config tab");
    assert!(body.contains(r#"href="/apps/alpha/logs""#), "logs tab");
    assert!(body.contains(r#"href="/apps/alpha""#), "overview tab");
    assert!(
        body.contains("border-emerald-500"),
        "active tab highlighted"
    );
}

#[tokio::test]
async fn config_partial_renders_env_vars() {
    let (state, _dir) = test_state_with_client(seeded_app_client().stub(
        DokkuCommand::ConfigShow {
            app: app_name("alpha"),
        },
        Ok(DokkuOutput::ok(CONFIG_FIXTURE)),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/config")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("DATABASE_URL"));
    assert!(body.contains("SECRET_KEY"));
    assert!(
        !body.contains("postgres://user:pass@host/db"),
        "value masked"
    );
    assert!(!body.contains("s3cr3t"), "value masked");
    assert!(body.contains("••••••••"), "masked values rendered");
    assert!(!body.contains("=====>"), "dokku header line skipped");
    assert!(
        body.contains("Reveal values"),
        "the reveal button is offered"
    );
}

#[tokio::test]
async fn config_empty_renders_empty_state() {
    let (state, _dir) = test_state_with_client(seeded_app_client().stub(
        DokkuCommand::ConfigShow {
            app: app_name("alpha"),
        },
        Ok(DokkuOutput::ok("=====> alpha env vars\n")),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/config")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("No environment variables set."));
}

#[tokio::test]
async fn config_unknown_app_renders_404_without_config_call() {
    let (state, client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/nope/config")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(
        client
            .calls()
            .iter()
            .all(|c| !matches!(c, DokkuCommand::ConfigShow { .. })),
        "no ConfigShow call for unknown app"
    );
}

#[tokio::test]
async fn config_fetch_error_renders_retry_fragment() {
    let (state, _dir) = test_state_with_client(seeded_app_client().stub(
        DokkuCommand::ConfigShow {
            app: app_name("alpha"),
        },
        Err(exit_error(1, "boom")),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/config")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "200 so htmx swaps it in");
    let body = get_body(resp).await;
    assert!(body.contains("Could not load this section."));
    assert!(body.contains("boom"));
    assert!(body.contains("/apps/alpha/partials/config"));
}

#[tokio::test]
async fn logs_shell_renders_lines_selector_and_panel() {
    let (state, client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/logs?lines=50")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("Lines"));
    assert!(body.contains(r#"value="50""#), "shell carries line count");
    assert!(body.contains(r#"hx-get="/apps/alpha/partials/logs""#));
    assert!(body.contains(r#"hx-get="/apps/alpha/partials/logs?lines=50""#));
    assert!(
        client
            .calls()
            .iter()
            .all(|c| !matches!(c, DokkuCommand::Logs { .. })),
        "shell never fetches logs"
    );
}

#[tokio::test]
async fn logs_shell_shows_the_live_tail_badge_from_the_shared_capabilities_row() {
    let (state, _client, _dir) = harness(
        seeded_app_client()
            .stub(
                DokkuCommand::DokkuVersion,
                Ok(DokkuOutput::ok(DOKKU_VERSION_FIXTURE)),
            )
            .stub(
                DokkuCommand::PluginList,
                Ok(DokkuOutput::ok(PLUGIN_LIST_FIXTURE)),
            )
            .stub(
                DokkuCommand::Help {
                    family: CapabilityFamily::Logs,
                },
                Ok(DokkuOutput::ok(LOGS_HELP_FIXTURE)),
            ),
    )
    .await;
    state
        .capabilities
        .ensure_loaded()
        .await
        .expect("probe capabilities");
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/logs")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("Live tail: available"), "{body}");
}

#[tokio::test]
async fn logs_shell_renders_unknown_live_tail_without_probe_data() {
    let (state, _client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/logs")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(
        body.contains("Live tail: unknown"),
        "no probe row yet, so the badge is honest: {body}"
    );
}

#[tokio::test]
async fn logs_partial_defaults_to_200_lines_and_strips_ansi() {
    let (state, client, _dir) = harness(seeded_app_client().stub(
        DokkuCommand::Logs {
            app: app_name("alpha"),
            num_lines: 200,
            follow: false,
            process: None,
        },
        Ok(DokkuOutput::ok(LOGS_FIXTURE)),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/logs")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("listening on 0.0.0.0:8080"));
    assert!(body.contains("GET /healthz 200"));
    assert!(body.contains("ERROR something failed"));
    assert!(!body.contains('\x1b'), "ANSI escapes stripped");
    assert!(client.calls().contains(&DokkuCommand::Logs {
        app: app_name("alpha"),
        num_lines: 200,
        follow: false,
        process: None
    }));
}

#[tokio::test]
async fn logs_partial_clamps_lines_parameter() {
    for (query, expected) in [
        ("", 200),
        ("?lines=5", 10),
        ("?lines=99999", 1000),
        ("?lines=abc", 200),
        ("?lines=50", 50),
    ] {
        let command = DokkuCommand::Logs {
            app: app_name("alpha"),
            num_lines: expected,
            follow: false,
            process: None,
        };
        let (state, client, _dir) =
            harness(seeded_app_client().stub(command.clone(), Ok(DokkuOutput::ok("")))).await;
        let app = test::init_service(build_app(state)).await;
        let cookie = complete_setup(&app).await;

        let resp = test::call_service(
            &app,
            test::TestRequest::get()
                .uri(&format!("/apps/alpha/partials/logs{query}"))
                .cookie(cookie)
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK, "{query}");
        assert!(
            client.calls().contains(&command),
            "{query} requests {expected} lines"
        );
    }
}

#[tokio::test]
async fn logs_unknown_app_renders_404() {
    let (state, _dir) = test_state_with_client(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/nope/logs")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn logs_empty_renders_empty_state() {
    let (state, _dir) = test_state_with_client(seeded_app_client().stub(
        DokkuCommand::Logs {
            app: app_name("alpha"),
            num_lines: 200,
            follow: false,
            process: None,
        },
        Ok(DokkuOutput::ok("")),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/logs")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("No log lines yet."));
}

#[tokio::test]
async fn logs_fetch_error_renders_retry_fragment() {
    let (state, _dir) = test_state_with_client(seeded_app_client().stub(
        DokkuCommand::Logs {
            app: app_name("alpha"),
            num_lines: 50,
            follow: false,
            process: None,
        },
        Err(exit_error(1, "boom")),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/logs?lines=50")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "200 so htmx swaps it in");
    let body = get_body(resp).await;
    assert!(body.contains("Could not load this section."));
    assert!(body.contains("boom"));
    assert!(
        body.contains("/apps/alpha/partials/logs?lines=50"),
        "retry keeps the chosen line count: {body}"
    );
}

fn processes_client() -> MockClient {
    seeded_app_client()
        .stub(
            DokkuCommand::PsScaleGet {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok(PS_SCALE_FIXTURE)),
        )
        .stub(
            DokkuCommand::PsInspect {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok(PS_INSPECT_FIXTURE)),
        )
        .stub(
            DokkuCommand::ResourceReport {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok(RESOURCE_REPORT_FIXTURE)),
        )
}

#[tokio::test]
async fn processes_shell_renders_tabs_and_panel() {
    let (state, _dir) = test_state_with_client(processes_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/processes")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(
        body.contains(r#"href="/apps/alpha/processes""#),
        "processes tab"
    );
    assert!(
        body.contains(r#"href="/apps/alpha/services""#),
        "services tab"
    );
    assert!(
        body.contains("border-emerald-500"),
        "active tab highlighted"
    );
    assert!(
        body.contains(r#"hx-get="/apps/alpha/partials/processes""#),
        "processes panel wired to htmx"
    );
}

#[tokio::test]
async fn processes_partial_renders_formation_containers_and_resources() {
    let (state, _dir) = test_state_with_client(processes_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/processes")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("Formation"));
    assert!(body.contains(r#"action="/apps/alpha/scale""#), "scale form");
    assert!(
        body.contains(r#"name="scale_web""#),
        "scale input for web: {body}"
    );
    assert!(
        !body.contains(r#"name="scale_release""#),
        "release is not editable"
    );
    assert!(body.contains("alpha.web.1"), "container row");
    assert!(body.contains("1024"), "resource limit prefilled");
    assert!(
        body.contains(r#"hx-post="/apps/alpha/resources""#),
        "resource editing form"
    );
    assert!(body.contains(r#"name="limit_memory""#), "{body}");
    assert!(body.contains("Apply scale"));
}

#[tokio::test]
async fn processes_hides_form_when_scaling_is_disabled() {
    let client = seeded_app_client()
        .stub(
            DokkuCommand::PsReport {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok(
                r#"{"deployed":"true","running":"true","processes":"1","ps-can-scale":"false","status-web.1":"running"}"#,
            )),
        )
        .stub(
            DokkuCommand::PsScaleGet {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok(PS_SCALE_FIXTURE)),
        )
        .stub(
            DokkuCommand::PsInspect {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok("[]")),
        )
        .stub(
            DokkuCommand::ResourceReport {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok("")),
        );
    let (state, _dir) = test_state_with_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/processes")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;

    assert!(
        !body.contains(r#"action="/apps/alpha/scale""#),
        "form hidden"
    );
    assert!(body.contains("app.json formation"));
    assert!(body.contains("No running containers."));
    assert!(
        body.contains(r#"name="process_type" value="web""#),
        "resource editing stays available"
    );
}

#[tokio::test]
async fn processes_shows_note_when_no_formation_exists() {
    let client = processes_client().stub(
        DokkuCommand::PsScaleGet {
            app: app_name("alpha"),
        },
        Ok(DokkuOutput::ok("[]")),
    );
    let (state, _dir) = test_state_with_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/processes")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("No formation found yet"));
    assert!(!body.contains(r#"action="/apps/alpha/scale""#));
}

#[tokio::test]
async fn processes_degrades_when_detail_commands_fail() {
    let client = seeded_app_client()
        .stub(
            DokkuCommand::PsScaleGet {
                app: app_name("alpha"),
            },
            Err(exit_error(1, "nope")),
        )
        .stub(
            DokkuCommand::PsInspect {
                app: app_name("alpha"),
            },
            Err(exit_error(1, "nope")),
        )
        .stub(
            DokkuCommand::ResourceReport {
                app: app_name("alpha"),
            },
            Err(exit_error(1, "nope")),
        );
    let (state, _dir) = test_state_with_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/processes")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("No formation found yet"));
}

#[tokio::test]
async fn processes_unknown_app_renders_404() {
    let (state, _dir) = test_state_with_client(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/nope/processes")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn scale_posts_formation_and_redirects_with_flash() {
    let scaled = DokkuCommand::PsScaleSet {
        app: app_name("alpha"),
        scales: vec![
            dokku_ui::domain::types::ScaleEntry::new("web", 4),
            dokku_ui::domain::types::ScaleEntry::new("worker", 0),
        ],
    };
    let (state, client, _dir) = harness(
        processes_client()
            .stub(scaled.clone(), Ok(DokkuOutput::ok("")))
            .stub(
                DokkuCommand::PsReport {
                    app: app_name("alpha"),
                },
                Ok(DokkuOutput::ok(
                    r#"{"deployed":"true","running":"true","processes":"4","status-web.1":"running"}"#,
                )),
            ),
    )
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let csrf = extract_csrf(&get_body(resp).await);

    let resp = test::call_service(
        &app,
        form_request(
            "/apps/alpha/scale",
            format!("csrf_token={csrf}&scale_web=4&scale_worker=0"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/alpha/processes");
    wait_for_call(&client, &scaled).await;

    let cookie = response_cookie(&resp).unwrap_or(cookie);
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/processes")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Queued: scale alpha."), "queued flash");
}

#[tokio::test]
async fn scale_rejects_out_of_range_value_without_calling_dokku() {
    let (state, client, _dir) = harness(processes_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let csrf = extract_csrf(&get_body(resp).await);

    let resp = test::call_service(
        &app,
        form_request(
            "/apps/alpha/scale",
            format!("csrf_token={csrf}&scale_web=999"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/alpha/processes");
    assert!(
        client
            .calls()
            .iter()
            .all(|call| !matches!(call, DokkuCommand::PsScaleSet { .. })),
        "no PsScaleSet call for out-of-range value"
    );
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/processes")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("between 0 and 100"));
}

#[tokio::test]
async fn scale_dokku_error_is_queued_and_fails_in_the_audit_trail() {
    let (state, client, _dir) = harness(processes_client().stub(
        DokkuCommand::PsScaleSet {
            app: app_name("alpha"),
            scales: vec![dokku_ui::domain::types::ScaleEntry::new("web", 2)],
        },
        Err(exit_error(1, "cannot scale")),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let csrf = extract_csrf(&get_body(resp).await);

    let resp = test::call_service(
        &app,
        form_request(
            "/apps/alpha/scale",
            format!("csrf_token={csrf}&scale_web=2"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/alpha/processes");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    wait_for_call(
        &client,
        &DokkuCommand::PsScaleSet {
            app: app_name("alpha"),
            scales: vec![dokku_ui::domain::types::ScaleEntry::new("web", 2)],
        },
    )
    .await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/processes")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Queued: scale alpha."), "{body}");

    let body = wait_for_body(&app, "/apps/alpha/activity", &cookie, "failed").await;
    assert!(body.contains("app.scale"), "{body}");
}

#[tokio::test]
async fn services_shell_renders_tabs_and_panel() {
    let (state, _dir) = test_state_with_client(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/services")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(
        body.contains(r#"hx-get="/apps/alpha/partials/services""#),
        "services panel wired to htmx"
    );
    assert!(
        body.contains(r#"href="/apps/alpha/services""#),
        "services tab active"
    );
    assert!(
        body.contains(r#"href="/apps/alpha/processes""#),
        "processes tab"
    );
    assert!(
        body.contains("border-emerald-500"),
        "active tab highlighted"
    );
}

#[tokio::test]
async fn services_partial_renders_linked_service_details() {
    let client = seeded_app_client()
        .stub(
            DokkuCommand::PluginList,
            Ok(DokkuOutput::ok(include_str!("fixtures/plugin_list.txt"))),
        )
        .stub(
            DokkuCommand::AppLinks {
                plugin: "postgres".into(),
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok(include_str!("fixtures/app_links.txt"))),
        )
        .stub(
            DokkuCommand::ServiceInfo {
                plugin: "postgres".into(),
                service: "roboswarm-db".into(),
            },
            Ok(DokkuOutput::ok(POSTGRES_INFO_FIXTURE)),
        );
    let (state, _client, _dir) = harness(client).await;

    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/services")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("postgres"));
    assert!(body.contains("roboswarm-db"));
    assert!(body.contains("running"));
    assert!(body.contains("postgres:16.2"));
    assert!(
        body.contains("5432-&#62;15432"),
        "exposed ports missing: {body}"
    );
    assert!(body.contains("roboswarm-server"));
    assert!(!body.contains("postgres://"), "dsn never rendered: {body}");
    assert!(body.contains("Container ID"));
}

#[tokio::test]
async fn services_shows_empty_state_when_no_links() {
    let client = seeded_app_client().stub(
        DokkuCommand::PluginList,
        Ok(DokkuOutput::ok(include_str!("fixtures/plugin_list.txt"))),
    );
    let (state, _client, _dir) = harness(client).await;

    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/services")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("No services are linked to this app."));
}

#[tokio::test]
async fn services_marks_unknown_when_plugin_list_fails() {
    let (state, _dir) = test_state_with_client(
        seeded_app_client().stub(DokkuCommand::PluginList, Err(exit_error(1, "no plugins"))),
    )
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/services")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Service links could not be determined yet."));
}

#[tokio::test]
async fn services_degrades_to_unknown_status_when_info_fails() {
    let client = seeded_app_client()
        .stub(
            DokkuCommand::PluginList,
            Ok(DokkuOutput::ok(include_str!("fixtures/plugin_list.txt"))),
        )
        .stub(
            DokkuCommand::AppLinks {
                plugin: "postgres".into(),
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok(include_str!("fixtures/app_links.txt"))),
        )
        .stub(
            DokkuCommand::ServiceInfo {
                plugin: "postgres".into(),
                service: "roboswarm-db".into(),
            },
            Err(exit_error(1, "boom")),
        );
    let (state, _client, _dir) = harness(client).await;

    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/services")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("roboswarm-db"));
    assert!(body.contains("unknown"));
}

#[tokio::test]
async fn services_unknown_app_renders_404() {
    let (state, _dir) = test_state_with_client(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/nope/services")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn partials_render_not_found_fragment_for_unknown_app() {
    let (state, client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    for path in [
        "/apps/nope/partials/overview",
        "/apps/nope/partials/processes",
        "/apps/nope/partials/services",
        "/apps/nope/partials/config",
        "/apps/nope/partials/logs",
    ] {
        let resp = test::call_service(
            &app,
            test::TestRequest::get()
                .uri(path)
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "{path}: htmx only swaps 2xx responses"
        );
        let body = get_body(resp).await;
        assert!(body.contains("was not found"), "{path}: {body}");
        assert!(body.contains("may have been deleted"), "{path}");
        assert!(body.contains("Could not load this section."), "{path}");
    }

    assert!(
        client.calls().iter().all(|c| {
            !matches!(
                c,
                DokkuCommand::ConfigShow { .. }
                    | DokkuCommand::Logs { .. }
                    | DokkuCommand::PsScaleGet { .. }
                    | DokkuCommand::ServiceInfo { .. }
                    | DokkuCommand::BuildsReport { .. }
            )
        }),
        "no per-tab detail commands for unknown app"
    );
}

fn hx_form_request(path: &str, body: String) -> actix_web::test::TestRequest {
    form_request(path, body).insert_header(("HX-Request", "true"))
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
            .uri("/apps/alpha")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    extract_csrf(&get_body(resp).await)
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

#[tokio::test]
async fn hx_restart_returns_run_fragment_and_streams_output() {
    let (state, client, _dir) = harness(seeded_app_client().stub(
        DokkuCommand::PsRestart {
            app: app_name("alpha"),
        },
        Ok(DokkuOutput::ok("-----> restarting\n-----> done\n")),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request("/apps/alpha/restart", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    let run_url = run_url(&body);
    assert!(
        run_url.starts_with("/actions/runs/") && run_url.ends_with("/events"),
        "run fragment points at the SSE stream: {body}"
    );
    assert!(body.contains(r#"data-refresh="/apps/alpha/partials/overview""#));
    assert!(body.contains("Restarting alpha"));
    assert!(body.contains("data-run-log"));
    assert!(!body.contains("<!doctype html>"), "fragment, not a page");

    let (status, events) = sse_events(&app, &run_url, &cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        events.contains("event: line\ndata: -----> restarting\n\n"),
        "{events}"
    );
    assert!(
        events.contains("event: line\ndata: -----> done\n\n"),
        "{events}"
    );
    assert!(events.contains("event: done\n"), "{events}");
    assert!(events.contains(r#""ok":true"#), "{events}");
    assert!(events.contains("restarted"), "{events}");
    assert!(
        client.calls().contains(&DokkuCommand::PsRestart {
            app: app_name("alpha")
        }),
        "the action ran"
    );
}

#[tokio::test]
async fn hx_action_failure_streams_error_outcome() {
    let (state, _client, _dir) = harness(seeded_app_client().stub(
        DokkuCommand::PsStart {
            app: app_name("alpha"),
        },
        Err(exit_error(1, "no such app")),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request("/apps/alpha/start", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("Starting alpha"), "{body}");

    let (status, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert!(events.contains(r#""ok":false"#), "{events}");
    assert!(events.contains("no such app"), "{events}");
}

#[tokio::test]
async fn hx_scale_validation_error_returns_modal_error() {
    let (state, client, _dir) = harness(processes_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/scale",
            format!("csrf_token={csrf}&scale_web=999"),
        )
        .cookie(cookie)
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("data-modal-error"), "{body}");
    assert!(body.contains("between 0 and 100"), "{body}");
    assert!(
        client
            .calls()
            .iter()
            .all(|call| !matches!(call, DokkuCommand::PsScaleSet { .. })),
        "no scale command for out-of-range value"
    );
}

#[tokio::test]
async fn hx_scale_returns_run_fragment_targeting_processes() {
    let (state, client, _dir) = harness(processes_client().stub(
        DokkuCommand::PsScaleSet {
            app: app_name("alpha"),
            scales: vec![dokku_ui::domain::types::ScaleEntry::new("web", 2)],
        },
        Ok(DokkuOutput::ok("-----> scaling web\n")),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/scale",
            format!("csrf_token={csrf}&scale_web=2"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(
        body.contains(r#"data-refresh="/apps/alpha/partials/processes""#),
        "{body}"
    );
    assert!(body.contains("Scaling alpha"), "{body}");

    let (status, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert!(events.contains(r#""ok":true"#), "{events}");
    assert!(events.contains("Scaled"), "{events}");
    assert!(
        client.calls().contains(&DokkuCommand::PsScaleSet {
            app: app_name("alpha"),
            scales: vec![dokku_ui::domain::types::ScaleEntry::new("web", 2)],
        }),
        "scale ran"
    );
}

#[tokio::test]
async fn hx_destroy_requires_name_echo_before_starting_a_run() {
    let (state, client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request("/apps/alpha/delete", format!("csrf_token={csrf}&name=beta"))
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("data-modal-error"), "{body}");
    assert!(body.contains("Type `alpha` to confirm deletion."), "{body}");
    assert!(
        client
            .calls()
            .iter()
            .all(|c| !matches!(c, DokkuCommand::AppsDestroy { .. })),
        "no destroy before the name is echoed"
    );
}

#[tokio::test]
async fn hx_destroy_streams_and_redirects_home_on_done() {
    let (state, client, _dir) = harness(seeded_app_client().stub(
        DokkuCommand::AppsDestroy {
            app: app_name("alpha"),
            force: true,
        },
        Ok(DokkuOutput::ok("-----> deleting alpha\n")),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/delete",
            format!("csrf_token={csrf}&name=alpha"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("Deleting alpha"), "{body}");
    assert!(
        !body.contains("data-refresh"),
        "destroy has no fragment to refresh"
    );

    let (status, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert!(events.contains(r#""ok":true"#), "{events}");
    assert!(events.contains(r#""redirect":"/""#), "{events}");
    assert!(events.contains("destroyed"), "{events}");
    assert!(client.calls().contains(&DokkuCommand::AppsDestroy {
        app: app_name("alpha"),
        force: true,
    }));
}

#[tokio::test]
async fn delete_confirm_modal_renders_form_and_404s_unknown_app() {
    let (state, _dir) = test_state_with_client(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/delete-confirm")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains(r#"hx-post="/apps/alpha/delete""#), "{body}");
    assert!(body.contains("data-action-form"), "{body}");
    assert!(body.contains(r#"name="name""#), "{body}");
    assert!(body.contains(r#"name="csrf_token""#), "{body}");
    assert!(!body.contains("<!doctype html>"), "fragment, not a page");

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/nope/partials/delete-confirm")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn action_events_404s_unknown_run() {
    let (state, _client, _dir) = harness(seeded_app_client().stub(
        DokkuCommand::PsRestart {
            app: app_name("alpha"),
        },
        Ok(DokkuOutput::ok("")),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let (status, _) = sse_events(
        &app,
        &format!("/actions/runs/{}/events", "9".repeat(64)),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "unknown run");
}

#[tokio::test]
async fn config_reveal_requires_reauth_then_shows_values_no_store() {
    let (state, _client, _dir) = harness(seeded_app_client().stub(
        DokkuCommand::ConfigShow {
            app: app_name("alpha"),
        },
        Ok(DokkuOutput::ok(CONFIG_FIXTURE)),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = {
        let resp = test::call_service(
            &app,
            test::TestRequest::get()
                .uri("/apps/alpha")
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        extract_csrf(&get_body(resp).await)
    };

    // Without a re-auth window the reveal bounces to /reauth.
    let resp = test::call_service(
        &app,
        form_request("/apps/alpha/config/reveal", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/reauth?next=/apps/alpha/config");

    // Re-auth, then reveal shows the raw values with no-store.
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/reauth?next=/apps/alpha/config")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let reauth_csrf = extract_csrf(&get_body(resp).await);
    let resp = test::call_service(
        &app,
        form_request(
            "/reauth",
            format!("csrf_token={reauth_csrf}&password=correct-horse-battery&next=%2Fapps%2Falpha%2Fconfig"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let resp = test::call_service(
        &app,
        form_request("/apps/alpha/config/reveal", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("cache-control"),
        Some(&actix_web::http::header::HeaderValue::from_static(
            "no-store"
        )),
        "revealed values are never cached"
    );
    let body = get_body(resp).await;
    assert!(body.contains("postgres://user:pass@host/db"), "{body}");
    assert!(body.contains("s3cr3t"), "{body}");
    assert!(!body.contains("••••••••"), "values are unmasked: {body}");

    // The reveal is in the audit trail.
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/activity")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("config.reveal"), "{body}");
}

#[tokio::test]
async fn config_edit_page_requires_reauth_and_enqueues_a_config_job() {
    let (state, client, _dir) = harness(seeded_app_client().stub(
        DokkuCommand::ConfigShow {
            app: app_name("alpha"),
        },
        Ok(DokkuOutput::ok(CONFIG_FIXTURE)),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = {
        let resp = test::call_service(
            &app,
            test::TestRequest::get()
                .uri("/apps/alpha")
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        extract_csrf(&get_body(resp).await)
    };

    // The edit page is reauth-gated too.
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/config/edit")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/reauth?next=/apps/alpha/config/edit");

    // Re-auth, then the edit page shows current values in the textarea.
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/reauth?next=/apps/alpha/config/edit")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let reauth_csrf = extract_csrf(&get_body(resp).await);
    let resp = test::call_service(
        &app,
        form_request(
            "/reauth",
            format!("csrf_token={reauth_csrf}&password=correct-horse-battery&next=%2Fapps%2Falpha%2Fconfig%2Fedit"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/config/edit")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("SECRET_KEY=s3cr3t"), "{body}");
    assert!(
        body.contains("DATABASE_URL=postgres://user:pass@host/db"),
        "{body}"
    );

    // Apply a batch: keep DATABASE_URL, change SECRET_KEY, drop DOKKU_PROXY_PORT.
    let env = "DATABASE_URL=postgres://user:pass@host/db\nSECRET_KEY=brand-new\n";
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/alpha/config",
            format!("csrf_token={csrf}&env={}", urlencode(env)),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let cookie = response_cookie(&resp).unwrap_or(cookie);
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/config")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Queued: config update for alpha."), "{body}");

    wait_for_call(
        &client,
        &DokkuCommand::ConfigSet {
            app: app_name("alpha"),
            vars: vec![dokku_ui::domain::types::EnvVar {
                key: "SECRET_KEY".into(),
                value: "brand-new".into(),
            }],
        },
    )
    .await;
    assert!(
        client.calls().contains(&DokkuCommand::ConfigUnset {
            app: app_name("alpha"),
            keys: vec!["DOKKU_PROXY_PORT".into()],
        }),
        "the diff unsets dropped keys"
    );
    assert!(
        client.calls().contains(&DokkuCommand::ConfigShow {
            app: app_name("alpha"),
        }),
        "current config was read"
    );

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/activity")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("config.set"), "{body}");
}

#[tokio::test]
async fn settings_shell_and_partial_render_lock_state() {
    let (state, _client, _dir) = harness(seeded_app_client().stub(
        DokkuCommand::AppsReport {
            app: app_name("alpha"),
        },
        Ok(DokkuOutput::ok(
            r#"{"app-created-at": "1791023796", "app-locked": "true"}"#,
        )),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/settings")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(
        body.contains(r#"hx-get="/apps/alpha/partials/settings""#),
        "{body}"
    );

    // The partial reads the (freshly resolved) lock state from the snapshot.
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/settings")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("Locked"), "{body}");
    assert!(body.contains(r#"hx-post="/apps/alpha/unlock""#), "{body}");
    assert!(body.contains(r#"hx-post="/apps/alpha/rename""#), "{body}");
}

#[tokio::test]
async fn hx_lock_streams_and_locks_the_app() {
    let (state, client, _dir) = harness(seeded_app_client().stub(
        DokkuCommand::AppsLock {
            app: app_name("alpha"),
        },
        Ok(DokkuOutput::ok("-----> Deploy lock created\n")),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request("/apps/alpha/lock", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("Locking alpha"), "{body}");
    assert!(
        body.contains(r#"data-refresh="/apps/alpha/partials/settings""#),
        "{body}"
    );

    let (_, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(events.contains("Deploy lock created"), "{events}");
    assert!(events.contains(r#""ok":true"#), "{events}");
    assert!(
        client.calls().contains(&DokkuCommand::AppsLock {
            app: app_name("alpha")
        }),
        "lock ran"
    );
}

#[tokio::test]
async fn hx_unlock_streams_and_unlocks_the_app() {
    let (state, client, _dir) = harness(seeded_app_client().stub(
        DokkuCommand::AppsUnlock {
            app: app_name("alpha"),
        },
        Ok(DokkuOutput::ok("-----> Deploy lock removed\n")),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request("/apps/alpha/unlock", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    let (_, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(events.contains(r#""ok":true"#), "{events}");
    assert!(
        client.calls().contains(&DokkuCommand::AppsUnlock {
            app: app_name("alpha")
        }),
        "unlock ran"
    );
}

#[tokio::test]
async fn hx_rename_streams_and_redirects_to_the_new_name() {
    let (state, client, _dir) = harness(seeded_app_client().stub(
        DokkuCommand::AppsRename {
            app: app_name("alpha"),
            new_name: app_name("beta"),
        },
        Ok(DokkuOutput::ok("-----> Renaming alpha to beta\n")),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request("/apps/alpha/rename", format!("csrf_token={csrf}&name=beta"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("Renaming alpha to beta"), "{body}");

    let (_, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(events.contains(r#""ok":true"#), "{events}");
    assert!(events.contains(r#""redirect":"/apps/beta""#), "{events}");
    assert!(
        client.calls().contains(&DokkuCommand::AppsRename {
            app: app_name("alpha"),
            new_name: app_name("beta"),
        }),
        "rename ran"
    );
}

#[tokio::test]
async fn hx_rename_rejects_invalid_and_same_names_without_dokku() {
    let (state, client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    for (new_name, expected) in [
        ("Bad_Name", "Invalid new name"),
        ("alpha", "same as the current name"),
    ] {
        let resp = test::call_service(
            &app,
            hx_form_request(
                "/apps/alpha/rename",
                format!("csrf_token={csrf}&name={new_name}"),
            )
            .cookie(cookie.clone())
            .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK, "modal errors stay 200");
        let body = get_body(resp).await;
        assert!(body.contains(expected), "{new_name}: {body}");
    }
    assert!(
        client
            .calls()
            .iter()
            .all(|call| !matches!(call, DokkuCommand::AppsRename { .. })),
        "no rename command for rejected names"
    );
}

#[tokio::test]
async fn non_htmx_lock_queues_and_redirects_to_settings() {
    let (state, client, _dir) = harness(seeded_app_client().stub(
        DokkuCommand::AppsLock {
            app: app_name("alpha"),
        },
        Ok(DokkuOutput::ok("")),
    ))
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        form_request("/apps/alpha/lock", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/alpha/settings");
    let cookie = response_cookie(&resp).unwrap_or(cookie);
    wait_for_call(
        &client,
        &DokkuCommand::AppsLock {
            app: app_name("alpha"),
        },
    )
    .await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/settings")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Queued: lock alpha."), "{body}");

    // The rename is audited.
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/activity")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("app.lock"), "{body}");
}

#[tokio::test]
async fn hx_resource_save_streams_and_sets_limits_and_reserves() {
    let client = processes_client()
        .stub(
            DokkuCommand::ResourceLimit {
                app: app_name("alpha"),
                process_type: "web".into(),
                cpu: None,
                memory: Some("1024".into()),
                memory_swap: None,
            },
            Ok(DokkuOutput::ok("")),
        )
        .stub(
            DokkuCommand::ResourceReserve {
                app: app_name("alpha"),
                process_type: "web".into(),
                cpu: Some("1".into()),
                memory: None,
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
            "/apps/alpha/resources",
            format!(
                "csrf_token={csrf}&process_type=web&limit_cpu=&limit_memory=1024&limit_memory_swap=&reserve_cpu=1&reserve_memory=&action=save"
            ),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("Updating resources for alpha"), "{body}");
    assert!(
        body.contains(r#"data-refresh="/apps/alpha/partials/processes""#),
        "{body}"
    );

    let (_, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(events.contains(r#""ok":true"#), "{events}");
    assert!(
        client.calls().contains(&DokkuCommand::ResourceLimit {
            app: app_name("alpha"),
            process_type: "web".into(),
            cpu: None,
            memory: Some("1024".into()),
            memory_swap: None,
        }),
        "limit ran"
    );
    assert!(
        client.calls().contains(&DokkuCommand::ResourceReserve {
            app: app_name("alpha"),
            process_type: "web".into(),
            cpu: Some("1".into()),
            memory: None,
        }),
        "reserve ran"
    );
}

#[tokio::test]
async fn hx_resource_save_accepts_unit_values() {
    let client = processes_client().stub(
        DokkuCommand::ResourceLimit {
            app: app_name("alpha"),
            process_type: "web".into(),
            cpu: None,
            memory: Some("1g".into()),
            memory_swap: Some("-1".into()),
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
            "/apps/alpha/resources",
            format!(
                "csrf_token={csrf}&process_type=web&limit_memory=1g&limit_memory_swap=-1&action=save"
            ),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    let (_, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(events.contains(r#""ok":true"#), "{events}");
    assert!(
        client.calls().contains(&DokkuCommand::ResourceLimit {
            app: app_name("alpha"),
            process_type: "web".into(),
            cpu: None,
            memory: Some("1g".into()),
            memory_swap: Some("-1".into()),
        }),
        "unit values reached dokku"
    );
}

#[tokio::test]
async fn hx_resource_clear_streams_and_clears_both() {
    let client = processes_client()
        .stub(
            DokkuCommand::ResourceLimitClear {
                app: app_name("alpha"),
                process_type: "web".into(),
            },
            Ok(DokkuOutput::ok("")),
        )
        .stub(
            DokkuCommand::ResourceReserveClear {
                app: app_name("alpha"),
                process_type: "web".into(),
            },
            Ok(DokkuOutput::ok("")),
        );
    let (state, client, _dir) = harness(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    // The pre-filled form carries whatever `resource:report` said — units and
    // all — and Clear must not reject them.
    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/resources",
            format!("csrf_token={csrf}&process_type=web&limit_memory=1g&limit_cpu=-1&action=clear"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    let (_, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(events.contains(r#"Cleared resources"#), "{events}");
    assert!(
        client.calls().contains(&DokkuCommand::ResourceLimitClear {
            app: app_name("alpha"),
            process_type: "web".into(),
        }),
        "limit-clear ran"
    );
    assert!(
        client
            .calls()
            .contains(&DokkuCommand::ResourceReserveClear {
                app: app_name("alpha"),
                process_type: "web".into(),
            }),
        "reserve-clear ran"
    );
}

#[tokio::test]
async fn resource_rows_include_report_only_process_types() {
    let client = processes_client().stub(
        DokkuCommand::ResourceReport {
            app: app_name("alpha"),
        },
        Ok(DokkuOutput::ok(
            "=====> alpha resource information\n       _default_ limit memory:  256\n",
        )),
    );
    let (state, _client, _dir) = harness(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/processes")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(
        body.contains("_default_"),
        "report-only type rendered: {body}"
    );
    assert!(
        body.contains(r#"value="256""#),
        "its value is editable and clearable: {body}"
    );
}

#[tokio::test]
async fn resource_forms_reject_empty_and_invalid_values_without_dokku() {
    let (state, client, _dir) = harness(processes_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    for (body, expected) in [
        ("process_type=web&action=save", "Enter at least one value"),
        (
            "process_type=web&limit_memory=12x&action=save",
            "Invalid resource value",
        ),
        (
            "process_type=web.1&limit_memory=128&action=save",
            "Invalid process type",
        ),
    ] {
        let resp = test::call_service(
            &app,
            hx_form_request("/apps/alpha/resources", format!("csrf_token={csrf}&{body}"))
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK, "modal errors stay 200");
        let html = get_body(resp).await;
        assert!(html.contains(expected), "{body}: {html}");
    }
    assert!(
        client.calls().iter().all(|call| !matches!(
            call,
            DokkuCommand::ResourceLimit { .. }
                | DokkuCommand::ResourceReserve { .. }
                | DokkuCommand::ResourceLimitClear { .. }
                | DokkuCommand::ResourceReserveClear { .. }
        )),
        "no resource commands for rejected input"
    );
}

#[tokio::test]
async fn non_htmx_resource_save_queues_flashes_and_is_audited() {
    let client = processes_client().stub(
        DokkuCommand::ResourceLimit {
            app: app_name("alpha"),
            process_type: "web".into(),
            cpu: None,
            memory: Some("256".into()),
            memory_swap: None,
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
            "/apps/alpha/resources",
            format!("csrf_token={csrf}&process_type=web&limit_memory=256&action=save"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/alpha/processes");
    let cookie = response_cookie(&resp).unwrap_or(cookie);
    wait_for_call(
        &client,
        &DokkuCommand::ResourceLimit {
            app: app_name("alpha"),
            process_type: "web".into(),
            cpu: None,
            memory: Some("256".into()),
            memory_swap: None,
        },
    )
    .await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/processes")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(
        body.contains("Queued: update resources for alpha."),
        "{body}"
    );

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/activity")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("resource.set"), "{body}");
}

fn plugin_probed_client() -> MockClient {
    seeded_app_client()
        .stub(
            DokkuCommand::DokkuVersion,
            Ok(DokkuOutput::ok("dokku version 0.38.4")),
        )
        .stub(
            DokkuCommand::PluginList,
            Ok(DokkuOutput::ok(
                "=====> Plugins\n  apps 0.38.4 enabled dokku core apps plugin\n  maintenance 1.0.0 enabled dokku maintenance plugin\n  http-auth 1.0.0 enabled dokku http auth plugin\n",
            )),
        )
        .stub(
            DokkuCommand::HttpAuthReport {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok(include_str!(
                "fixtures/http_auth_report.txt"
            ))),
        )
}

#[tokio::test]
async fn settings_plugin_sections_explain_missing_plugins() {
    let (state, _client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/settings")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Maintenance mode"), "{body}");
    assert!(body.contains("HTTP basic auth"), "{body}");
    assert!(
        body.contains("unknown (no probe data yet)"),
        "cold start explains the gate: {body}"
    );
    assert!(
        !body.contains(r#"hx-post="/apps/alpha/maintenance/enable""#),
        "no actions without support: {body}"
    );
}

#[tokio::test]
async fn settings_plugin_sections_expose_actions_when_supported() {
    let (state, _client, _dir) = harness(plugin_probed_client()).await;
    state.capabilities.ensure_loaded().await.expect("probe");
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/settings")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(
        body.contains("While enabled, requests receive a maintenance page"),
        "{body}"
    );
    assert!(
        body.contains(r#"hx-post="/apps/alpha/maintenance/enable""#),
        "{body}"
    );
    assert!(
        body.contains(r#"hx-post="/apps/alpha/maintenance/disable""#),
        "{body}"
    );
    assert!(
        body.contains(r#"hx-post="/apps/alpha/http-auth/enable""#),
        "{body}"
    );
    assert!(
        body.contains(r#"hx-post="/apps/alpha/http-auth/add-user""#),
        "{body}"
    );
    assert!(
        body.contains(r#"hx-post="/apps/alpha/http-auth/remove-user""#),
        "{body}"
    );
    assert!(
        body.contains(r#"hx-post="/apps/alpha/http-auth/add-allowed-ip""#),
        "{body}"
    );
    assert!(
        body.contains(r#"hx-post="/apps/alpha/http-auth/set-allowed-ips""#),
        "{body}"
    );
    assert!(
        body.contains("10.0.0.0/8"),
        "the bypass list renders: {body}"
    );
    assert!(
        body.contains(r#"hx-post="/apps/alpha/http-auth/remove-allowed-ip""#),
        "{body}"
    );
}

#[tokio::test]
async fn hx_maintenance_enable_streams_and_runs_when_supported() {
    let client = plugin_probed_client().stub(
        DokkuCommand::MaintenanceEnable {
            app: app_name("alpha"),
        },
        Ok(DokkuOutput::ok("-----> App is now in maintenance mode\n")),
    );
    let (state, client, _dir) = harness(client).await;
    state.capabilities.ensure_loaded().await.expect("probe");
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/maintenance/enable",
            format!("csrf_token={csrf}"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(
        body.contains("Enabling maintenance mode for alpha"),
        "{body}"
    );

    let (_, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(events.contains(r#""ok":true"#), "{events}");
    assert!(
        client.calls().contains(&DokkuCommand::MaintenanceEnable {
            app: app_name("alpha")
        }),
        "maintenance enable ran"
    );
}

#[tokio::test]
async fn plugin_actions_are_refused_without_the_plugin() {
    let (state, client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/maintenance/enable",
            format!("csrf_token={csrf}"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "gate errors stay 200");
    let body = get_body(resp).await;
    assert!(body.contains("Unavailable on this host"), "{body}");
    assert!(
        client.calls().iter().all(|call| !matches!(
            call,
            DokkuCommand::MaintenanceEnable { .. }
                | DokkuCommand::MaintenanceDisable { .. }
                | DokkuCommand::HttpAuthEnable { .. }
                | DokkuCommand::HttpAuthDisable { .. }
                | DokkuCommand::HttpAuthAddUser { .. }
                | DokkuCommand::HttpAuthRemoveUser { .. }
        )),
        "no plugin commands without support"
    );
}

#[tokio::test]
async fn hx_http_auth_add_user_streams_and_redacts_the_password() {
    let client = plugin_probed_client().stub(
        DokkuCommand::HttpAuthAddUser {
            app: app_name("alpha"),
            username: "alice".into(),
            password: "hunter2".into(),
        },
        Ok(DokkuOutput::ok(
            "-----> Adding user alice with password hunter2\n",
        )),
    );
    let (state, client, _dir) = harness(client).await;
    state.capabilities.ensure_loaded().await.expect("probe");
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/http-auth/add-user",
            format!("csrf_token={csrf}&username=alice&password=hunter2"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    let (_, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(events.contains(r#""ok":true"#), "{events}");
    assert!(
        !events.contains("hunter2"),
        "the password never appears in the stream: {events}"
    );
    assert!(
        events.contains("••••••••"),
        "the password is masked: {events}"
    );
    assert!(
        client.calls().contains(&DokkuCommand::HttpAuthAddUser {
            app: app_name("alpha"),
            username: "alice".into(),
            password: "hunter2".into(),
        }),
        "add-user ran with the password"
    );
}

#[tokio::test]
async fn http_auth_add_user_validates_and_queues_on_the_no_js_path() {
    let client = plugin_probed_client().stub(
        DokkuCommand::HttpAuthAddUser {
            app: app_name("alpha"),
            username: "alice".into(),
            password: "s3cr3t".into(),
        },
        Ok(DokkuOutput::ok("")),
    );
    let (state, client, _dir) = harness(client).await;
    state.capabilities.ensure_loaded().await.expect("probe");
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    // Invalid usernames are rejected before any command.
    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/http-auth/add-user",
            format!("csrf_token={csrf}&username=bad user&password=x"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Username may contain"), "{body}");

    let resp = test::call_service(
        &app,
        form_request(
            "/apps/alpha/http-auth/add-user",
            format!("csrf_token={csrf}&username=alice&password=s3cr3t"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/alpha/settings");
    let cookie = response_cookie(&resp).unwrap_or(cookie);
    wait_for_call(
        &client,
        &DokkuCommand::HttpAuthAddUser {
            app: app_name("alpha"),
            username: "alice".into(),
            password: "s3cr3t".into(),
        },
    )
    .await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/settings")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(
        body.contains("Queued: add basic-auth user alice to alpha."),
        "{body}"
    );

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/activity")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("http-auth.add-user"), "{body}");
}

#[tokio::test]
async fn hx_http_auth_allowed_ips_validate_and_run() {
    let client = plugin_probed_client()
        .stub(
            DokkuCommand::HttpAuthAddAllowedIp {
                app: app_name("alpha"),
                address: "10.0.0.0/8".into(),
            },
            Ok(DokkuOutput::ok(
                "-----> Adding 10.0.0.0/8 to allowed ip list\n",
            )),
        )
        .stub(
            DokkuCommand::HttpAuthSetAllowedIps {
                app: app_name("alpha"),
                addresses: Vec::new(),
            },
            Ok(DokkuOutput::ok(
                "-----> Setting allowed ip list for alpha\n",
            )),
        );
    let (state, client, _dir) = harness(client).await;
    state.capabilities.ensure_loaded().await.expect("probe");
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/http-auth/add-allowed-ip",
            format!("csrf_token={csrf}&address=10.0.0.0%2F8"),
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
        &DokkuCommand::HttpAuthAddAllowedIp {
            app: app_name("alpha"),
            address: "10.0.0.0/8".into(),
        },
    )
    .await;

    // Invalid addresses never reach the host.
    let calls_before = client.calls().len();
    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/http-auth/add-allowed-ip",
            format!("csrf_token={csrf}&address=10.0.0.999"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "htmx errors stay 200");
    let body = get_body(resp).await;
    assert!(body.contains("data-modal-error"), "{body}");
    assert_eq!(client.calls().len(), calls_before, "nothing ran");

    // A blank replace clears the whole list.
    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/http-auth/set-allowed-ips",
            format!("csrf_token={csrf}&addresses="),
        )
        .cookie(cookie)
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    wait_for_call(
        &client,
        &DokkuCommand::HttpAuthSetAllowedIps {
            app: app_name("alpha"),
            addresses: Vec::new(),
        },
    )
    .await;
}
