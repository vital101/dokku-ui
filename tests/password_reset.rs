mod common;

use actix_web::http::StatusCode;
use actix_web::test;

use common::{
    extract_csrf, form_request, get_body, location, login, response_cookie, seed_user, test_state,
};

use dokku_ui::domain::InstanceSettings;
use dokku_ui::storage::instance_settings::{InstanceSettingsRepo, SqliteInstanceSettingsRepo};
use dokku_ui::web::build_app;

async fn save_public_url(state: &dokku_ui::web::AppState, url: &str) {
    SqliteInstanceSettingsRepo::new(state.db.clone())
        .save(&InstanceSettings::parse(Some(url), None, None, None, None).expect("settings"))
        .await
        .expect("save settings");
}

fn extract_reset_token(html: &str) -> String {
    let marker = "https://ui.example.com/reset/";
    let start = html.find(marker).expect("reset link in body") + marker.len();
    html[start..]
        .chars()
        .take_while(|c| c.is_ascii_hexdigit())
        .collect()
}

#[tokio::test]
async fn admin_generates_a_one_time_reset_link_that_sets_the_password() {
    let (state, _dir) = test_state().await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    save_public_url(&state, "https://ui.example.com").await;
    let app = test::init_service(build_app(state.clone())).await;
    let admin = login(&app, "admin@example.com", "correct-horse-battery").await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/users")
            .cookie(admin.clone())
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    let csrf = extract_csrf(&body);

    let resp = test::call_service(
        &app,
        form_request("/users/1/reset", format!("csrf_token={csrf}"))
            .cookie(admin.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("Reset link for admin@example.com"), "{body}");
    let token = extract_reset_token(&body);
    assert_eq!(token.len(), 64, "token extracted: {token}");

    let reset_path = format!("/reset/{token}");
    let resp =
        test::call_service(&app, test::TestRequest::get().uri(&reset_path).to_request()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let reset_cookie = response_cookie(&resp).expect("reset session cookie");
    let body = get_body(resp).await;
    assert!(body.contains("New password"), "{body}");
    let reset_csrf = extract_csrf(&body);

    let resp = test::call_service(
        &app,
        form_request(
            &reset_path,
            format!("csrf_token={reset_csrf}&password=new-password-123&confirm=new-password-123"),
        )
        .cookie(reset_cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/login");
    let reset_cookie = response_cookie(&resp).unwrap_or(reset_cookie);

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/login")
            .cookie(reset_cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("Password updated"), "{body}");

    let resp = test::call_service(
        &app,
        test::TestRequest::get().uri("/").cookie(admin).to_request(),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::TEMPORARY_REDIRECT,
        "resetting revokes the target's existing sessions"
    );
    assert_eq!(location(&resp), "/login");

    let fresh = login(&app, "admin@example.com", "new-password-123").await;
    let resp = test::call_service(
        &app,
        test::TestRequest::get().uri("/").cookie(fresh).to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = test::call_service(
        &app,
        form_request(
            &reset_path,
            format!(
                "csrf_token={reset_csrf}&password=another-password-1&confirm=another-password-1"
            ),
        )
        .cookie(reset_cookie)
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("invalid or has expired"), "{body}");
}

#[tokio::test]
async fn reset_link_generation_requires_a_public_url() {
    let (state, _dir) = test_state().await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state.clone())).await;
    let admin = login(&app, "admin@example.com", "correct-horse-battery").await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/users")
            .cookie(admin.clone())
            .to_request(),
    )
    .await;
    let csrf = extract_csrf(&get_body(resp).await);

    let resp = test::call_service(
        &app,
        form_request("/users/1/reset", format!("csrf_token={csrf}"))
            .cookie(admin.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/users");
    let admin = response_cookie(&resp).unwrap_or(admin);

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/users")
            .cookie(admin)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Set a public URL"), "{body}");
}

#[tokio::test]
async fn reset_routes_validate_tokens_and_csrf() {
    let (state, _dir) = test_state().await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/reset/not-a-token")
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let token = "a".repeat(64);
    let resp = test::call_service(
        &app,
        form_request(
            &format!("/reset/{token}"),
            "password=new-password-123&confirm=new-password-123".to_owned(),
        )
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
