mod common;

use actix_web::http::StatusCode;
use actix_web::http::header::CONTENT_TYPE;
use actix_web::test;

use dokku_ui::web::build_app;

#[tokio::test]
async fn favicon_is_served_without_authentication() {
    let (state, _dir) = common::test_state().await;
    let app = test::init_service(build_app(state)).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get().uri("/favicon.ico").to_request(),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.response()
            .headers()
            .get(CONTENT_TYPE)
            .expect("content type")
            .to_str()
            .expect("utf-8"),
        "image/svg+xml"
    );
    let body = String::from_utf8(test::read_body(resp).await.to_vec()).expect("utf-8 body");
    assert!(body.contains("<svg"));
    assert!(body.ends_with("</svg>"));
}

#[tokio::test]
async fn favicon_is_a_valid_svg() {
    let (state, _dir) = common::test_state().await;
    let app = test::init_service(build_app(state)).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get().uri("/favicon.ico").to_request(),
    )
    .await;

    let body = String::from_utf8(test::read_body(resp).await.to_vec()).expect("utf-8 body");
    let head = &body[..body.find('>').expect("svg opening tag") + 1];
    assert!(head.contains("viewBox"));
    assert!(head.contains("xmlns=\"http://www.w3.org/2000/svg\""));
}

#[tokio::test]
async fn favicon_has_no_external_references() {
    let (state, _dir) = common::test_state().await;
    let app = test::init_service(build_app(state)).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get().uri("/favicon.ico").to_request(),
    )
    .await;

    let body = String::from_utf8(test::read_body(resp).await.to_vec()).expect("utf-8 body");
    assert!(!body.contains("href="));
    assert!(!body.contains("<image"));
    assert!(!body.contains("url("));
    let without_namespace = body.replace("http://www.w3.org/2000/svg", "");
    assert!(!without_namespace.contains("http://"));
    assert!(!without_namespace.contains("https://"));
}

#[tokio::test]
async fn favicon_gets_security_headers() {
    let (state, _dir) = common::test_state().await;
    let app = test::init_service(build_app(state)).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get().uri("/favicon.ico").to_request(),
    )
    .await;

    for name in [
        actix_web::http::header::X_CONTENT_TYPE_OPTIONS,
        actix_web::http::header::X_FRAME_OPTIONS,
        actix_web::http::header::REFERRER_POLICY,
        actix_web::http::header::CONTENT_SECURITY_POLICY,
    ] {
        assert!(
            resp.response().headers().get(&name).is_some(),
            "missing {name}"
        );
    }
    assert_eq!(
        resp.response()
            .headers()
            .get(actix_web::http::header::CACHE_CONTROL),
        None
    );
}
