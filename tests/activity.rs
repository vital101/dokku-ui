mod common;

use actix_web::http::StatusCode;
use actix_web::test;

use common::{
    complete_setup, form_request, get_body, location, seed_user, test_state,
    test_state_with_shared_client,
};

use dokku_ui::dokku::{DokkuOutput, MockClient};
use dokku_ui::domain::AppName;
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::domain::service_plugin::ServicePlugin;
use dokku_ui::storage::runs::{Actor, NewRun, RunOutcome, TargetKind};
use dokku_ui::web::AppState;
use dokku_ui::web::build_app;

fn app_name(name: &str) -> AppName {
    AppName::try_from(name).expect("valid app name")
}

fn apps_report() -> DokkuOutput {
    DokkuOutput::ok(r#"{"app-created-at": "1791023796", "app-locked": "false"}"#)
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
        .stub(
            DokkuCommand::PsRestart {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok("-----> restarting\n-----> done\n")),
        )
}

fn redis_plugin() -> ServicePlugin {
    ServicePlugin::try_from("postgres").expect("postgres plugin")
}

/// Seeds a run directly through the repo (as any container would persist one).
async fn seed_run(
    state: &AppState,
    operation: &str,
    kind: TargetKind,
    subject: &str,
    actor_email: &str,
    ok: Option<bool>,
) {
    let run_id = state
        .action_runs
        .insert_with(&NewRun {
            subject: subject.to_owned(),
            operation: operation.to_owned(),
            target_kind: kind,
            actor: Actor {
                user_id: Some(1),
                email: Some(actor_email.to_owned()),
            },
            parent_run_id: None,
        })
        .await
        .expect("insert run");
    if let Some(ok) = ok {
        state
            .action_runs
            .finish(
                &run_id,
                &RunOutcome {
                    ok,
                    message: "done".to_owned(),
                    redirect: None,
                },
            )
            .await
            .expect("finish run");
    }
}

#[tokio::test]
async fn activity_requires_login() {
    let (state, _dir) = test_state().await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;

    for path in [
        "/activity",
        "/apps/alpha/activity",
        "/services/postgres/cache/activity",
    ] {
        let resp = test::call_service(&app, test::TestRequest::get().uri(path).to_request()).await;
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT, "{path}");
        assert_eq!(location(&resp), "/login", "{path}");
    }
}

#[tokio::test]
async fn global_activity_lists_runs_with_actor_and_outcome() {
    let (state, _client, _dir) = test_state_with_shared_client(seeded_app_client()).await;
    seed_run(
        &state,
        "app.restart",
        TargetKind::App,
        "alpha",
        "ops@example.com",
        Some(true),
    )
    .await;
    seed_run(
        &state,
        "service.create",
        TargetKind::Service,
        "candid",
        "ops@example.com",
        None,
    )
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/activity")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("app.restart"), "{body}");
    assert!(body.contains("service.create"), "{body}");
    assert!(body.contains("ops@example.com"), "{body}");
    assert!(body.contains("succeeded"), "{body}");
    assert!(
        body.contains("running"),
        "an unfinished run shows as running: {body}"
    );
    assert!(body.contains("alpha"), "{body}");
    assert!(body.contains("candid"), "{body}");
}

#[tokio::test]
async fn global_activity_lists_newest_first() {
    let (state, _client, _dir) = test_state_with_shared_client(seeded_app_client()).await;
    seed_run(
        &state,
        "app.restart",
        TargetKind::App,
        "alpha",
        "ops@example.com",
        Some(true),
    )
    .await;
    sqlx::query("UPDATE action_runs SET created_at = ? WHERE operation = 'app.restart'")
        .bind(1_700_000_000)
        .execute(&state.db)
        .await
        .expect("backdate");
    seed_run(
        &state,
        "service.create",
        TargetKind::Service,
        "candid",
        "ops@example.com",
        None,
    )
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/activity")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    let restart_at = body.find("app.restart").expect("restart row");
    let create_at = body.find("service.create").expect("create row");
    assert!(create_at < restart_at, "newest first: {body}");
}

#[tokio::test]
async fn global_activity_filters_by_user() {
    let (state, _client, _dir) = test_state_with_shared_client(seeded_app_client()).await;
    seed_run(
        &state,
        "app.restart",
        TargetKind::App,
        "alpha",
        "ops@example.com",
        Some(true),
    )
    .await;
    seed_run(
        &state,
        "app.stop",
        TargetKind::App,
        "alpha",
        "other@example.com",
        Some(true),
    )
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/activity?user=other@example.com")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("app.stop"), "{body}");
    assert!(
        !body.contains("app.restart"),
        "filtered to the requested actor: {body}"
    );
}

#[tokio::test]
async fn global_activity_empty_state() {
    let (state, _dir) = test_state().await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/activity")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("No activity yet"), "{body}");
}

#[tokio::test]
async fn app_activity_shows_only_that_apps_runs() {
    let (state, _client, _dir) = test_state_with_shared_client(seeded_app_client()).await;
    seed_run(
        &state,
        "app.restart",
        TargetKind::App,
        "alpha",
        "ops@example.com",
        Some(true),
    )
    .await;
    seed_run(
        &state,
        "service.create",
        TargetKind::Service,
        "candid",
        "ops@example.com",
        None,
    )
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/alpha/activity")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("app.restart"), "{body}");
    assert!(
        !body.contains("service.create"),
        "other targets stay off the app tab: {body}"
    );
    assert!(
        body.contains(r#"href="/apps/alpha/activity""#),
        "tab nav present: {body}"
    );
}

#[tokio::test]
async fn app_activity_404s_unknown_apps() {
    let (state, _client, _dir) = test_state_with_shared_client(seeded_app_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/ghost/activity")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn service_activity_shows_only_that_services_runs() {
    let client = seeded_app_client().stub(
        DokkuCommand::ServiceList {
            plugin: redis_plugin(),
        },
        Ok(DokkuOutput::ok("=====> PostgreSQL services\ncandid")),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    seed_run(
        &state,
        "service.stop",
        TargetKind::Service,
        "candid",
        "ops@example.com",
        Some(true),
    )
    .await;
    seed_run(
        &state,
        "app.restart",
        TargetKind::App,
        "alpha",
        "ops@example.com",
        None,
    )
    .await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/postgres/candid/activity")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("service.stop"), "{body}");
    assert!(
        !body.contains("app.restart"),
        "app runs stay off the service tab: {body}"
    );
    assert!(
        body.contains(r#"href="/services/postgres/candid/activity""#),
        "{body}"
    );
}

#[tokio::test]
async fn service_activity_404s_unlisted_services() {
    let client = seeded_app_client().stub(
        DokkuCommand::ServiceList {
            plugin: redis_plugin(),
        },
        Ok(DokkuOutput::ok("=====> PostgreSQL services\ncandid")),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/postgres/ghost/activity")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn non_htmx_actions_record_audit_runs() {
    let (state, _client, _dir) = test_state_with_shared_client(seeded_app_client()).await;
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
        common::extract_csrf(&get_body(resp).await)
    };
    let resp = test::call_service(
        &app,
        form_request("/apps/alpha/restart", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/activity")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(
        body.contains("app.restart"),
        "no-JS actions land in the audit trail: {body}"
    );
    assert!(
        body.contains("admin@example.com"),
        "attributed to the session user: {body}"
    );
}
