mod common;

use std::sync::Arc;

use actix_web::http::StatusCode;
use actix_web::test;

use common::{
    complete_setup, extract_csrf, form_request, get_body, location, response_cookie,
    test_state_with_shared_client,
};

use dokku_ui::dokku::{DokkuOutput, MockClient};
use dokku_ui::domain::AppName;
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::storage::webhooks::Webhook;
use dokku_ui::web::{AppState, build_app};

const SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const REPO: &str = "https://github.com/org/repo.git";
const BRANCH: &str = "main";

fn app_name(name: &str) -> AppName {
    AppName::try_from(name).expect("valid app name")
}

fn sign(secret: &str, body: &[u8]) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
    let tag = ring::hmac::sign(&key, body);
    format!(
        "sha256={}",
        tag.as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

fn push_payload(repo: &str, branch: &str) -> String {
    format!(
        r#"{{"ref":"refs/heads/{branch}","repository":{{"full_name":"{repo}","clone_url":"https://github.com/{repo}.git","ssh_url":"git@github.com:{repo}.git","html_url":"https://github.com/{repo}"}}}}"#
    )
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
            Ok(DokkuOutput::ok(include_str!("fixtures/git_report.txt"))),
        )
        .stub(DokkuCommand::GitPublicKey, Ok(DokkuOutput::ok("")))
}

async fn harness(client: MockClient) -> (AppState, Arc<MockClient>, tempfile::TempDir) {
    test_state_with_shared_client(client).await
}

