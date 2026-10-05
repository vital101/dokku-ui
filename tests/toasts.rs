mod common;

use actix_web::http::StatusCode;
use actix_web::test;

use common::{
    complete_setup, form_request, get_body, location, seed_user, states_over_shared_db, test_state,
    test_state_with_client,
};

use dokku_ui::dokku::{DokkuOutput, MockClient};
use dokku_ui::domain::AppName;
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::storage::runs::{Actor, NewRun, RunOutcome, TargetKind};
use dokku_ui::web::AppState;
use dokku_ui::web::build_app;

fn app_name(name: &str) -> AppName {
    AppName::try_from(name).expect("valid app name")
}

fn seeded_client() -> MockClient {
    MockClient::new()
        .stub(
            DokkuCommand::AppsList,
            Ok(DokkuOutput::ok("=====> My Apps\nalpha")),
        )
        .stub(
            DokkuCommand::PsRestart {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok("-----> restarting\n-----> done\n")),
        )
}

/// Seeds a run attributed to the session user (id 1 after setup).
async fn seed_run(state: &AppState, operation: &str, ok: Option<bool>) {
    let run_id = state
        .action_runs
        .insert_with(&NewRun {
            subject: "alpha".to_owned(),
            operation: operation.to_owned(),
            target_kind: TargetKind::App,
            actor: Actor {
                user_id: Some(1),
                email: Some("admin@example.com".to_owned()),
            },
            parent_run_id: None,
        })
        .await
        .expect("run");
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
            .expect("finish");
    }
}

#[tokio::test]
async fn toast_routes_require_login() {
    let (state, _dir) = test_state().await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;

    for (path, method) in [("/actions/toasts", "get"), ("/actions/runs/x/ack", "post")] {
        let req = if method == "get" {
            test::TestRequest::get().uri(path).to_request()
        } else {
            test::TestRequest::post().uri(path).to_request()
        };
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT, "{path}");
        assert_eq!(location(&resp), "/login", "{path}");
    }
}

#[tokio::test]
async fn tray_shows_running_and_recent_completions_for_the_actor() {
    let (state, _dir) = test_state_with_client(seeded_client()).await;
    seed_run(&state, "app.restart", None).await;
    seed_run(&state, "app.stop", Some(true)).await;
    seed_run(&state, "app.rebuild", Some(false)).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/actions/toasts")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("app.restart"), "{body}");
    assert!(
        body.contains("queued"),
        "no outcome and no job => queued: {body}"
    );
    assert!(body.contains("app.stop"), "{body}");
    assert!(body.contains("succeeded"), "{body}");
    assert!(body.contains("app.rebuild"), "{body}");
    assert!(body.contains("failed"), "{body}");
    assert!(
        body.contains("Dismiss"),
        "completions carry a dismiss control: {body}"
    );
}

#[tokio::test]
async fn tray_excludes_other_actors() {
    let (state, _dir) = test_state_with_client(seeded_client()).await;
    state
        .action_runs
        .insert_with(&NewRun {
            subject: "beta".to_owned(),
            operation: "app.start".to_owned(),
            target_kind: TargetKind::App,
            actor: Actor {
                user_id: Some(99),
                email: Some("other@example.com".to_owned()),
            },
            parent_run_id: None,
        })
        .await
        .expect("other run");
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/actions/toasts")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(
        !body.contains("app.start"),
        "someone else's run stays out: {body}"
    );
}

#[tokio::test]
async fn tray_is_empty_when_there_is_nothing_to_show() {
    let (state, _dir) = test_state().await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/actions/toasts")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert_eq!(body.trim(), "", "an empty tray renders nothing: {body:?}");
}

#[tokio::test]
async fn ack_dismisses_a_completion_and_is_per_user() {
    let (state, _dir) = test_state_with_client(seeded_client()).await;
    seed_run(&state, "app.stop", Some(true)).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/actions/toasts")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Dismiss"), "completion visible: {body}");
    let csrf = common::extract_csrf(&body);

    // The ack form posts to /actions/runs/{id}/ack — pull the run id from the form action.
    let marker = r#"hx-post="/actions/runs/"#;
    let start = body.find(marker).expect("ack form") + marker.len();
    let run_id = body[start..start + 64].to_owned();

    let resp = test::call_service(
        &app,
        form_request(
            &format!("/actions/runs/{run_id}/ack"),
            format!("csrf_token={csrf}"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(
        !body.contains("Dismiss"),
        "acknowledged run is gone: {body}"
    );

    // The ack survives a reload (durable per-user dismissal).
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/actions/toasts")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(!body.contains("Dismiss"), "dismissal is durable: {body}");
}

#[tokio::test]
async fn ack_requires_csrf_and_unknown_runs_404() {
    let (state, _dir) = test_state().await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        form_request(
            &format!("/actions/runs/{}/ack", "a".repeat(64)),
            "csrf_token=bad".to_owned(),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let csrf = {
        let resp = test::call_service(
            &app,
            test::TestRequest::get()
                .uri("/")
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        common::extract_csrf(&get_body(resp).await)
    };
    let resp = test::call_service(
        &app,
        form_request(
            &format!("/actions/runs/{}/ack", "b".repeat(64)),
            format!("csrf_token={csrf}"),
        )
        .cookie(cookie)
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_run_started_on_one_container_toasts_on_another() {
    let (a, b, _dir) = states_over_shared_db(
        std::sync::Arc::new(seeded_client()),
        std::sync::Arc::new(seeded_client()),
    )
    .await;
    let app_a = test::init_service(build_app(a)).await;
    let app_b = test::init_service(build_app(b)).await;
    let cookie = complete_setup(&app_b).await;

    // Start a restart on A (queued run); B's tray must see it.
    let resp = test::call_service(
        &app_a,
        test::TestRequest::get()
            .uri("/apps/alpha")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let csrf = common::extract_csrf(&get_body(resp).await);
    let resp = test::call_service(
        &app_a,
        common::form_request("/apps/alpha/restart", format!("csrf_token={csrf}"))
            .insert_header(("HX-Request", "true"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains(r#"data-run-url="/actions/runs/"#), "{body}");

    // Poll B's tray until the run shows up (executor may finish it instantly).
    let mut tray = String::new();
    for _ in 0..200 {
        let resp = test::call_service(
            &app_b,
            test::TestRequest::get()
                .uri("/actions/toasts")
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        tray = get_body(resp).await;
        if tray.contains("app.restart") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(tray.contains("app.restart"), "B toasts A's run: {tray}");
    assert!(
        tray.contains("succeeded") || tray.contains("running") || tray.contains("queued"),
        "{tray}"
    );
}
