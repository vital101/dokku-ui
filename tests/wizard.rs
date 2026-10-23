mod common;

use actix_web::body::MessageBody;
use actix_web::cookie::Cookie;
use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::StatusCode;
use actix_web::test;

use common::{
    complete_setup, extract_csrf, form_request, get_body, location, login, response_cookie,
    seed_user_with_role, test_state, test_state_with_shared_client,
};
use dokku_ui::auth::rbac::Role;
use dokku_ui::dokku::{DokkuOutput, MockClient};
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::web::build_app;

async fn get_ok<S, B, E>(app: &S, path: &str, cookie: &Cookie<'static>) -> String
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = E>,
    B: MessageBody,
    E: std::fmt::Debug,
{
    let resp = test::call_service(
        app,
        test::TestRequest::get()
            .uri(path)
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "GET {path}");
    get_body(resp).await
}

#[tokio::test]
async fn wizard_name_step_redirects_to_login_when_unauthenticated() {
    let (state, _dir) = test_state().await;
    seed_user_with_role(
        &state,
        "admin@example.com",
        "correct-horse-battery",
        Role::Admin,
    )
    .await;
    let app = test::init_service(build_app(state)).await;

    let resp =
        test::call_service(&app, test::TestRequest::get().uri("/apps/new").to_request()).await;
    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(location(&resp), "/login");
}

#[tokio::test]
async fn viewer_gets_name_step_redirected_with_role_flash() {
    let (state, _dir) = test_state().await;
    seed_user_with_role(
        &state,
        "viewer@example.com",
        "correct-horse-battery",
        Role::Viewer,
    )
    .await;
    let app = test::init_service(build_app(state)).await;
    let viewer = login(&app, "viewer@example.com", "correct-horse-battery").await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/new")
            .cookie(viewer.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/");
    let cookie = response_cookie(&resp).unwrap_or(viewer);

    let body = get_ok(&app, "/", &cookie).await;
    assert!(
        body.contains("Your role does not allow creating apps."),
        "{body}"
    );
}

#[tokio::test]
async fn name_step_renders_after_setup() {
    let (state, _dir) = test_state().await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let body = get_ok(&app, "/apps/new", &cookie).await;
    assert!(body.contains(r#"name="name""#), "{body}");
    assert!(body.contains(r#"action="/apps/new""#), "{body}");
    assert!(body.contains("Next"), "{body}");
}

#[tokio::test]
async fn invalid_name_re_renders_without_creating_app() {
    let (state, client, _dir) = test_state_with_shared_client(MockClient::new()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let body = get_ok(&app, "/apps/new", &cookie).await;
    let csrf = extract_csrf(&body);

    let resp = test::call_service(
        &app,
        form_request("/apps/new", format!("csrf_token={csrf}&name=Bad_App"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "re-render, not redirect");
    let body = get_body(resp).await;
    assert!(body.contains("Invalid app name"), "{body}");
    assert!(
        body.contains(r#"value="Bad_App""#),
        "typed name kept: {body}"
    );
    assert!(
        client
            .calls()
            .iter()
            .all(|cmd| !matches!(cmd, DokkuCommand::AppsCreate { .. })),
        "no AppsCreate call for invalid name"
    );
}

#[tokio::test]
async fn duplicate_name_re_renders_with_message() {
    let client = MockClient::new().stub(
        DokkuCommand::AppsList,
        Ok(DokkuOutput::ok("=====> My Apps\nalpha")),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let body = get_ok(&app, "/apps/new", &cookie).await;
    let csrf = extract_csrf(&body);

    let resp = test::call_service(
        &app,
        form_request("/apps/new", format!("csrf_token={csrf}&name=alpha"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(
        body.contains("An app named &#39;alpha&#39; already exists."),
        "{body}"
    );
    assert!(body.contains(r#"value="alpha""#), "{body}");
}

#[tokio::test]
async fn valid_name_saves_draft_and_redirects_to_build_step() {
    let (state, _dir) = test_state().await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let body = get_ok(&app, "/apps/new", &cookie).await;
    let csrf = extract_csrf(&body);

    let resp = test::call_service(
        &app,
        form_request("/apps/new", format!("csrf_token={csrf}&name=my-app"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/new/build");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let body = get_ok(&app, "/apps/new", &cookie).await;
    assert!(
        body.contains(r#"value="my-app""#),
        "draft round-trips: {body}"
    );
}

#[tokio::test]
async fn reset_clears_the_saved_name() {
    let (state, _dir) = test_state().await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let body = get_ok(&app, "/apps/new", &cookie).await;
    let csrf = extract_csrf(&body);

    let resp = test::call_service(
        &app,
        form_request("/apps/new", format!("csrf_token={csrf}&name=my-app"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let body = get_ok(&app, "/apps/new", &cookie).await;
    assert!(body.contains(r#"value="my-app""#), "{body}");
    let csrf = extract_csrf(&body);

    let resp = test::call_service(
        &app,
        form_request("/apps/new/reset", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/new");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let body = get_ok(&app, "/apps/new", &cookie).await;
    assert!(body.contains("Wizard reset."), "{body}");
    assert!(
        body.contains(r#"name="name" type="text" required maxlength="64" value="""#),
        "empty input after reset: {body}"
    );
}
