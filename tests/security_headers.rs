mod common;

use actix_web::http::StatusCode;
use actix_web::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, HeaderName, HeaderValue, REFERRER_POLICY,
    X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use actix_web::test;

use dokku_ui::web::build_app;

const CSP: &str =
    "default-src 'self'; style-src 'self'; img-src 'self' data:; frame-ancestors 'none'";

fn header_value<B>(resp: &actix_web::dev::ServiceResponse<B>, name: HeaderName) -> &HeaderValue {
    resp.response().headers().get(name).expect("header present")
}

fn assert_security_headers<B>(resp: &actix_web::dev::ServiceResponse<B>) {
    assert_eq!(
        header_value(resp, X_CONTENT_TYPE_OPTIONS)
            .to_str()
            .expect("utf-8"),
        "nosniff"
    );
    assert_eq!(
        header_value(resp, X_FRAME_OPTIONS).to_str().expect("utf-8"),
        "DENY"
    );
    assert_eq!(
        header_value(resp, REFERRER_POLICY).to_str().expect("utf-8"),
        "no-referrer"
    );
    assert_eq!(
        header_value(resp, CONTENT_SECURITY_POLICY)
            .to_str()
            .expect("utf-8"),
        CSP
    );
}

#[tokio::test]
async fn public_pages_send_security_headers() {
    let (state, _dir) = common::test_state().await;
    let app = test::init_service(build_app(state)).await;

    let resp =
        test::call_service(&app, test::TestRequest::get().uri("/healthz").to_request()).await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_security_headers(&resp);
}

#[tokio::test]
async fn login_page_sends_security_headers() {
    let (state, _dir) = common::test_state().await;
    common::seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;

    let resp = test::call_service(&app, test::TestRequest::get().uri("/login").to_request()).await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_security_headers(&resp);
}

#[tokio::test]
async fn auth_redirects_send_security_headers() {
    let (state, _dir) = common::test_state().await;
    let app = test::init_service(build_app(state)).await;

    let resp = test::call_service(&app, test::TestRequest::get().uri("/").to_request()).await;

    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_security_headers(&resp);
}

#[tokio::test]
async fn error_pages_send_security_headers() {
    let (state, _dir) = common::test_state().await;
    let app = test::init_service(build_app(state)).await;
    let cookie = common::complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/nope")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_security_headers(&resp);
}

#[tokio::test]
async fn static_responses_get_cache_control_and_security_headers() {
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
    assert_security_headers(&resp);
    assert_eq!(
        header_value(&resp, CACHE_CONTROL).to_str().expect("utf-8"),
        "public, max-age=86400"
    );
}

#[tokio::test]
async fn non_static_responses_have_no_cache_control() {
    let (state, _dir) = common::test_state().await;
    let app = test::init_service(build_app(state)).await;

    let resp =
        test::call_service(&app, test::TestRequest::get().uri("/healthz").to_request()).await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.response().headers().get(CACHE_CONTROL), None);
}
