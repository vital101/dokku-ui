mod common;

use std::sync::Arc;

use actix_web::http::StatusCode;
use actix_web::test;

use common::{
    complete_setup, extract_csrf, form_request, get_body, location, run_url,
    test_state_with_shared_client,
};

use dokku_ui::dokku::{DokkuError, DokkuOutput, MockClient};
use dokku_ui::domain::AppName;
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::domain::git::GitBuildMode;
use dokku_ui::web::{AppState, build_app};

const GIT_REPORT: &str = include_str!("fixtures/git_report.txt");
const GIT_PUBLIC_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILnLJJXqj2pzpcitocD+onmzABt0V33gxiRh1K0IS01D dokku@host\n";

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
            DokkuCommand::GitReport {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok(GIT_REPORT)),
        )
        .stub(
            DokkuCommand::GitPublicKey,
            Ok(DokkuOutput::ok(GIT_PUBLIC_KEY)),
        )
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
            .uri("/apps/alpha/deploy")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    extract_csrf(&get_body(resp).await)
}

#[tokio::test]
async fn deploy_routes_require_login() {
    let (state, _client, _dir) = test_state_with_shared_client(seeded_app_client()).await;
    common::seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;

    for path in ["/apps/alpha/deploy", "/apps/alpha/partials/deploy"] {
        let resp = test::call_service(&app, test::TestRequest::get().uri(path).to_request()).await;
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT, "{path}");
        assert_eq!(location(&resp), "/login", "{path}");
    }
}

