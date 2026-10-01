mod common;

use actix_web::body::MessageBody;
use actix_web::cookie::Cookie;
use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::StatusCode;
use actix_web::http::header::{CONTENT_TYPE, LOCATION};
use actix_web::test;

use dokku_ui::auth::password::hash_password;
use dokku_ui::domain::Password;
use dokku_ui::storage::users::{SqliteUsersRepo, UsersRepo};
use dokku_ui::web::build_app;

const SESSION_COOKIE: &str = "dokku-ui-session";

fn extract_csrf(html: &str) -> String {
    let marker = r#"name="csrf_token" value=""#;
    let start = html.find(marker).expect("csrf field in form") + marker.len();
    let rest = &html[start..];
    let end = rest.find('"').expect("closing quote");
    rest[..end].to_owned()
}

fn session_cookie<B>(resp: &ServiceResponse<B>) -> Cookie<'static> {
    response_cookie(resp).expect("session cookie set")
}

fn response_cookie<B>(resp: &ServiceResponse<B>) -> Option<Cookie<'static>> {
    resp.response()
        .cookies()
        .find(|cookie| cookie.name() == SESSION_COOKIE)
        .map(|cookie| Cookie::build(cookie.name().to_owned(), cookie.value().to_owned()).finish())
}

fn location<B>(resp: &ServiceResponse<B>) -> String {
    resp.response()
        .headers()
        .get(LOCATION)
        .expect("location header")
        .to_str()
        .expect("utf-8 location")
        .to_owned()
}

fn form_request(path: &str, body: String) -> actix_web::test::TestRequest {
    test::TestRequest::post()
        .uri(path)
        .insert_header((CONTENT_TYPE, "application/x-www-form-urlencoded"))
        .set_payload(body)
}

