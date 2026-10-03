#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use actix_web::body::MessageBody;
use actix_web::cookie::Cookie;
use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::StatusCode;
use actix_web::http::header::{CONTENT_TYPE, LOCATION};
use actix_web::test;

use dokku_ui::auth::password::hash_password;
use dokku_ui::dokku::{DokkuClient, FakeResolver, MockClient, SnapshotStore};
use dokku_ui::domain::Password;
use dokku_ui::settings::Settings;
use dokku_ui::storage;
use dokku_ui::storage::users::{SqliteUsersRepo, UsersRepo};
use dokku_ui::web::AppState;

pub const SESSION_COOKIE: &str = "dokku-ui-session";

pub async fn test_state() -> (AppState, tempfile::TempDir) {
    let client = MockClient::new().stub(
        dokku_ui::domain::command::DokkuCommand::AppsList,
        Ok(dokku_ui::dokku::DokkuOutput::ok("=====> My Apps")),
    );
    test_state_with_client(client).await
}

pub async fn test_state_with_client(client: MockClient) -> (AppState, tempfile::TempDir) {
    let (state, _client, dir) = test_state_with_shared_client(client).await;
    (state, dir)
}

pub async fn test_state_with_shared_client(
    client: MockClient,
) -> (AppState, Arc<MockClient>, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let database_url = format!("sqlite://{}/test.db", dir.path().display());
    let pool = storage::connect(&database_url).await.expect("connect db");
    let settings = Settings::from_map(&HashMap::new()).expect("default settings");
    let client_arc = Arc::new(client);
    let dokku: Arc<dyn DokkuClient> = client_arc.clone();
    let snapshot = Arc::new(SnapshotStore::with_resolver(
        dokku.clone(),
        Arc::new(FakeResolver::none()),
    ));
    (
        AppState {
            db: pool,
            settings,
            dokku,
            snapshot,
        },
        client_arc,
        dir,
    )
}

pub async fn seed_user(state: &AppState, email: &str, password: &str) {
    let repo = SqliteUsersRepo::new(state.db.clone());
    let hash = hash_password(&Password::new(password).expect("valid password")).expect("hash");
    repo.insert(email, &hash).await.expect("insert user");
}

pub fn extract_csrf(html: &str) -> String {
    let marker = r#"name="csrf_token" value=""#;
    let start = html.find(marker).expect("csrf field in form") + marker.len();
    let rest = &html[start..];
    let end = rest.find('"').expect("closing quote");
    rest[..end].to_owned()
}

pub fn response_cookie<B>(resp: &ServiceResponse<B>) -> Option<Cookie<'static>> {
    resp.response()
        .cookies()
        .find(|cookie| cookie.name() == SESSION_COOKIE)
        .map(|cookie| Cookie::build(cookie.name().to_owned(), cookie.value().to_owned()).finish())
}

pub fn session_cookie<B>(resp: &ServiceResponse<B>) -> Cookie<'static> {
    response_cookie(resp).expect("session cookie set")
}

pub fn location<B>(resp: &ServiceResponse<B>) -> String {
    resp.response()
        .headers()
        .get(LOCATION)
        .expect("location header")
        .to_str()
        .expect("utf-8 location")
        .to_owned()
}

pub fn form_request(path: &str, body: String) -> actix_web::test::TestRequest {
    test::TestRequest::post()
        .uri(path)
        .insert_header((CONTENT_TYPE, "application/x-www-form-urlencoded"))
        .set_payload(body)
}

pub async fn get_body<B>(resp: ServiceResponse<B>) -> String
where
    B: MessageBody,
{
    String::from_utf8(test::read_body(resp).await.to_vec()).expect("utf-8 body")
}

pub async fn complete_setup<S, B, E>(app: &S) -> Cookie<'static>
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = E>,
    B: MessageBody,
    E: std::fmt::Debug,
{
    let resp = test::call_service(app, test::TestRequest::get().uri("/setup").to_request()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = response_cookie(&resp).expect("setup session cookie");
    let body = get_body(resp).await;
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
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/");
    response_cookie(&resp).unwrap_or(cookie)
}

pub async fn login<S, B, E>(app: &S, email: &str, password: &str) -> Cookie<'static>
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = E>,
    B: MessageBody,
    E: std::fmt::Debug,
{
    let resp = test::call_service(app, test::TestRequest::get().uri("/login").to_request()).await;
    let cookie = response_cookie(&resp).expect("login session cookie");
    let body = get_body(resp).await;
    let csrf = extract_csrf(&body);

    let resp = test::call_service(
        app,
        form_request(
            "/login",
            format!(
                "csrf_token={csrf}&email={}&password={}",
                urlencode(email),
                urlencode(password)
            ),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/");
    session_cookie(&resp)
}

fn urlencode(input: &str) -> String {
    input
        .bytes()
        .flat_map(|byte| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            other => format!("%{other:02X}").chars().collect(),
        })
        .collect()
}
