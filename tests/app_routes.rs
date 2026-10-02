mod common;

use std::collections::HashMap;
use std::sync::Arc;

use actix_web::http::StatusCode;
use actix_web::test;

use common::{
    complete_setup, extract_csrf, form_request, get_body, location, response_cookie, seed_user,
    test_state, test_state_with_client,
};

use dokku_ui::dokku::{DokkuClient, DokkuError, DokkuOutput, MockClient};
use dokku_ui::domain::AppName;
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::settings::Settings;
use dokku_ui::storage;
use dokku_ui::web::{AppState, build_app};

fn app_name(name: &str) -> AppName {
    AppName::try_from(name).expect("valid app name")
}

const CONFIG_FIXTURE: &str = include_str!("fixtures/config_show.txt");
const LOGS_FIXTURE: &str = include_str!("fixtures/logs.txt");

fn apps_report() -> DokkuOutput {
    DokkuOutput::ok(
        r#"{"app created at": "2026-01-01T00:00:00Z", "app locked": "false"}"#.to_owned(),
    )
}

fn ps_report(running: bool, deployed: bool, processes: i64) -> DokkuOutput {
    DokkuOutput::ok(format!(
        r#"{{"deployed": "{deployed}", "running": "{running}", "processes": "{processes}"}}"#
    ))
}

fn seeded_app_client() -> MockClient {
    MockClient::new()
        .stub(DokkuCommand::AppsList, Ok(DokkuOutput::ok(r#"["alpha"]"#)))
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
    (
        AppState {
            db: pool,
            settings,
            dokku,
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

#[tokio::test]
async fn app_routes_redirect_to_login_when_unauthenticated() {
    let (state, _dir) = test_state().await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;

    for path in [
        "/apps/new",
        "/apps/alpha",
        "/apps/alpha/delete",
        "/apps/alpha/config",
        "/apps/alpha/logs",
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
async fn show_renders_app_overview() {
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
    assert!(body.contains("Running"));
    assert!(body.contains(r#">2</p>"#), "process count");
    assert!(body.contains("Yes"), "deployed flag");
    assert!(body.contains("2026-01-01T00:00:00Z"), "created at");
    assert!(body.contains("no"), "locked label");
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
async fn destroy_with_matching_echo_destroys_and_redirects_home() {
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
    assert!(
        client.calls().contains(&destroy),
        "AppsDestroy called with force"
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
    assert!(body.contains("App &#39;alpha&#39; destroyed."));
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
async fn destroy_error_flashes_and_returns_to_confirm() {
    let (state, _dir) = test_state_with_client(MockClient::new().stub(
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
    assert_eq!(location(&resp), "/apps/alpha/delete");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/delete")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Failed to destroy app"));
    assert!(body.contains("gone"));
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
async fn actions_succeed_flash_and_redirect_to_show() {
    for (path, command, flash) in [
        (
            "/apps/alpha/start",
            DokkuCommand::PsStart {
                app: app_name("alpha"),
            },
            "App &#39;alpha&#39; started.",
        ),
        (
            "/apps/alpha/stop",
            DokkuCommand::PsStop {
                app: app_name("alpha"),
            },
            "App &#39;alpha&#39; stopped.",
        ),
        (
            "/apps/alpha/restart",
            DokkuCommand::PsRestart {
                app: app_name("alpha"),
            },
            "App &#39;alpha&#39; restarted.",
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

        assert!(client.calls().contains(&command), "{path} called dokku");

        let resp = test::call_service(
            &app,
            test::TestRequest::get()
                .uri("/apps/alpha")
                .cookie(cookie)
                .to_request(),
        )
        .await;
        let body = get_body(resp).await;
        assert!(body.contains(flash), "{path} flash");
    }
}

#[tokio::test]
async fn action_error_flashes_stderr_and_redirects_to_show() {
    let (state, _dir) = test_state_with_client(seeded_app_client().stub(
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

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Failed to start app"));
    assert!(body.contains("no such app"));
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
}

#[tokio::test]
async fn config_renders_env_vars_and_tabs() {
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
            .uri("/apps/alpha/config")
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
    assert!(body.contains("Values are masked"));
    assert!(body.contains(r#"href="/apps/alpha/config""#), "config tab");
    assert!(body.contains(r#"href="/apps/alpha/logs""#), "logs tab");
    assert!(body.contains(r#"href="/apps/alpha""#), "overview tab");
    assert!(
        body.contains("border-emerald-500"),
        "active tab highlighted"
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
            .uri("/apps/alpha/config")
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
async fn config_fetch_error_renders_502() {
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
            .uri("/apps/alpha/config")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body = get_body(resp).await;
    assert!(body.contains("boom"));
}

#[tokio::test]
async fn logs_defaults_to_200_lines_and_strips_ansi() {
    let (state, client, _dir) = harness(seeded_app_client().stub(
        DokkuCommand::Logs {
            app: app_name("alpha"),
            num_lines: 200,
        },
        Ok(DokkuOutput::ok(LOGS_FIXTURE)),
    ))
    .await;
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

    assert!(body.contains("listening on 0.0.0.0:8080"));
    assert!(body.contains("GET /healthz 200"));
    assert!(body.contains("ERROR something failed"));
    assert!(!body.contains('\x1b'), "ANSI escapes stripped");
    assert!(client.calls().contains(&DokkuCommand::Logs {
        app: app_name("alpha"),
        num_lines: 200
    }));
    assert!(body.contains("Lines"));
}

#[tokio::test]
async fn logs_clamps_lines_parameter() {
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
        };
        let (state, client, _dir) =
            harness(seeded_app_client().stub(command.clone(), Ok(DokkuOutput::ok("")))).await;
        let app = test::init_service(build_app(state)).await;
        let cookie = complete_setup(&app).await;

        let resp = test::call_service(
            &app,
            test::TestRequest::get()
                .uri(&format!("/apps/alpha/logs{query}"))
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
        },
        Ok(DokkuOutput::ok("")),
    ))
    .await;
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
    let body = get_body(resp).await;
    assert!(body.contains("No log lines yet."));
}

#[tokio::test]
async fn logs_fetch_error_renders_502() {
    let (state, _dir) = test_state_with_client(seeded_app_client().stub(
        DokkuCommand::Logs {
            app: app_name("alpha"),
            num_lines: 200,
        },
        Err(exit_error(1, "boom")),
    ))
    .await;
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
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body = get_body(resp).await;
    assert!(body.contains("boom"));
}
