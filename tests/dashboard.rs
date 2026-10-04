mod common;

use actix_web::http::StatusCode;
use actix_web::test;

use common::{
    complete_setup, extract_csrf, form_request, get_body, location, seed_user, test_state,
    test_state_with_client, test_state_with_shared_client,
};

use dokku_ui::dokku::{DokkuError, DokkuOutput, MockClient};
use dokku_ui::domain::AppName;
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::web::build_app;

fn app(name: &str) -> AppName {
    AppName::try_from(name).expect("valid app name")
}

fn ps_report(running: bool, deployed: bool, processes: i64) -> DokkuOutput {
    DokkuOutput::ok(format!(
        r#"{{"deployed": "{deployed}", "running": "{running}", "processes": "{processes}"}}"#
    ))
}

fn ps_failure() -> Result<DokkuOutput, DokkuError> {
    Err(DokkuError::Exit {
        code: 1,
        stderr: "boom".into(),
    })
}

fn seeded_dashboard() -> MockClient {
    MockClient::new()
        .stub(
            DokkuCommand::AppsList,
            Ok(DokkuOutput::ok("=====> My Apps\nalpha\nbeta\ngamma")),
        )
        .stub(
            DokkuCommand::PsReport { app: app("alpha") },
            Ok(ps_report(true, true, 2)),
        )
        .stub(
            DokkuCommand::PsReport { app: app("beta") },
            Ok(ps_report(false, true, 1)),
        )
        .stub(
            DokkuCommand::PsReport { app: app("gamma") },
            Ok(ps_report(false, false, 0)),
        )
}

#[tokio::test]
async fn dashboard_shows_stats_and_status_badges() {
    let (state, _dir) = test_state_with_client(seeded_dashboard()).await;
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

    for (app_name, label) in [
        ("alpha", "Running"),
        ("beta", "Stopped"),
        ("gamma", "Not deployed"),
    ] {
        assert!(body.contains(app_name), "{app_name} row present");
        assert!(body.contains(label), "{app_name} shows `{label}`");
    }
    assert!(body.contains("text-emerald-400"));
    assert!(body.contains("text-red-400"));
    assert!(body.contains("text-slate-400"));
    assert!(body.contains(r#"text-white">3</p>"#), "total stat");
    assert!(body.contains(r#"text-emerald-400">1</p>"#), "running stat");
    assert!(body.contains(r#"text-red-400">2</p>"#), "stopped stat");
}

#[tokio::test]
async fn dashboard_survives_single_app_report_failure() {
    let (state, _dir) = test_state_with_client(
        MockClient::new()
            .stub(
                DokkuCommand::AppsList,
                Ok(DokkuOutput::ok("=====> My Apps\ngood\nbad")),
            )
            .stub(
                DokkuCommand::PsReport { app: app("good") },
                Ok(ps_report(true, true, 1)),
            )
            .stub(DokkuCommand::PsReport { app: app("bad") }, ps_failure()),
    )
    .await;
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

    assert!(body.contains("good"));
    assert!(body.contains("Running"));
    assert!(body.contains("bad"));
    assert!(body.contains("Unknown"));
}

#[tokio::test]
async fn dashboard_shows_empty_state_without_apps() {
    let (state, _dir) = test_state_with_client(MockClient::new().stub(
        DokkuCommand::AppsList,
        Ok(DokkuOutput::ok("=====> My Apps")),
    ))
    .await;
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

    assert!(body.contains("No applications yet"));
    assert!(body.contains(">0<"));
}

#[tokio::test]
async fn dashboard_renders_503_when_apps_list_unreachable() {
    let (state, _dir) = test_state_with_client(MockClient::with_default(Err(DokkuError::Connect(
        "refused".into(),
    ))))
    .await;
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
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = get_body(resp).await;
    assert!(body.contains("503"));
}

#[tokio::test]
async fn dashboard_second_request_makes_no_new_dokku_calls() {
    let (state, client, _dir) = test_state_with_shared_client(seeded_dashboard()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let first = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let calls_after_first = client.calls().len();

    let second = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(
        client.calls().len(),
        calls_after_first,
        "second load served entirely from the warm snapshot"
    );
}

#[tokio::test]
async fn dashboard_shows_data_age_chip() {
    let (state, _dir) = test_state_with_client(seeded_dashboard()).await;
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
    let body = get_body(resp).await;

    assert!(body.contains("Updated"));
    assert!(body.contains("ago") || body.contains("just now"));
}

#[tokio::test]
async fn dashboard_renders_refresh_button() {
    let (state, _dir) = test_state_with_client(seeded_dashboard()).await;
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
    let body = get_body(resp).await;

    assert!(body.contains(r#"action="/refresh""#));
    assert!(body.contains("Refresh data"));
}

#[tokio::test]
async fn refresh_post_reruns_snapshot_and_redirects_home() {
    let (state, client, _dir) = test_state_with_shared_client(seeded_dashboard()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let csrf = extract_csrf(&get_body(resp).await);
    let calls_before = client.calls().len();

    let resp = test::call_service(
        &app,
        form_request("/refresh", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/");
    assert!(
        client.calls().len() > calls_before,
        "manual refresh re-runs the snapshot pass"
    );
    let cookie = common::response_cookie(&resp).unwrap_or(cookie);

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Data refreshed."));
}

#[tokio::test]
async fn refresh_post_without_csrf_is_rejected() {
    let (state, _dir) = test_state_with_client(seeded_dashboard()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        form_request("/refresh", String::new())
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unauthenticated_dashboard_redirects_to_login() {
    let (state, _dir) = test_state().await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;

    let resp = test::call_service(&app, test::TestRequest::get().uri("/").to_request()).await;
    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(common::location(&resp), "/login");
}
