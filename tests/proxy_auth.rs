mod common;

use std::collections::HashMap;
use std::sync::Arc;

use actix_web::http::StatusCode;
use actix_web::test;

use common::{extract_csrf, form_request, get_body};

use dokku_ui::auth::rbac::Role;
use dokku_ui::dokku::{
    CapabilitiesStore, DokkuClient, DokkuOutput, FakeResolver, MockClient, SnapshotStore,
};
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::settings::Settings;
use dokku_ui::storage;
use dokku_ui::storage::jobs::SqliteJobsRepo;
use dokku_ui::storage::runs::SqliteRunsRepo;
use dokku_ui::storage::users::{SqliteUsersRepo, UsersRepo};
use dokku_ui::storage::webhooks::SqliteWebhooksRepo;
use dokku_ui::web::{AppState, build_app};

async fn proxy_state(vars: &[(&str, &str)]) -> (AppState, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let database_url = format!("sqlite://{}/test.db", dir.path().display());
    let pool = storage::connect(&database_url).await.expect("connect db");
    let map: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let settings = Settings::from_map(&map).expect("settings");
    let client = MockClient::new().stub(
        DokkuCommand::AppsList,
        Ok(DokkuOutput::ok("=====> My Apps")),
    );
    let dokku: Arc<dyn DokkuClient> = Arc::new(client);
    let snapshot = Arc::new(SnapshotStore::with_resolver(
        dokku.clone(),
        Arc::new(FakeResolver::none()),
        pool.clone(),
    ));
    let state = AppState {
        action_runs: Arc::new(SqliteRunsRepo::new(pool.clone())),
        jobs: Arc::new(SqliteJobsRepo::new(pool.clone())),
        webhooks: Arc::new(SqliteWebhooksRepo::new(pool.clone())),
        capabilities: Arc::new(CapabilitiesStore::new(dokku.clone(), pool.clone())),
        db: pool,
        settings,
        dokku,
        snapshot,
    };
    // One existing user keeps the login redirect on /login (not /setup).
    common::seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    (state, dir)
}

async fn get_with_peer<S, B, E>(
    app: &S,
    path: &str,
    peer: &str,
    header: Option<(&str, &str)>,
) -> (StatusCode, String)
where
    S: actix_web::dev::Service<
            actix_http::Request,
            Response = actix_web::dev::ServiceResponse<B>,
            Error = E,
        >,
    B: actix_web::body::MessageBody,
    E: std::fmt::Debug,
{
    let mut request = test::TestRequest::get()
        .uri(path)
        .peer_addr(peer.parse().expect("peer"));
    if let Some((name, value)) = header {
        request = request.insert_header((name.to_owned(), value.to_owned()));
    }
    let resp = test::call_service(app, request.to_request()).await;
    let status = resp.status();
    let location = resp
        .headers()
        .get("location")
        .map(|v| v.to_str().unwrap_or("").to_owned());
    let body = get_body(resp).await;
    (status, location.unwrap_or(body))
}

#[tokio::test]
async fn trusted_proxy_header_authenticates_and_auto_registers() {
    let (state, _dir) = proxy_state(&[
        ("TRUSTED_PROXY_CIDRS", "10.0.0.0/8"),
        ("PROXY_AUTH_DEFAULT_ROLE", "operator"),
    ])
    .await;
    let app = test::init_service(build_app(state.clone())).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/")
            .peer_addr("10.1.2.3:5555".parse().expect("peer"))
            .insert_header(("x-forwarded-user", "Alice@Example.com"))
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("Dashboard"), "{body}");

    let repo = SqliteUsersRepo::new(state.db.clone());
    let user = repo
        .find_by_email("alice@example.com")
        .await
        .expect("find")
        .expect("auto-registered");
    assert_eq!(user.role, Role::Operator);
    assert_eq!(user.password_hash, "!proxy-auth");
}

