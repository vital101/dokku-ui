mod common;

use actix_web::http::StatusCode;
use actix_web::http::header::{HeaderValue, LOCATION};
use actix_web::test;

use dokku_ui::web::build_app;

#[tokio::test]
async fn healthz_serves_styled_html() {
    let (state, _dir) = common::test_state().await;
    let app = test::init_service(build_app(state)).await;

    let resp =
        test::call_service(&app, test::TestRequest::get().uri("/healthz").to_request()).await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = String::from_utf8(test::read_body(resp).await.to_vec()).expect("utf-8 body");
    assert!(body.contains("healthy"));
    assert!(body.contains(r#"href="/static/css/app.css""#));
}

#[tokio::test]
async fn healthz_returns_503_when_database_unavailable() {
    let (state, _dir) = common::test_state().await;
    let app = test::init_service(build_app(state.clone())).await;
    state.db.close().await;

    let resp =
        test::call_service(&app, test::TestRequest::get().uri("/healthz").to_request()).await;

    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn static_css_is_served() {
    let (state, _dir) = common::test_state().await;
    let app = test::init_service(build_app(state)).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/static/css/app.css")
            .to_request(),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn vendored_htmx_is_served() {
    let (state, _dir) = common::test_state().await;
    let app = test::init_service(build_app(state)).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/static/js/htmx.min.js")
            .to_request(),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = String::from_utf8(test::read_body(resp).await.to_vec()).expect("utf-8 body");
    assert!(body.contains("htmx"), "vendored htmx bundle served");
}

#[tokio::test]
async fn unknown_route_redirects_unauthenticated_requests_to_setup() {
    let (state, _dir) = common::test_state().await;
    let app = test::init_service(build_app(state)).await;

    let resp = test::call_service(&app, test::TestRequest::get().uri("/nope").to_request()).await;

    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        resp.response().headers().get(LOCATION),
        Some(&HeaderValue::from_static("/setup"))
    );
}