#[tokio::test]
async fn deploy_partial_renders_push_url_key_and_git_summary() {
    let (state, _client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/deploy")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(
        body.contains("dokku@host.docker.internal:alpha"),
        "scp-style push URL: {body}"
    );
    assert!(body.contains("ssh-ed25519"), "deploy key rendered: {body}");
    assert!(body.contains("main"), "deploy branch rendered: {body}");
    assert!(
        body.contains(r#"hx-post="/apps/alpha/deploy/branch""#),
        "{body}"
    );
}

#[tokio::test]
async fn deploy_partial_explains_a_missing_deploy_key() {
    let client = seeded_app_client().stub(
        DokkuCommand::GitPublicKey,
        Err(DokkuError::Exit {
            code: 1,
            stderr: String::new(),
        }),
    );
    let (state, _client, _dir) = harness(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/deploy")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("No deploy key is configured"), "{body}");
    assert!(body.contains("git:generate-deploy-key"), "{body}");
}

#[tokio::test]
async fn hx_deploy_branch_set_streams_and_clears() {
    let client = seeded_app_client()
        .stub(
            DokkuCommand::GitSet {
                app: app_name("alpha"),
                property: "deploy-branch".into(),
                value: Some("develop".into()),
            },
            Ok(DokkuOutput::ok("=====> Setting deploy-branch to develop\n")),
        )
        .stub(
            DokkuCommand::GitSet {
                app: app_name("alpha"),
                property: "deploy-branch".into(),
                value: None,
            },
            Ok(DokkuOutput::ok("=====> Unsetting deploy-branch\n")),
        );
    let (state, client, _dir) = harness(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    for value in ["develop", ""] {
        let resp = test::call_service(
            &app,
            hx_form_request(
                "/apps/alpha/deploy/branch",
                format!("csrf_token={csrf}&deploy_branch={value}"),
            )
            .cookie(cookie.clone())
            .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let html = get_body(resp).await;
        assert!(html.contains("Updating deploy branch"), "{html}");
        assert!(
            html.contains(r#"data-refresh="/apps/alpha/partials/deploy""#),
            "{html}"
        );
        let (_, events) = sse_events(&app, &run_url(&html), &cookie).await;
        assert!(events.contains(r#""ok":true"#), "{value:?}: {events}");
    }

    assert!(
        client.calls().contains(&DokkuCommand::GitSet {
            app: app_name("alpha"),
            property: "deploy-branch".into(),
            value: Some("develop".into()),
        }),
        "set ran"
    );
    assert!(
        client.calls().contains(&DokkuCommand::GitSet {
            app: app_name("alpha"),
            property: "deploy-branch".into(),
            value: None,
        }),
        "clear ran"
    );
}

#[tokio::test]
async fn deploy_branch_rejects_invalid_input_without_dokku() {
    let (state, client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/deploy/branch",
            format!("csrf_token={csrf}&deploy_branch=it%27s"),
        )
        .cookie(cookie)
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("letters, digits"), "{body}");
    assert!(
        client
            .calls()
            .iter()
            .all(|call| !matches!(call, DokkuCommand::GitSet { .. })),
        "no git:set for rejected input"
    );
}

#[tokio::test]
async fn hx_sync_streams_with_flag_ref_and_audit() {
    let client = seeded_app_client().stub(
        DokkuCommand::GitSync {
            app: app_name("alpha"),
            repo: "https://github.com/org/repo.git".into(),
            git_ref: Some("main".into()),
            build_mode: GitBuildMode::Build,
        },
        Ok(DokkuOutput::ok("-----> Cloning alpha\n")),
    );
    let (state, client, _dir) = harness(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/apps/alpha/deploy/sync",
            format!(
                "csrf_token={csrf}&repo=https%3A%2F%2Fgithub.com%2Forg%2Frepo.git&git_ref=main&build_mode=build"
            ),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let html = get_body(resp).await;
    assert!(html.contains("Syncing from git for alpha"), "{html}");
    assert!(
        html.contains(r#"data-refresh="/apps/alpha/partials/deploy""#),
        "{html}"
    );
    let (_, events) = sse_events(&app, &run_url(&html), &cookie).await;
    assert!(events.contains(r#""ok":true"#), "{events}");
    assert!(
        client.calls().contains(&DokkuCommand::GitSync {
            app: app_name("alpha"),
            repo: "https://github.com/org/repo.git".into(),
            git_ref: Some("main".into()),
            build_mode: GitBuildMode::Build,
        }),
        "sync ran with the build flag"
    );
}

#[tokio::test]
async fn hx_image_and_archive_deploys_stream() {
    let client = seeded_app_client()
        .stub(
            DokkuCommand::GitFromImage {
                app: app_name("alpha"),
                image: "ghcr.io/org/app:v1".into(),
            },
            Ok(DokkuOutput::ok("")),
        )
        .stub(
            DokkuCommand::GitFromArchive {
                app: app_name("alpha"),
                archive_url: "https://example.com/app.tar.gz".into(),
            },
            Ok(DokkuOutput::ok("")),
        );
    let (state, client, _dir) = harness(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    for (path, body) in [
        (
            "/apps/alpha/deploy/from-image",
            format!("csrf_token={csrf}&image=ghcr.io%2Forg%2Fapp%3Av1"),
        ),
        (
            "/apps/alpha/deploy/from-archive",
            format!("csrf_token={csrf}&archive_url=https%3A%2F%2Fexample.com%2Fapp.tar.gz"),
        ),
    ] {
        let resp = test::call_service(
            &app,
            hx_form_request(path, body)
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let html = get_body(resp).await;
        let (_, events) = sse_events(&app, &run_url(&html), &cookie).await;
        assert!(events.contains(r#""ok":true"#), "{path}: {events}");
    }

    assert!(
        client.calls().contains(&DokkuCommand::GitFromImage {
            app: app_name("alpha"),
            image: "ghcr.io/org/app:v1".into(),
        }),
        "image deploy ran"
    );
    assert!(
        client.calls().contains(&DokkuCommand::GitFromArchive {
            app: app_name("alpha"),
            archive_url: "https://example.com/app.tar.gz".into(),
        }),
        "archive deploy ran"
    );
}

#[tokio::test]
async fn deploy_actions_reject_invalid_input_without_dokku() {
    let (state, client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    for (path, body, expected) in [
        (
            "/apps/alpha/deploy/sync",
            format!("csrf_token={csrf}&repo=github.com%2Forg%2Frepo&build_mode=build"),
            "https://",
        ),
        (
            "/apps/alpha/deploy/sync",
            format!(
                "csrf_token={csrf}&repo=https%3A%2F%2Fgithub.com%2Forg%2Frepo.git&git_ref=it%27s&build_mode=build"
            ),
            "Git ref",
        ),
        (
            "/apps/alpha/deploy/from-image",
            format!("csrf_token={csrf}&image=it%27s"),
            "registry reference",
        ),
        (
            "/apps/alpha/deploy/from-archive",
            format!("csrf_token={csrf}&archive_url=ftp%3A%2F%2Fexample.com%2Fapp.tar.gz"),
            "http(s) URL",
        ),
    ] {
        let resp = test::call_service(
            &app,
            hx_form_request(path, body)
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let html = get_body(resp).await;
        assert!(html.contains(expected), "{path}: {html}");
    }
    assert!(
        client.calls().iter().all(|call| !matches!(
            call,
            DokkuCommand::GitSync { .. }
                | DokkuCommand::GitFromImage { .. }
                | DokkuCommand::GitFromArchive { .. }
        )),
        "no deploy commands for rejected input"
    );
}

#[tokio::test]
async fn failed_logs_partial_renders_empty_and_populated_states() {
    let client = seeded_app_client().stub(
        DokkuCommand::LogsFailed {
            app: app_name("alpha"),
        },
        Ok(DokkuOutput::ok(include_str!("fixtures/logs_failed.txt"))),
    );
    let (state, _client, _dir) = harness(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/failed-logs")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("No failed deploy logs."), "{body}");

    let client = seeded_app_client().stub(
        DokkuCommand::LogsFailed {
            app: app_name("alpha"),
        },
        Ok(DokkuOutput::ok(include_str!(
            "fixtures/logs_failed_populated.txt"
        ))),
    );
    let (state, _client, _dir) = harness(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/failed-logs")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("pre-receive hook declined"), "{body}");
    assert!(
        !body.contains("failed deploy logs"),
        "the dokku banner is not rendered: {body}"
    );
}

#[tokio::test]
async fn webhook_config_card_lives_on_the_deploy_tab() {
    let (state, _client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/partials/deploy")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("GitHub webhook"), "{body}");
    assert!(body.contains("/webhooks/github/"), "{body}");
    assert!(body.contains(r#"hx-post="/apps/alpha/webhooks""#), "{body}");
    assert!(
        body.contains(r#"hx-get="/apps/alpha/partials/failed-logs""#),
        "{body}"
    );
}

#[tokio::test]
async fn non_htmx_sync_queues_flash_and_is_audited() {
    let client = seeded_app_client().stub(
        DokkuCommand::GitSync {
            app: app_name("alpha"),
            repo: "https://github.com/org/repo.git".into(),
            git_ref: None,
            build_mode: GitBuildMode::NoBuild,
        },
        Ok(DokkuOutput::ok("")),
    );
    let (state, _client, _dir) = harness(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        form_request(
            "/apps/alpha/deploy/sync",
            format!(
                "csrf_token={csrf}&repo=https%3A%2F%2Fgithub.com%2Forg%2Frepo.git&build_mode=no-build"
            ),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/alpha/deploy");
    let cookie = common::response_cookie(&resp).unwrap_or(cookie);

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/deploy")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Queued: sync from git for alpha."), "{body}");

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/activity")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("git.sync"), "{body}");
}

#[tokio::test]
async fn non_htmx_deploy_branch_queues_flashes_and_is_audited() {
    let client = seeded_app_client().stub(
        DokkuCommand::GitSet {
            app: app_name("alpha"),
            property: "deploy-branch".into(),
            value: Some("main".into()),
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
            "/apps/alpha/deploy/branch",
            format!("csrf_token={csrf}&deploy_branch=main"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/alpha/deploy");
    let cookie = common::response_cookie(&resp).unwrap_or(cookie);
    wait_for_call(
        &client,
        &DokkuCommand::GitSet {
            app: app_name("alpha"),
            property: "deploy-branch".into(),
            value: Some("main".into()),
        },
    )
    .await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/deploy")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(
        body.contains("Queued: update deploy branch for alpha."),
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
    assert!(body.contains("git.set"), "{body}");
}