async fn seed_webhook(state: &AppState, enabled: bool) {
    state
        .webhooks
        .upsert(&Webhook {
            app: "alpha".to_owned(),
            repo: REPO.to_owned(),
            branch: BRANCH.to_owned(),
            build_mode: "build".to_owned(),
            secret: SECRET.to_owned(),
            enabled,
        })
        .await
        .expect("seed webhook");
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
            .uri("/apps/alpha/deploy")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    extract_csrf(&get_body(resp).await)
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

#[tokio::test]
async fn webhook_config_routes_require_login() {
    let (state, _client, _dir) = harness(seeded_app_client()).await;
    common::seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;

    for path in [
        "/apps/alpha/webhooks",
        "/apps/alpha/webhooks/remove",
        "/apps/alpha/webhooks/reveal",
    ] {
        let resp = test::call_service(
            &app,
            test::TestRequest::post()
                .uri(path)
                .set_payload("x=1")
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT, "{path}");
        assert_eq!(location(&resp), "/login", "{path}");
    }
}

#[tokio::test]
async fn webhook_config_requires_csrf() {
    let (state, _client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        form_request(
            "/apps/alpha/webhooks",
            "repo=https%3A%2F%2Fgithub.com%2Forg%2Frepo.git&branch=main&build_mode=build"
                .to_owned(),
        )
        .cookie(cookie)
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn save_generates_a_secret_once_and_validates_input() {
    let (state, _client, _dir) = harness(seeded_app_client()).await;
    let pool = state.db.clone();
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    // First save creates the webhook and generates a secret.
    let body = format!(
        "csrf_token={csrf}&repo=https%3A%2F%2Fgithub.com%2Forg%2Frepo.git&branch=main&build_mode=build-if-changes&enabled=on"
    );
    let resp = test::call_service(
        &app,
        hx_form_request("/apps/alpha/webhooks", body)
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("HX-Redirect")
            .and_then(|value| value.to_str().ok()),
        Some("/apps/alpha/deploy")
    );
    let webhook = sqlx::query_as::<_, (String, String, String, String, bool)>(
        "SELECT repo, branch, build_mode, secret, enabled FROM app_webhooks WHERE app = 'alpha'",
    )
    .fetch_one(&pool)
    .await
    .expect("stored webhook");
    assert_eq!(webhook.0, REPO);
    assert_eq!(webhook.1, "main");
    assert_eq!(webhook.2, "build-if-changes");
    assert!(webhook.4, "enabled");
    assert_eq!(webhook.3.len(), 64);
    let secret = webhook.3;

    // Editing config keeps the secret and can disable the webhook.
    let body = format!(
        "csrf_token={csrf}&repo=https%3A%2F%2Fgithub.com%2Forg%2Frepo.git&branch=develop&build_mode=no-build"
    );
    let resp = test::call_service(
        &app,
        hx_form_request("/apps/alpha/webhooks", body)
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let row = sqlx::query_as::<_, (String, String, bool)>(
        "SELECT branch, secret, enabled FROM app_webhooks WHERE app = 'alpha'",
    )
    .fetch_one(&pool)
    .await
    .expect("stored webhook");
    assert_eq!(row.0, "develop");
    assert_eq!(row.1, secret, "secret survives edits");
    assert!(!row.2, "unchecked box disables");

    // Invalid remotes are rejected with an htmx error card.
    let body = format!(
        "csrf_token={csrf}&repo=github.com%2Forg%2Frepo&branch=main&build_mode=build&enabled=on"
    );
    let resp = test::call_service(
        &app,
        hx_form_request("/apps/alpha/webhooks", body)
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let html = get_body(resp).await;
    assert!(html.contains("https://"), "{html}");
}

#[tokio::test]
async fn reveal_requires_reauth_then_shows_the_secret_no_store() {
    let (state, _client, _dir) = harness(seeded_app_client()).await;
    seed_webhook(&state, true).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        form_request("/apps/alpha/webhooks/reveal", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/reauth?next=/apps/alpha/deploy");

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/reauth?next=/apps/alpha/deploy")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let reauth_csrf = extract_csrf(&get_body(resp).await);
    let resp = test::call_service(
        &app,
        form_request(
            "/reauth",
            format!(
                "csrf_token={reauth_csrf}&password=correct-horse-battery&next=%2Fapps%2Falpha%2Fdeploy"
            ),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let resp = test::call_service(
        &app,
        form_request("/apps/alpha/webhooks/reveal", format!("csrf_token={csrf}"))
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
        "a revealed secret is never cached"
    );
    let body = get_body(resp).await;
    assert!(body.contains(SECRET), "{body}");
}

#[tokio::test]
async fn public_route_is_hmac_gated_and_does_not_redirect_to_login() {
    let (state, _client, _dir) = harness(seeded_app_client()).await;
    seed_webhook(&state, true).await;
    let app = test::init_service(build_app(state)).await;

    let body = push_payload("org/repo", BRANCH);
    // No signature and a tampered signature are both rejected, with no login
    // redirect (the route is public).
    for signature in [None, Some("sha256=deadbeef")] {
        let mut req = test::TestRequest::post()
            .uri("/webhooks/github/alpha")
            .insert_header(("X-GitHub-Event", "push"))
            .set_payload(body.clone());
        if let Some(signature) = signature {
            req = req.insert_header(("X-Hub-Signature-256", signature));
        }
        let resp = test::call_service(&app, req.to_request()).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{signature:?}");
    }

    let resp = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/webhooks/github/alpha")
            .insert_header(("X-Hub-Signature-256", sign(SECRET, body.as_bytes())))
            .insert_header(("X-GitHub-Event", "ping"))
            .set_payload(body)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(get_body(resp).await, "pong");
}

#[tokio::test]
async fn unknown_or_disabled_apps_are_404() {
    let (state, _client, _dir) = harness(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/webhooks/github/unknown")
            .set_payload("{}")
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let (state, _client, _dir) = harness(seeded_app_client()).await;
    seed_webhook(&state, false).await;
    let app = test::init_service(build_app(state)).await;
    let body = push_payload("org/repo", BRANCH);
    let resp = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/webhooks/github/alpha")
            .insert_header(("X-Hub-Signature-256", sign(SECRET, body.as_bytes())))
            .insert_header(("X-GitHub-Event", "push"))
            .set_payload(body)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn push_filters_repo_and_branch_then_enqueues_sync() {
    let (state, client, _dir) = harness(seeded_app_client()).await;
    let pool = state.db.clone();
    seed_webhook(&state, true).await;
    let app = test::init_service(build_app(state)).await;

    // Other repositories and branches are acknowledged but ignored.
    for payload in [
        push_payload("other/repo", BRANCH),
        push_payload("org/repo", "develop"),
    ] {
        let resp = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/webhooks/github/alpha")
                .insert_header(("X-Hub-Signature-256", sign(SECRET, payload.as_bytes())))
                .insert_header(("X-GitHub-Event", "push"))
                .set_payload(payload)
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(get_body(resp).await, "ignored");
    }

    let payload = push_payload("org/repo", BRANCH);
    let resp = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/webhooks/github/alpha")
            .insert_header(("X-Hub-Signature-256", sign(SECRET, payload.as_bytes())))
            .insert_header(("X-GitHub-Event", "push"))
            .set_payload(payload)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    let expected = DokkuCommand::GitSync {
        app: app_name("alpha"),
        repo: REPO.to_owned(),
        git_ref: Some(BRANCH.to_owned()),
        build_mode: dokku_ui::domain::git::GitBuildMode::Build,
    };
    wait_for_call(&client, &expected).await;

    // The run is attributed to the system actor, and no run line carries the
    // secret.
    let actor = sqlx::query_as::<_, (Option<i64>, Option<String>)>(
        "SELECT actor_user_id, actor_email FROM action_runs WHERE operation = 'git.sync'",
    )
    .fetch_one(&pool)
    .await
    .expect("audited run");
    assert_eq!(actor.0, None);
    assert_eq!(actor.1.as_deref(), Some("github-webhook"));
    let leaked: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM action_run_lines WHERE line LIKE ?")
        .bind(format!("%{SECRET}%"))
        .fetch_one(&pool)
        .await
        .expect("line scan");
    assert_eq!(leaked, 0, "the webhook secret never reaches run lines");
}

#[tokio::test]
async fn oversized_and_unknown_events_are_handled() {
    let (state, _client, _dir) = harness(seeded_app_client()).await;
    seed_webhook(&state, true).await;
    let app = test::init_service(build_app(state)).await;

    let oversized = vec![b'x'; 256 * 1024 + 1];
    let resp = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/webhooks/github/alpha")
            .set_payload(oversized)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let body = push_payload("org/repo", BRANCH);
    let resp = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/webhooks/github/alpha")
            .insert_header(("X-Hub-Signature-256", sign(SECRET, body.as_bytes())))
            .insert_header(("X-GitHub-Event", "issues"))
            .set_payload(body)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(get_body(resp).await, "ignored");
}
