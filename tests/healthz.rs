use actix_web::http::StatusCode;
use actix_web::test;

use dokku_ui::web::build_app;

#[tokio::test]
async fn healthz_serves_styled_html() {
    let app = test::init_service(build_app()).await;

    let resp =
        test::call_service(&app, test::TestRequest::get().uri("/healthz").to_request()).await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = String::from_utf8(test::read_body(resp).await.to_vec()).expect("utf-8 body");
    assert!(body.contains("healthy"));
    assert!(body.contains(r#"href="/static/css/app.css""#));
}

#[tokio::test]
async fn static_css_is_served() {
    let app = test::init_service(build_app()).await;

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
async fn unknown_route_returns_404() {
    let app = test::init_service(build_app()).await;

    let resp = test::call_service(&app, test::TestRequest::get().uri("/nope").to_request()).await;

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