async fn complete_setup<S, B, E>(app: &S) -> Cookie<'static>
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = E>,
    B: MessageBody,
    E: std::fmt::Debug,
{
    let resp = test::call_service(app, test::TestRequest::get().uri("/setup").to_request()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = response_cookie(&resp).expect("setup session cookie");
    let body = String::from_utf8(test::read_body(resp).await.to_vec()).expect("utf-8");
    let csrf = extract_csrf(&body);

    let resp = test::call_service(
        app,
        form_request(
            "/setup",
            format!(
                "csrf_token={csrf}&email=admin%40example.com&password=correct-horse-battery&confirm=correct-horse-battery"
            ),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(location(&resp), "/");
    response_cookie(&resp).unwrap_or(cookie)
}

async fn seed_user(state: &dokku_ui::web::AppState, email: &str, password: &str) {
    let repo = SqliteUsersRepo::new(state.db.clone());
    let hash = hash_password(&Password::new(password).expect("valid password")).expect("hash");
    repo.insert(email, &hash).await.expect("insert user");
}

#[tokio::test]
async fn unauthenticated_requests_redirect_to_setup_when_no_users() {
    let (state, _dir) = common::test_state().await;
    let app = test::init_service(build_app(state)).await;

    for path in ["/", "/apps", "/apps/myapp", "/logout"] {
        let resp = test::call_service(&app, test::TestRequest::get().uri(path).to_request()).await;
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT, "{path}");
        assert_eq!(location(&resp), "/setup", "{path}");
    }
}

#[tokio::test]
async fn unauthenticated_requests_redirect_to_login_when_users_exist() {
    let (state, _dir) = common::test_state().await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;

    let resp = test::call_service(&app, test::TestRequest::get().uri("/").to_request()).await;
    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(location(&resp), "/login");
}

#[tokio::test]
async fn public_paths_are_reachable_without_auth() {
    let (state, _dir) = common::test_state().await;
    let app = test::init_service(build_app(state)).await;

    for path in ["/healthz", "/static/css/app.css"] {
        let resp = test::call_service(&app, test::TestRequest::get().uri(path).to_request()).await;
        assert_eq!(resp.status(), StatusCode::OK, "{path}");
    }
}

#[tokio::test]
async fn setup_flow_creates_admin_and_logs_in() {
    let (state, _dir) = common::test_state().await;
    let app = test::init_service(build_app(state.clone())).await;

    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = String::from_utf8(test::read_body(resp).await.to_vec()).expect("utf-8");
    assert!(body.contains("admin@example.com"));
    assert!(body.contains("Setup complete"));

    let repo = SqliteUsersRepo::new(state.db.clone());
    assert_eq!(repo.count().await.expect("count"), 1);
}

#[tokio::test]
async fn setup_rejects_mismatched_passwords() {
    let (state, _dir) = common::test_state().await;
    let app = test::init_service(build_app(state)).await;

    let resp = test::call_service(&app, test::TestRequest::get().uri("/setup").to_request()).await;
    let cookie = response_cookie(&resp).expect("setup session cookie");
    let body = String::from_utf8(test::read_body(resp).await.to_vec()).expect("utf-8");
    let csrf = extract_csrf(&body);

    let resp = test::call_service(
        &app,
        form_request(
            "/setup",
            format!(
                "csrf_token={csrf}&email=admin%40example.com&password=correct-horse-battery&confirm=different-password"
            ),
        )
        .cookie(cookie)
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = String::from_utf8(test::read_body(resp).await.to_vec()).expect("utf-8");
    assert!(body.contains("Passwords do not match"));
}

#[tokio::test]
async fn setup_redirects_to_login_when_users_exist() {
    let (state, _dir) = common::test_state().await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state.clone())).await;

    let resp = test::call_service(&app, test::TestRequest::get().uri("/setup").to_request()).await;
    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(location(&resp), "/login");

    let resp = test::call_service(&app, test::TestRequest::get().uri("/login").to_request()).await;
    let cookie = response_cookie(&resp).expect("login session cookie");
    let body = String::from_utf8(test::read_body(resp).await.to_vec()).expect("utf-8");
    let csrf = extract_csrf(&body);

    let resp = test::call_service(
        &app,
        form_request(
            "/setup",
            format!(
                "csrf_token={csrf}&email=second%40example.com&password=correct-horse-battery&confirm=correct-horse-battery"
            ),
        )
        .cookie(cookie)
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(location(&resp), "/login");
    assert_eq!(
        SqliteUsersRepo::new(state.db.clone())
            .count()
            .await
            .expect("count"),
        1
    );
}

#[tokio::test]
async fn login_flow_authenticates_and_logs_out() {
    let (state, _dir) = common::test_state().await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;

    let resp = test::call_service(&app, test::TestRequest::get().uri("/login").to_request()).await;
    let cookie = response_cookie(&resp).expect("login session cookie");
    let body = String::from_utf8(test::read_body(resp).await.to_vec()).expect("utf-8");
    let csrf = extract_csrf(&body);

    let resp = test::call_service(
        &app,
        form_request(
            "/login",
            format!("csrf_token={csrf}&email=admin%40example.com&password=wrong-password-123"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = String::from_utf8(test::read_body(resp).await.to_vec()).expect("utf-8");
    assert!(body.contains("Invalid email or password"));

    let resp = test::call_service(
        &app,
        form_request(
            "/login",
            format!("csrf_token={csrf}&email=admin%40example.com&password=correct-horse-battery"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(location(&resp), "/");
    let cookie = session_cookie(&resp);

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let logout_page = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let body = String::from_utf8(test::read_body(logout_page).await.to_vec()).expect("utf-8");
    let logout_csrf = extract_csrf(&body);

    let resp = test::call_service(
        &app,
        form_request("/logout", format!("csrf_token={logout_csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(location(&resp), "/login");

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(location(&resp), "/login");
}

#[tokio::test]
async fn login_honors_safe_next_and_blocks_external_redirects() {
    let (state, _dir) = common::test_state().await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;

    let resp = test::call_service(&app, test::TestRequest::get().uri("/login").to_request()).await;
    let cookie = response_cookie(&resp).expect("login session cookie");
    let body = String::from_utf8(test::read_body(resp).await.to_vec()).expect("utf-8");
    let csrf = extract_csrf(&body);

    let login = |next: &str, csrf: &str, cookie: Cookie<'static>| {
        form_request(
            "/login",
            format!(
                "csrf_token={csrf}&email=admin%40example.com&password=correct-horse-battery&next={next}"
            ),
        )
        .cookie(cookie)
        .to_request()
    };

    let resp = test::call_service(&app, login("/apps", &csrf, cookie.clone())).await;
    assert_eq!(location(&resp), "/apps");
    let renewed = response_cookie(&resp).unwrap_or(cookie);

    let encoded_evil = "https%3A%2F%2Fevil.example";
    let resp = test::call_service(&app, login(encoded_evil, &csrf, renewed)).await;
    assert_eq!(location(&resp), "/");
}

#[tokio::test]
async fn posts_without_valid_csrf_are_rejected() {
    let (state, _dir) = common::test_state().await;
    let app = test::init_service(build_app(state)).await;

    for (path, body) in [
        ("/login", "email=a%40b.com&password=correct-horse-battery"),
        (
            "/setup",
            "email=a%40b.com&password=correct-horse-battery&confirm=correct-horse-battery",
        ),
    ] {
        let resp = test::call_service(&app, form_request(path, body.to_owned()).to_request()).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{path}");

        let resp = test::call_service(
            &app,
            form_request(path, format!("csrf_token={}&{body}", "f".repeat(64))).to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{path} wrong token");
    }
}

#[tokio::test]
async fn authenticated_unknown_route_renders_404() {
    let (state, _dir) = common::test_state().await;
    let app = test::init_service(build_app(state)).await;

    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/nope")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = String::from_utf8(test::read_body(resp).await.to_vec()).expect("utf-8");
    assert!(body.contains("404"));
}
