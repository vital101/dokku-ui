mod common;

use actix_web::http::StatusCode;
use actix_web::test;

use common::{
    complete_setup, extract_csrf, form_request, get_body, location, response_cookie, test_state,
    test_state_with_client,
};

use dokku_ui::dokku::{DokkuOutput, MockClient};
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::domain::{AppName, InstanceSettings};
use dokku_ui::storage::instance_settings::{InstanceSettingsRepo, SqliteInstanceSettingsRepo};
use dokku_ui::storage::runs::{Actor, NewRun, TargetKind};
use dokku_ui::web::build_app;

fn app(name: &str) -> AppName {
    AppName::try_from(name).expect("valid app name")
}

fn seeded_dashboard() -> MockClient {
    MockClient::new()
        .stub(
            DokkuCommand::AppsList,
            Ok(DokkuOutput::ok("=====> My Apps\nalpha\nbeta")),
        )
        .stub(
            DokkuCommand::PsReport { app: app("alpha") },
            Ok(DokkuOutput::ok(
                r#"{"deployed": "true", "running": "true", "processes": "1"}"#,
            )),
        )
        .stub(
            DokkuCommand::PsReport { app: app("beta") },
            Ok(DokkuOutput::ok(
                r#"{"deployed": "true", "running": "true", "processes": "1"}"#,
            )),
        )
}

async fn save_settings(state: &dokku_ui::web::AppState, settings: &InstanceSettings) {
    SqliteInstanceSettingsRepo::new(state.db.clone())
        .save(settings)
        .await
        .expect("save settings");
}

#[tokio::test]
async fn dashboard_hides_filtered_apps_and_badges_destroys() {
    let (state, _dir) = test_state_with_client(seeded_dashboard()).await;
    save_settings(
        &state,
        &InstanceSettings::parse(None, None, Some("beta"), None, None).expect("filter"),
    )
    .await;
    let _ = state
        .action_runs
        .insert_with(&NewRun {
            subject: "alpha".to_owned(),
            operation: "app.destroy".to_owned(),
            target_kind: TargetKind::App,
            actor: Actor {
                user_id: None,
                email: None,
            },
            parent_run_id: None,
        })
        .await
        .expect("run");

    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("alpha"), "{body}");
    assert!(!body.contains("beta"), "filtered app is hidden: {body}");
    assert!(body.contains("deleting"), "destroy badge: {body}");
}

#[tokio::test]
async fn login_page_shows_the_configured_banner() {
    let (state, _dir) = test_state().await;
    common::seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    save_settings(
        &state,
        &InstanceSettings::parse(
            None,
            Some("Scheduled maintenance at 22:00 UTC"),
            None,
            None,
            None,
        )
        .expect("banner"),
    )
    .await;
    let app = test::init_service(build_app(state)).await;

    let resp = test::call_service(&app, test::TestRequest::get().uri("/login").to_request()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(
        body.contains("Scheduled maintenance at 22:00 UTC"),
        "{body}"
    );
}

#[tokio::test]
async fn admin_saves_and_reloads_instance_settings() {
    let (state, _dir) = test_state_with_client(seeded_dashboard()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/settings")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("Instance settings"));
    let csrf = extract_csrf(&body);

    let resp = test::call_service(
        &app,
        form_request(
            "/settings",
            format!(
                "csrf_token={csrf}&public_url=https%3A%2F%2Fui.example.com&login_banner=Hello&app_filter=beta&activity_ttl=86400&run_log_ttl=3600"
            ),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/settings");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/settings")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("https://ui.example.com"), "{body}");
    assert!(body.contains("Instance settings saved."), "{body}");

    let resp = test::call_service(
        &app,
        form_request(
            "/settings",
            format!("csrf_token={csrf}&public_url=ftp%3A%2F%2Fbad&login_banner=&app_filter=&activity_ttl=&run_log_ttl="),
        )
        .cookie(cookie)
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("public URL must start"), "{body}");
}

#[tokio::test]
async fn settings_route_requires_admin() {
    let (state, _dir) = test_state().await;
    common::seed_user_with_role(
        &state,
        "viewer@example.com",
        "correct-horse-battery",
        dokku_ui::auth::rbac::Role::Viewer,
    )
    .await;
    let app = test::init_service(build_app(state)).await;
    let viewer = common::login(&app, "viewer@example.com", "correct-horse-battery").await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/settings")
            .cookie(viewer.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = test::call_service(
        &app,
        form_request("/settings", "csrf_token=x".to_owned())
            .cookie(viewer)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