#[tokio::test]
async fn untrusted_peers_and_missing_headers_are_redirected_to_login() {
    let (state, _dir) = proxy_state(&[("TRUSTED_PROXY_CIDRS", "10.0.0.0/8")]).await;
    let app = test::init_service(build_app(state)).await;

    // Header from an untrusted peer is ignored.
    let (status, location) = get_with_peer(
        &app,
        "/",
        "192.168.1.9:5555",
        Some(("x-forwarded-user", "alice@example.com")),
    )
    .await;
    assert_eq!(status, StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(location, "/login");

    // Trusted peer but no header.
    let (status, location) = get_with_peer(&app, "/", "10.1.2.3:5555", None).await;
    assert_eq!(status, StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(location, "/login");

    // Trusted peer but a header value that is not an email.
    let (status, location) = get_with_peer(
        &app,
        "/",
        "10.1.2.3:5555",
        Some(("x-forwarded-user", "not-an-email")),
    )
    .await;
    assert_eq!(status, StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(location, "/login");
}

#[tokio::test]
async fn proxy_auth_is_disabled_without_trusted_cidrs() {
    let (state, _dir) = proxy_state(&[("PROXY_AUTH_HEADER", "x-auth-request-email")]).await;
    let app = test::init_service(build_app(state)).await;

    let (status, location) = get_with_peer(
        &app,
        "/",
        "10.1.2.3:5555",
        Some(("x-auth-request-email", "alice@example.com")),
    )
    .await;
    assert_eq!(status, StatusCode::TEMPORARY_REDIRECT, "feature off");
    assert_eq!(location, "/login");
}

#[tokio::test]
async fn proxy_auth_header_is_configurable_and_role_applies_to_admin_pages() {
    let (state, _dir) = proxy_state(&[
        ("TRUSTED_PROXY_CIDRS", "10.0.0.0/8"),
        ("PROXY_AUTH_HEADER", "X-Auth-Request-Email"),
        ("PROXY_AUTH_DEFAULT_ROLE", "admin"),
    ])
    .await;
    let app = test::init_service(build_app(state.clone())).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/users")
            .peer_addr("10.9.9.9:4444".parse().expect("peer"))
            .insert_header(("x-auth-request-email", "root@example.com"))
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("Add user"), "{body}");

    let repo = SqliteUsersRepo::new(state.db.clone());
    let user = repo
        .find_by_email("root@example.com")
        .await
        .expect("find")
        .expect("registered");
    assert_eq!(user.role, Role::Admin);
}

#[tokio::test]
async fn proxy_auth_establishes_a_session_cookie() {
    let (state, _dir) = proxy_state(&[("TRUSTED_PROXY_CIDRS", "10.0.0.0/8")]).await;
    let app = test::init_service(build_app(state)).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/")
            .peer_addr("10.1.2.3:5555".parse().expect("peer"))
            .insert_header(("x-forwarded-user", "alice@example.com"))
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = common::session_cookie(&resp);

    // The issued session authenticates without the proxy header.
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn proxy_registered_users_never_get_a_500_on_password_checks() {
    let (state, _dir) = proxy_state(&[("TRUSTED_PROXY_CIDRS", "10.0.0.0/8")]).await;
    let app = test::init_service(build_app(state)).await;

    // Auto-register the proxy user.
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/")
            .peer_addr("10.1.2.3:5555".parse().expect("peer"))
            .insert_header(("x-forwarded-user", "alice@example.com"))
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = common::session_cookie(&resp);

    // Password login for the `!proxy-auth` hash renders the normal error.
    let resp = test::call_service(&app, test::TestRequest::get().uri("/login").to_request()).await;
    let login_cookie = common::response_cookie(&resp).expect("login session cookie");
    let csrf = extract_csrf(&get_body(resp).await);
    let resp = test::call_service(
        &app,
        form_request(
            "/login",
            format!("csrf_token={csrf}&email=alice%40example.com&password=whatever123"),
        )
        .cookie(login_cookie)
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "not a 500");
    let body = get_body(resp).await;
    assert!(body.contains("Invalid email or password"), "{body}");

    // Re-auth (which sensitive actions require) renders its error too.
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/reauth")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let csrf = extract_csrf(&get_body(resp).await);
    let resp = test::call_service(
        &app,
        form_request(
            "/reauth",
            format!("csrf_token={csrf}&password=whatever123&next=%2F"),
        )
        .cookie(cookie)
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "not a 500");
    let body = get_body(resp).await;
    assert!(body.contains("Invalid password"), "{body}");
}
