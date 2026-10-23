mod common;

use std::collections::HashMap;

use actix_web::body::MessageBody;
use actix_web::cookie::Cookie;
use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::StatusCode;
use actix_web::test;

use common::{
    complete_setup, extract_csrf, form_request, get_body, location, login, response_cookie,
    run_url, seed_user_with_role, test_state, test_state_with_shared_client,
};
use dokku_ui::auth::rbac::Role;
use dokku_ui::dokku::{DokkuError, DokkuOutput, MockClient};
use dokku_ui::domain::ServicePlugin;
use dokku_ui::domain::app_wizard::{AppWizardDraft, VolumeMode, WizardService, WizardVolume};
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::web::{AppState, build_app};

const PLUGIN_LIST: &str = include_str!("fixtures/plugin_list.txt");
const REDIS_LIST: &str = include_str!("fixtures/redis_list.txt");
const LIST_ENTRIES_JSON: &str = include_str!("fixtures/list_entries.json");

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

/// Saves a valid name through step 1 and returns the session cookie holding
/// the draft, positioned at the build step.
async fn save_app_name<S, B, E>(app: &S, cookie: Cookie<'static>) -> Cookie<'static>
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = E>,
    B: MessageBody,
    E: std::fmt::Debug,
{
    save_app_name_as(app, cookie, "my-app").await
}

async fn save_app_name_as<S, B, E>(app: &S, cookie: Cookie<'static>, name: &str) -> Cookie<'static>
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = E>,
    B: MessageBody,
    E: std::fmt::Debug,
{
    let body = get_ok(app, "/apps/new", &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        app,
        form_request("/apps/new", format!("csrf_token={csrf}&name={name}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/new/build");
    response_cookie(&resp).unwrap_or(cookie)
}

fn no_build_mutation_calls(client: &MockClient) -> bool {
    client.calls().iter().all(|cmd| {
        !matches!(
            cmd,
            DokkuCommand::AppsCreate { .. }
                | DokkuCommand::BuilderSet { .. }
                | DokkuCommand::BuildpacksAdd { .. }
        )
    })
}

#[tokio::test]
async fn build_step_renders_after_name_saved() {
    let (state, _dir) = test_state().await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = get_ok(&app, "/apps/new/build", &cookie).await;
    assert!(body.contains(r#"action="/apps/new/build""#), "{body}");
    assert!(body.contains(r#"name="builder""#), "{body}");
    assert!(body.contains("<select"), "{body}");
    assert!(body.contains(r#"name="buildpacks""#), "{body}");
    assert!(body.contains("Auto (detect)"), "{body}");
    assert!(body.contains("Next: services"), "{body}");
}

#[tokio::test]
async fn build_step_redirects_to_name_step_without_a_name() {
    let (state, _dir) = test_state().await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/new/build")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/new");
}

#[tokio::test]
async fn auto_with_no_buildpacks_saves_and_never_calls_dokku() {
    let (state, client, _dir) = test_state_with_shared_client(MockClient::new()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = get_ok(&app, "/apps/new/build", &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/build",
            format!("csrf_token={csrf}&builder=&buildpacks="),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/new/services");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let body = get_ok(&app, "/apps/new/build", &cookie).await;
    assert!(
        body.contains(r#"value="" selected"#),
        "auto stays selected: {body}"
    );
    assert!(
        no_build_mutation_calls(&client),
        "no dokku calls during the build step: {:?}",
        client.calls()
    );
}

#[tokio::test]
async fn herokuish_with_one_buildpack_saves_and_redirects() {
    let (state, client, _dir) = test_state_with_shared_client(MockClient::new()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = get_ok(&app, "/apps/new/build", &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/build",
            format!("csrf_token={csrf}&builder=herokuish&buildpacks=https://example.com/bp-one"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/new/services");
    assert!(
        no_build_mutation_calls(&client),
        "no dokku calls during the build step: {:?}",
        client.calls()
    );
}

#[tokio::test]
async fn going_back_to_build_preserves_saved_builder_and_urls() {
    let (state, _dir) = test_state().await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = get_ok(&app, "/apps/new/build", &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/build",
            format!(
                "csrf_token={csrf}&builder=herokuish&buildpacks=https://example.com/bp-one%0Ahttps://example.com/bp-two"
            ),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/new/services");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let body = get_ok(&app, "/apps/new/build", &cookie).await;
    assert!(
        body.contains(r#"value="herokuish" selected"#),
        "builder kept: {body}"
    );
    assert!(body.contains("https://example.com/bp-one"), "{body}");
    assert!(body.contains("https://example.com/bp-two"), "{body}");
}

#[tokio::test]
async fn unknown_builder_re_renders_with_error() {
    let (state, client, _dir) = test_state_with_shared_client(MockClient::new()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = get_ok(&app, "/apps/new/build", &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/build",
            format!("csrf_token={csrf}&builder=podman&buildpacks="),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "re-render, not redirect");
    let body = get_body(resp).await;
    assert!(body.contains("Unknown builder"), "{body}");
    assert!(no_build_mutation_calls(&client), "{:?}", client.calls());
}

#[tokio::test]
async fn invalid_buildpack_re_renders_with_error() {
    let (state, client, _dir) = test_state_with_shared_client(MockClient::new()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = get_ok(&app, "/apps/new/build", &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/build",
            format!("csrf_token={csrf}&builder=&buildpacks=https://good.example/bp%0Ait%27s+bad"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("Invalid buildpack"), "{body}");
    assert!(
        body.contains("it&#39;s bad"),
        "offending line shown: {body}"
    );
    assert!(no_build_mutation_calls(&client), "{:?}", client.calls());
}

#[tokio::test]
async fn dockerfile_builder_with_buildpack_renders_conflict_error() {
    let (state, client, _dir) = test_state_with_shared_client(MockClient::new()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = get_ok(&app, "/apps/new/build", &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/build",
            format!("csrf_token={csrf}&builder=dockerfile&buildpacks=https://example.com/bp-one"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(
        body.contains("Buildpacks require the herokuish builder"),
        "{body}"
    );
    assert!(
        body.contains(r#"value="dockerfile" selected"#),
        "builder untouched: {body}"
    );
    assert!(
        body.contains("https://example.com/bp-one"),
        "urls kept: {body}"
    );
    assert!(no_build_mutation_calls(&client), "{:?}", client.calls());
}

fn redis_plugin() -> ServicePlugin {
    ServicePlugin::try_from("redis").expect("redis plugin")
}

fn postgres_plugin() -> ServicePlugin {
    ServicePlugin::try_from("postgres").expect("postgres plugin")
}

/// MockClient covering the services step's reads: snapshot apps, the
/// capability probe (version + plugin list), and `redis:list` only.
fn services_client() -> MockClient {
    MockClient::new()
        .stub(
            DokkuCommand::AppsList,
            Ok(DokkuOutput::ok("=====> My Apps")),
        )
        .stub(
            DokkuCommand::DokkuVersion,
            Ok(DokkuOutput::ok("dokku version 0.38.4\n")),
        )
        .stub(DokkuCommand::PluginList, Ok(DokkuOutput::ok(PLUGIN_LIST)))
        .stub(
            DokkuCommand::ServiceList {
                plugin: redis_plugin(),
            },
            Ok(DokkuOutput::ok(REDIS_LIST)),
        )
}

fn no_service_mutation_calls(client: &MockClient) -> bool {
    !client.calls().iter().any(|cmd| {
        matches!(
            cmd,
            DokkuCommand::ServiceCreate { .. } | DokkuCommand::ServiceLink { .. }
        )
    })
}

#[tokio::test]
async fn services_step_redirects_to_name_step_without_a_name() {
    let (state, _dir) = test_state().await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/new/services")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/new");
}

#[tokio::test]
async fn services_step_lists_installed_plugins_and_existing_services() {
    let (state, client, _dir) = test_state_with_shared_client(services_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = get_ok(&app, "/apps/new/services", &cookie).await;
    assert!(body.contains(r#"value="redis:candid""#), "{body}");
    assert!(body.contains("Next: volumes"), "{body}");
    assert!(body.contains(r#"value="postgres""#), "{body}");
    assert!(body.contains("PostgreSQL"), "{body}");
    assert!(
        !body.contains("MariaDB"),
        "uninstalled plugin offered: {body}"
    );
    assert!(
        body.contains("No services selected yet"),
        "empty selection table: {body}"
    );
    assert!(no_service_mutation_calls(&client), "{:?}", client.calls());
}

#[tokio::test]
async fn attach_existing_service_appends_it_to_the_draft() {
    let (state, client, _dir) = test_state_with_shared_client(services_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = get_ok(&app, "/apps/new/services", &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/services/attach",
            format!("csrf_token={csrf}&service_key=redis%3Acandid"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/new/services");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let body = get_ok(&app, "/apps/new/services", &cookie).await;
    assert!(body.contains(">candid</td>"), "row shown: {body}");
    assert!(body.contains(">attach</td>"), "mode label: {body}");
    assert!(body.contains("Redis</td>"), "plugin label: {body}");
    assert!(no_service_mutation_calls(&client), "{:?}", client.calls());
}

#[tokio::test]
async fn create_new_service_appends_it_with_the_create_flag() {
    let (state, client, _dir) = test_state_with_shared_client(services_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = get_ok(&app, "/apps/new/services", &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/services/create",
            format!("csrf_token={csrf}&plugin=postgres&service=db-one"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/new/services");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let body = get_ok(&app, "/apps/new/services", &cookie).await;
    assert!(body.contains(">db-one</td>"), "row shown: {body}");
    assert!(body.contains("create + link</td>"), "mode label: {body}");
    assert!(no_service_mutation_calls(&client), "{:?}", client.calls());
}

#[tokio::test]
async fn creating_a_service_that_already_exists_is_rejected() {
    let (state, client, _dir) = test_state_with_shared_client(services_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = get_ok(&app, "/apps/new/services", &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/services/create",
            format!("csrf_token={csrf}&plugin=redis&service=candid"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("already exists; attach it instead"), "{body}");
    assert!(
        body.contains("No services selected yet"),
        "draft untouched: {body}"
    );
    assert!(no_service_mutation_calls(&client), "{:?}", client.calls());
}

#[tokio::test]
async fn duplicate_attach_is_rejected() {
    let (state, client, _dir) = test_state_with_shared_client(services_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = get_ok(&app, "/apps/new/services", &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/services/attach",
            format!("csrf_token={csrf}&service_key=redis%3Acandid"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let body = get_ok(&app, "/apps/new/services", &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/services/attach",
            format!("csrf_token={csrf}&service_key=redis%3Acandid"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("is already selected"), "{body}");
    assert!(
        body.matches(">candid</td>").count() == 1,
        "one row only: {body}"
    );
    assert!(no_service_mutation_calls(&client), "{:?}", client.calls());
}

#[tokio::test]
async fn attach_or_create_with_invalid_names_never_touches_the_draft() {
    let (state, client, _dir) = test_state_with_shared_client(services_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = get_ok(&app, "/apps/new/services", &cookie).await;
    let csrf = extract_csrf(&body);
    for (path, form, needle) in [
        (
            "/apps/new/services/attach",
            "service_key=no-service".to_owned(),
            "Choose a service to attach.",
        ),
        (
            "/apps/new/services/attach",
            "service_key=nope%3Acandid".to_owned(),
            "Unknown service plugin",
        ),
        (
            "/apps/new/services/attach",
            "service_key=redis%3ABad Name".to_owned(),
            "Invalid service name",
        ),
        (
            "/apps/new/services/create",
            "plugin=nope&service=ok-one".to_owned(),
            "Unknown service plugin",
        ),
        (
            "/apps/new/services/create",
            "plugin=redis&service=Bad%20Name".to_owned(),
            "Invalid service name",
        ),
    ] {
        let resp = test::call_service(
            &app,
            form_request(path, format!("csrf_token={csrf}&{form}"))
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK, "{path} {form}");
        let body = get_body(resp).await;
        assert!(body.contains(needle), "{needle} missing from {body}");
        assert!(
            body.contains("No services selected yet"),
            "draft untouched after {path}: {body}"
        );
    }
    assert!(no_service_mutation_calls(&client), "{:?}", client.calls());
}

#[tokio::test]
async fn remove_drops_the_row_and_out_of_range_indexes_are_no_ops() {
    let (state, client, _dir) = test_state_with_shared_client(services_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = get_ok(&app, "/apps/new/services", &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/services/attach",
            format!("csrf_token={csrf}&service_key=redis%3Acandid"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/services/remove",
            format!("csrf_token={csrf}&index=9"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let cookie = response_cookie(&resp).unwrap_or(cookie);
    let body = get_ok(&app, "/apps/new/services", &cookie).await;
    assert!(body.contains(">candid</td>"), "row kept: {body}");

    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/services/remove",
            format!("csrf_token={csrf}&index=0"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let cookie = response_cookie(&resp).unwrap_or(cookie);
    let body = get_ok(&app, "/apps/new/services", &cookie).await;
    assert!(
        body.contains("No services selected yet"),
        "row removed: {body}"
    );
    assert!(no_service_mutation_calls(&client), "{:?}", client.calls());
}

#[tokio::test]
async fn failed_capability_probe_shows_a_notice_but_keeps_next() {
    let client = services_client().stub(
        DokkuCommand::DokkuVersion,
        Err(DokkuError::Connect("ssh refused".to_owned())),
    );
    let (state, client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = get_ok(&app, "/apps/new/services", &cookie).await;
    assert!(
        body.contains("Could not read the host's installed service plugins"),
        "notice: {body}"
    );
    assert!(body.contains("Next: volumes"), "next kept: {body}");
    assert!(
        body.contains("failed to connect to dokku host: ssh refused"),
        "reason shown: {body}"
    );
    assert!(no_service_mutation_calls(&client), "{:?}", client.calls());
}

#[tokio::test]
async fn a_failing_plugin_list_shows_its_notice_and_still_offers_others() {
    let client = services_client().stub(
        DokkuCommand::ServiceList {
            plugin: redis_plugin(),
        },
        Err(DokkuError::Exit {
            code: 2,
            stderr: "xargs: unmatched single quote".to_owned(),
        }),
    );
    let (state, client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = get_ok(&app, "/apps/new/services", &cookie).await;
    assert!(
        body.contains("Could not read redis services:"),
        "notice shown: {body}"
    );
    assert!(body.contains("PostgreSQL"), "other plugin offered: {body}");
    assert!(body.contains("Next: volumes"), "next kept: {body}");
    assert!(no_service_mutation_calls(&client), "{:?}", client.calls());
}

#[tokio::test]
async fn failed_plugin_list_on_attach_prompts_a_retry() {
    let client = services_client().stub(
        DokkuCommand::ServiceList {
            plugin: postgres_plugin(),
        },
        Err(DokkuError::Timeout { secs: 30 }),
    );
    let (state, client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = get_ok(&app, "/apps/new/services", &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/services/attach",
            format!("csrf_token={csrf}&service_key=postgres%3Acandid"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(
        body.contains("Could not read PostgreSQL services; retry."),
        "{body}"
    );
    assert!(no_service_mutation_calls(&client), "{:?}", client.calls());
}

#[tokio::test]
async fn service_step_posts_require_csrf() {
    let (state, _client, _dir) = test_state_with_shared_client(services_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    for (path, form) in [
        ("/apps/new/services/attach", "service_key=redis%3Acandid"),
        (
            "/apps/new/services/create",
            "plugin=redis&service=fresh-one",
        ),
        ("/apps/new/services/remove", "index=0"),
    ] {
        let resp = test::call_service(
            &app,
            form_request(path, format!("csrf_token=not-a-token&{form}"))
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{path}");
    }
}

/// MockClient covering the volumes step's only read: the registered entries
/// from `tests/fixtures/list_entries.json`.
fn volumes_client() -> MockClient {
    MockClient::new()
        .stub(
            DokkuCommand::AppsList,
            Ok(DokkuOutput::ok("=====> My Apps")),
        )
        .stub(
            DokkuCommand::StorageListEntries,
            Ok(DokkuOutput::ok(LIST_ENTRIES_JSON)),
        )
}

fn no_volume_mutation_calls(client: &MockClient) -> bool {
    !client.calls().iter().any(|cmd| {
        matches!(
            cmd,
            DokkuCommand::StorageCreate { .. }
                | DokkuCommand::StorageMount { .. }
                | DokkuCommand::StorageMountEntry { .. }
        )
    })
}

async fn volumes_page<S, B, E>(app: &S, cookie: &Cookie<'static>) -> String
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = E>,
    B: MessageBody,
    E: std::fmt::Debug,
{
    get_ok(app, "/apps/new/volumes", cookie).await
}

#[tokio::test]
async fn volumes_step_redirects_to_name_step_without_a_name() {
    let (state, _dir) = test_state().await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/new/volumes")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/new");
}

#[tokio::test]
async fn volumes_step_lists_registered_entries_from_the_fixture() {
    let (state, client, _dir) = test_state_with_shared_client(volumes_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = volumes_page(&app, &cookie).await;
    assert!(body.contains(r#"value="legacy-151e4f1a23""#), "{body}");
    assert!(
        body.contains("/var/lib/dokku/data/storage/candid/uploads"),
        "entry label carries the host path: {body}"
    );
    assert!(body.contains("Review plan"), "{body}");
    assert!(body.contains("No volumes selected yet"), "{body}");
    assert!(no_volume_mutation_calls(&client), "{:?}", client.calls());
}

#[tokio::test]
async fn create_volume_appends_a_create_named_entry_row() {
    let (state, client, _dir) = test_state_with_shared_client(volumes_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = volumes_page(&app, &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/volumes/create",
            format!("csrf_token={csrf}&entry=fresh-logs&container=%2Fapp%2Flogs"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/new/volumes");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let body = volumes_page(&app, &cookie).await;
    assert!(body.contains(">create named entry</td>"), "mode: {body}");
    assert!(body.contains(">fresh-logs</td>"), "source: {body}");
    assert!(body.contains(">/app/logs</td>"), "container: {body}");
    assert!(no_volume_mutation_calls(&client), "{:?}", client.calls());
}

#[tokio::test]
async fn create_volume_for_a_registered_entry_is_rejected_with_an_attach_hint() {
    let client = volumes_client().stub(
        DokkuCommand::StorageListEntries,
        Ok(DokkuOutput::ok(
            r#"[{"name":"uploads","host_path":"/var/lib/dokku/data/storage/uploads"}]"#,
        )),
    );
    let (state, client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = volumes_page(&app, &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/volumes/create",
            format!("csrf_token={csrf}&entry=uploads&container=%2Fapp%2Fuploads"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("already exists; attach it instead"), "{body}");
    assert!(
        body.contains("No volumes selected yet"),
        "draft untouched: {body}"
    );
    assert!(no_volume_mutation_calls(&client), "{:?}", client.calls());
}

#[tokio::test]
async fn attach_volume_entry_appends_it_including_a_fixture_legacy_entry() {
    let (state, client, _dir) = test_state_with_shared_client(volumes_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = volumes_page(&app, &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/volumes/attach-entry",
            format!("csrf_token={csrf}&entry=legacy-151e4f1a23&container=%2Fdata"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/new/volumes");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let body = volumes_page(&app, &cookie).await;
    assert!(body.contains(">attach entry</td>"), "mode: {body}");
    assert!(body.contains(">legacy-151e4f1a23</td>"), "source: {body}");
    assert!(body.contains(">/data</td>"), "container: {body}");
    assert!(no_volume_mutation_calls(&client), "{:?}", client.calls());
}

#[tokio::test]
async fn attach_volume_path_appends_an_attach_host_path_row() {
    let (state, client, _dir) = test_state_with_shared_client(volumes_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = volumes_page(&app, &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/volumes/attach-path",
            format!(
                "csrf_token={csrf}&host=%2Fvar%2Flib%2Fdokku%2Fdata%2Fstorage%2Fshared&container=%2Fmnt%2Fshared"
            ),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/new/volumes");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let body = volumes_page(&app, &cookie).await;
    assert!(body.contains(">attach host path</td>"), "mode: {body}");
    assert!(
        body.contains(">/var/lib/dokku/data/storage/shared</td>"),
        "source kept in colon-form halves: {body}"
    );
    assert!(body.contains(">/mnt/shared</td>"), "container: {body}");
    assert!(no_volume_mutation_calls(&client), "{:?}", client.calls());
}

#[tokio::test]
async fn duplicate_volume_is_rejected_and_the_draft_keeps_one_row() {
    let (state, client, _dir) = test_state_with_shared_client(volumes_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = volumes_page(&app, &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/volumes/attach-path",
            format!("csrf_token={csrf}&host=%2Fhost&container=%2Fmnt%2Fshared"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let body = volumes_page(&app, &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/volumes/attach-path",
            format!("csrf_token={csrf}&host=%2Fhost&container=%2Fmnt%2Fshared"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("is already selected"), "{body}");
    assert!(
        body.matches(">/mnt/shared</td>").count() == 1,
        "one row only: {body}"
    );
    assert!(no_volume_mutation_calls(&client), "{:?}", client.calls());
}

#[tokio::test]
async fn invalid_volume_values_never_touch_the_draft() {
    let (state, client, _dir) = test_state_with_shared_client(volumes_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = volumes_page(&app, &cookie).await;
    let csrf = extract_csrf(&body);
    for (path, form, needle) in [
        (
            "/apps/new/volumes/create",
            "entry=Bad%20Entry&container=%2Fdata".to_owned(),
            "Invalid storage entry name",
        ),
        (
            "/apps/new/volumes/create",
            "entry=legacy-151e4f1a23&container=%2Fdata".to_owned(),
            "reserved by dokku",
        ),
        (
            "/apps/new/volumes/create",
            "entry=uploads&container=relative%2Fpath".to_owned(),
            "Invalid container path",
        ),
        (
            "/apps/new/volumes/attach-entry",
            "entry=nope-one&container=%2Fdata".to_owned(),
            "No storage entry named",
        ),
        (
            "/apps/new/volumes/attach-path",
            "host=%2Fbad+path&container=%2Fdata".to_owned(),
            "must not contain whitespace",
        ),
        (
            "/apps/new/volumes/attach-path",
            "host=%2Fhost&container=relative".to_owned(),
            "container path must be absolute",
        ),
    ] {
        let resp = test::call_service(
            &app,
            form_request(path, format!("csrf_token={csrf}&{form}"))
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK, "{path} {form}");
        let body = get_body(resp).await;
        assert!(body.contains(needle), "{needle} missing from {body}");
        assert!(
            body.contains("No volumes selected yet"),
            "draft untouched after {path}: {body}"
        );
    }
    assert!(no_volume_mutation_calls(&client), "{:?}", client.calls());
}

#[tokio::test]
async fn remove_drops_the_volume_row_and_out_of_range_indexes_are_no_ops() {
    let (state, client, _dir) = test_state_with_shared_client(volumes_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = volumes_page(&app, &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/volumes/create",
            format!("csrf_token={csrf}&entry=fresh-logs&container=%2Fapp%2Flogs"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let body = volumes_page(&app, &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/volumes/attach-path",
            format!("csrf_token={csrf}&host=%2Fhost&container=%2Fmnt%2Fshared"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let body = volumes_page(&app, &cookie).await;
    assert!(body.contains(">fresh-logs</td>"), "two rows first: {body}");
    assert!(body.contains(">/mnt/shared</td>"), "two rows first: {body}");

    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/volumes/remove",
            format!("csrf_token={csrf}&index=9"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let cookie = response_cookie(&resp).unwrap_or(cookie);
    let body = volumes_page(&app, &cookie).await;
    assert!(body.contains(">fresh-logs</td>"), "rows kept: {body}");

    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/volumes/remove",
            format!("csrf_token={csrf}&index=0"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let cookie = response_cookie(&resp).unwrap_or(cookie);
    let body = volumes_page(&app, &cookie).await;
    assert!(
        !body.contains(">fresh-logs</td>"),
        "first row removed: {body}"
    );
    assert!(
        body.contains(">/mnt/shared</td>"),
        "second row kept: {body}"
    );
    assert!(no_volume_mutation_calls(&client), "{:?}", client.calls());
}

#[tokio::test]
async fn failed_volume_entry_list_shows_a_notice_but_allows_create_path_and_review() {
    let client = volumes_client().stub(
        DokkuCommand::StorageListEntries,
        Err(DokkuError::Connect("ssh refused".to_owned())),
    );
    let (state, client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = volumes_page(&app, &cookie).await;
    assert!(
        body.contains("Could not read storage entries:"),
        "notice: {body}"
    );
    assert!(
        body.contains("failed to connect to dokku host: ssh refused"),
        "reason shown: {body}"
    );
    assert!(body.contains("Review plan"), "review kept: {body}");
    assert!(
        !body.contains("/apps/new/volumes/attach-entry"),
        "attach-entry hidden: {body}"
    );

    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/volumes/create",
            format!("csrf_token={csrf}&entry=fresh-logs&container=%2Fapp%2Flogs"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER, "create still works");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let body = volumes_page(&app, &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/volumes/attach-path",
            format!("csrf_token={csrf}&host=%2Fhost&container=%2Fmnt%2Fshared"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER, "attach-path works");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let body = volumes_page(&app, &cookie).await;
    assert!(body.contains(">fresh-logs</td>"), "create row: {body}");
    assert!(body.contains(">attach host path</td>"), "path row: {body}");
    assert!(no_volume_mutation_calls(&client), "{:?}", client.calls());
}

#[tokio::test]
async fn failed_volume_entry_list_on_attach_entry_prompts_a_retry() {
    let client = volumes_client().stub(
        DokkuCommand::StorageListEntries,
        Err(DokkuError::Exit {
            code: 2,
            stderr: "xargs: unmatched single quote".to_owned(),
        }),
    );
    let (state, client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    let body = volumes_page(&app, &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/volumes/attach-entry",
            format!("csrf_token={csrf}&entry=legacy-151e4f1a23&container=%2Fdata"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(
        body.contains("Could not read storage entries; retry."),
        "{body}"
    );
    assert!(no_volume_mutation_calls(&client), "{:?}", client.calls());
}

#[tokio::test]
async fn volume_step_posts_require_csrf() {
    let (state, _client, _dir) = test_state_with_shared_client(volumes_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name(&app, cookie).await;

    for (path, form) in [
        (
            "/apps/new/volumes/create",
            "entry=fresh-logs&container=%2Fapp%2Flogs",
        ),
        (
            "/apps/new/volumes/attach-entry",
            "entry=legacy-151e4f1a23&container=%2Fdata",
        ),
        (
            "/apps/new/volumes/attach-path",
            "host=%2Fhost&container=%2Fmnt%2Fshared",
        ),
        ("/apps/new/volumes/remove", "index=0"),
    ] {
        let resp = test::call_service(
            &app,
            form_request(path, format!("csrf_token=not-a-token&{form}"))
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{path}");
    }
}

/// MockClient covering every wizard read: apps, the capability probe, the
/// installed datastore service lists, and the registered storage entries.
fn wizard_client() -> MockClient {
    services_client()
        .stub(
            DokkuCommand::ServiceList {
                plugin: postgres_plugin(),
            },
            Ok(DokkuOutput::ok(include_str!("fixtures/postgres_list.txt"))),
        )
        .stub(
            DokkuCommand::ServiceList {
                plugin: ServicePlugin::try_from("mysql").expect("mysql plugin"),
            },
            Ok(DokkuOutput::ok(include_str!("fixtures/mysql_list.txt"))),
        )
        .stub(
            DokkuCommand::ServiceList {
                plugin: ServicePlugin::try_from("mongo").expect("mongo plugin"),
            },
            Ok(DokkuOutput::ok(include_str!("fixtures/mongo_list.txt"))),
        )
        .stub(
            DokkuCommand::StorageListEntries,
            Ok(DokkuOutput::ok(LIST_ENTRIES_JSON)),
        )
}

fn hx_form_request(path: &str, body: String) -> actix_web::test::TestRequest {
    form_request(path, body).insert_header(("HX-Request", "true"))
}

fn mutation_calls(client: &MockClient) -> Vec<DokkuCommand> {
    client
        .calls()
        .into_iter()
        .filter(|cmd| {
            matches!(
                cmd,
                DokkuCommand::AppsCreate { .. }
                    | DokkuCommand::BuilderSet { .. }
                    | DokkuCommand::BuildpacksAdd { .. }
                    | DokkuCommand::ServiceCreate { .. }
                    | DokkuCommand::ServiceLink { .. }
                    | DokkuCommand::StorageCreate { .. }
                    | DokkuCommand::StorageMount { .. }
                    | DokkuCommand::StorageMountEntry { .. }
            )
        })
        .collect()
}

async fn wait_for_mutations(client: &MockClient, count: usize) -> Vec<DokkuCommand> {
    for _ in 0..200 {
        let calls = mutation_calls(client);
        if calls.len() >= count {
            return calls;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    mutation_calls(client)
}

async fn save_build_step<S, B, E>(
    app: &S,
    cookie: Cookie<'static>,
    builder: &str,
    buildpacks: &str,
) -> Cookie<'static>
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = E>,
    B: MessageBody,
    E: std::fmt::Debug,
{
    let body = get_ok(app, "/apps/new/build", &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        app,
        form_request(
            "/apps/new/build",
            format!("csrf_token={csrf}&builder={builder}&buildpacks={buildpacks}"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/new/services");
    response_cookie(&resp).unwrap_or(cookie)
}

async fn add_created_service<S, B, E>(
    app: &S,
    cookie: Cookie<'static>,
    plugin: &str,
    service: &str,
) -> Cookie<'static>
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = E>,
    B: MessageBody,
    E: std::fmt::Debug,
{
    let body = get_ok(app, "/apps/new/services", &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        app,
        form_request(
            "/apps/new/services/create",
            format!("csrf_token={csrf}&plugin={plugin}&service={service}"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/new/services");
    response_cookie(&resp).unwrap_or(cookie)
}

async fn add_created_volume<S, B, E>(
    app: &S,
    cookie: Cookie<'static>,
    entry: &str,
    container: &str,
) -> Cookie<'static>
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = E>,
    B: MessageBody,
    E: std::fmt::Debug,
{
    let body = get_ok(app, "/apps/new/volumes", &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        app,
        form_request(
            "/apps/new/volumes/create",
            format!(
                "csrf_token={csrf}&entry={entry}&container={}",
                urlencode(container)
            ),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/new/volumes");
    response_cookie(&resp).unwrap_or(cookie)
}

fn urlencode(input: &str) -> String {
    input
        .bytes()
        .flat_map(|byte| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                vec![byte as char]
            }
            other => format!("%{other:02X}").chars().collect(),
        })
        .collect()
}

/// Writes a draft straight into the shared SQLite session row. The route
/// steps validate their own input, so this is how a stale/hand-edited draft
/// (the executor's last line of defence) is simulated.
async fn inject_draft(state: &AppState, _cookie: &Cookie<'static>, draft: &AppWizardDraft) {
    // The middleware signs the cookie value, so the raw session key is not
    // recoverable from it here. The harness has a single live session; patch
    // every row (normally exactly one) with the draft value.
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT id, data FROM sessions")
        .fetch_all(&state.db)
        .await
        .expect("session rows");
    assert!(!rows.is_empty(), "setup created a session");
    for (id, raw) in rows {
        let mut data: HashMap<String, String> = serde_json::from_str(&raw).expect("session json");
        data.insert(
            "app_wizard_draft".to_owned(),
            serde_json::to_string(draft).expect("draft json"),
        );
        sqlx::query("UPDATE sessions SET data = ? WHERE id = ?")
            .bind(serde_json::to_string(&data).expect("session json"))
            .bind(id)
            .execute(&state.db)
            .await
            .expect("update session");
    }
}

fn full_draft() -> AppWizardDraft {
    AppWizardDraft {
        name: Some("alpha".into()),
        builder: None,
        buildpacks: vec!["https://example.com/bp".into()],
        services: vec![WizardService {
            plugin: "postgres".into(),
            service: "cache".into(),
            create: true,
        }],
        volumes: vec![WizardVolume {
            mode: VolumeMode::Create,
            source: "uploads".into(),
            container: "/app/uploads".into(),
        }],
    }
}

async fn review_csrf<S, B, E>(app: &S, cookie: &Cookie<'static>) -> String
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = E>,
    B: MessageBody,
    E: std::fmt::Debug,
{
    extract_csrf(&get_ok(app, "/apps/new/review", cookie).await)
}

#[tokio::test]
async fn review_step_requires_a_name() {
    let (state, _dir) = test_state().await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/apps/new/review")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/new");
}

#[tokio::test]
async fn review_shows_the_exact_plan_collected_through_the_steps() {
    let (state, client, _dir) = test_state_with_shared_client(wizard_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name_as(&app, cookie, "alpha").await;
    let cookie = save_build_step(&app, cookie, "", "https://example.com/bp").await;
    let cookie = add_created_service(&app, cookie, "postgres", "cache").await;
    let cookie = add_created_volume(&app, cookie, "uploads", "/app/uploads").await;

    let body = get_ok(&app, "/apps/new/review", &cookie).await;
    assert!(body.contains("alpha"), "name shown: {body}");
    assert!(body.contains("Auto -&#62; herokuish"), "builder: {body}");
    assert!(body.contains("https://example.com/bp"), "buildpack: {body}");
    assert!(body.contains(">cache</td>"), "service row: {body}");
    assert!(body.contains("create + link</td>"), "service mode: {body}");
    assert!(body.contains(">uploads</td>"), "volume row: {body}");
    assert!(body.contains(">/app/uploads</td>"), "volume path: {body}");
    assert!(body.contains(r#"action="/apps/new/execute""#), "{body}");

    let commands = [
        "apps:create alpha",
        "builder:set alpha selected herokuish",
        "buildpacks:add alpha https://example.com/bp",
        "postgres:create cache",
        "postgres:link cache alpha",
        "storage:create uploads",
        "storage:mount alpha uploads --container-dir /app/uploads",
    ];
    let mut search_from = 0;
    for command in commands {
        let offset = body[search_from..]
            .find(command)
            .unwrap_or_else(|| panic!("missing {command} in order: {body}"));
        search_from += offset + command.len();
    }

    assert!(
        mutation_calls(&client).is_empty(),
        "review must not execute commands: {:?}",
        client.calls()
    );
}

#[tokio::test]
async fn review_renders_attach_path_with_the_colon_form_mount() {
    let (state, client, _dir) = test_state_with_shared_client(wizard_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let cookie = save_app_name_as(&app, cookie, "alpha").await;
    let body = get_ok(&app, "/apps/new/volumes", &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/apps/new/volumes/attach-path",
            format!("csrf_token={csrf}&host=%2Fsrv%2Ffiles&container=%2Fapp%2Ffiles"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let body = get_ok(&app, "/apps/new/review", &cookie).await;
    assert!(
        body.contains("storage:mount alpha /srv/files:/app/files"),
        "{body}"
    );
    assert!(mutation_calls(&client).is_empty(), "{:?}", client.calls());
}

#[tokio::test]
async fn review_renders_an_invalid_draft_with_an_error_and_no_execute_form() {
    let (state, client, _dir) = test_state_with_shared_client(wizard_client()).await;
    let app = test::init_service(build_app(state.clone())).await;
    let cookie = complete_setup(&app).await;
    let mut draft = full_draft();
    draft.builder = Some("dockerfile".into());
    inject_draft(&state, &cookie, &draft).await;

    let body = get_ok(&app, "/apps/new/review", &cookie).await;
    assert!(
        body.contains("buildpacks require the herokuish builder"),
        "{body}"
    );
    assert!(
        !body.contains(r#"action="/apps/new/execute""#),
        "no execute form: {body}"
    );
    assert!(!body.contains("apps:create alpha"), "{body}");
    assert!(mutation_calls(&client).is_empty(), "{:?}", client.calls());
}

#[tokio::test]
async fn execute_htmx_enqueues_one_job_that_runs_the_plan_in_order() {
    let (state, client, _dir) = test_state_with_shared_client(wizard_client()).await;
    let app = test::init_service(build_app(state.clone())).await;
    let cookie = complete_setup(&app).await;
    inject_draft(&state, &cookie, &full_draft()).await;
    let csrf = review_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request("/apps/new/execute", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains(r#"data-run-url="/actions/runs/"#), "{body}");
    assert!(body.contains("Creating alpha and resources..."), "{body}");
    let run_url = run_url(&body);
    assert!(run_url.ends_with("/events"), "{run_url}");

    let calls = wait_for_mutations(&client, 7).await;
    assert_eq!(
        calls,
        vec![
            DokkuCommand::AppsCreate {
                app: "alpha".try_into().expect("app"),
            },
            DokkuCommand::BuilderSet {
                app: "alpha".try_into().expect("app"),
                property: "selected".into(),
                value: Some("herokuish".into()),
            },
            DokkuCommand::BuildpacksAdd {
                app: "alpha".try_into().expect("app"),
                buildpack: "https://example.com/bp".into(),
                index: None,
            },
            DokkuCommand::ServiceCreate {
                plugin: postgres_plugin(),
                service: "cache".try_into().expect("service"),
                options: Default::default(),
            },
            DokkuCommand::ServiceLink {
                plugin: postgres_plugin(),
                service: "cache".try_into().expect("service"),
                app: "alpha".try_into().expect("app"),
            },
            DokkuCommand::StorageCreate {
                entry: "uploads".try_into().expect("entry"),
            },
            DokkuCommand::StorageMountEntry {
                app: "alpha".try_into().expect("app"),
                entry: "uploads".try_into().expect("entry"),
                container: "/app/uploads".into(),
            },
        ]
    );

    let jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM action_jobs")
        .fetch_one(&state.db)
        .await
        .expect("count jobs");
    assert_eq!(jobs, 1, "one combined persisted job");
}

#[tokio::test]
async fn execute_service_attach_only_omits_service_create() {
    let (state, client, _dir) = test_state_with_shared_client(wizard_client()).await;
    let app = test::init_service(build_app(state.clone())).await;
    let cookie = complete_setup(&app).await;
    inject_draft(
        &state,
        &cookie,
        &AppWizardDraft {
            name: Some("alpha".into()),
            services: vec![WizardService {
                plugin: "redis".into(),
                service: "candid".into(),
                create: false,
            }],
            ..Default::default()
        },
    )
    .await;
    let csrf = review_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request("/apps/new/execute", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let calls = wait_for_mutations(&client, 2).await;
    assert_eq!(
        calls,
        vec![
            DokkuCommand::AppsCreate {
                app: "alpha".try_into().expect("app"),
            },
            DokkuCommand::ServiceLink {
                plugin: redis_plugin(),
                service: "candid".try_into().expect("service"),
                app: "alpha".try_into().expect("app"),
            },
        ]
    );
}

#[tokio::test]
async fn execute_attach_path_uses_the_colon_form_storage_mount() {
    let (state, client, _dir) = test_state_with_shared_client(wizard_client()).await;
    let app = test::init_service(build_app(state.clone())).await;
    let cookie = complete_setup(&app).await;
    inject_draft(
        &state,
        &cookie,
        &AppWizardDraft {
            name: Some("alpha".into()),
            volumes: vec![WizardVolume {
                mode: VolumeMode::AttachPath,
                source: "/srv/files".into(),
                container: "/app/files".into(),
            }],
            ..Default::default()
        },
    )
    .await;
    let csrf = review_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request("/apps/new/execute", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let calls = wait_for_mutations(&client, 2).await;
    assert!(
        matches!(
            calls.get(1),
            Some(DokkuCommand::StorageMount { app, mount })
                if app.as_str() == "alpha" && mount.arg() == "/srv/files:/app/files"
        ),
        "{calls:?}"
    );
}

#[tokio::test]
async fn execute_without_htmx_flashes_queued_and_redirects_to_activity() {
    let (state, _client, _dir) = test_state_with_shared_client(wizard_client()).await;
    let app = test::init_service(build_app(state.clone())).await;
    let cookie = complete_setup(&app).await;
    inject_draft(&state, &cookie, &full_draft()).await;
    let csrf = review_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        form_request("/apps/new/execute", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/activity");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let body = get_ok(&app, "/activity", &cookie).await;
    assert!(body.contains("Queued: creating app"), "{body}");
    assert!(body.contains("alpha"), "{body}");

    let body = get_ok(&app, "/apps/new", &cookie).await;
    assert!(
        body.contains(r#"name="name" type="text" required maxlength="64" value="""#),
        "draft cleared: {body}"
    );
}

#[tokio::test]
async fn execute_invalid_draft_renders_a_modal_error_and_enqueues_nothing() {
    let (state, client, _dir) = test_state_with_shared_client(wizard_client()).await;
    let app = test::init_service(build_app(state.clone())).await;
    let cookie = complete_setup(&app).await;
    let mut draft = full_draft();
    draft.builder = Some("dockerfile".into());
    inject_draft(&state, &cookie, &draft).await;
    let csrf = review_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request("/apps/new/execute", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(
        body.contains("buildpacks require the herokuish builder"),
        "{body}"
    );
    assert!(!body.contains("data-run-url"), "{body}");

    let jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM action_jobs")
        .fetch_one(&state.db)
        .await
        .expect("count jobs");
    assert_eq!(jobs, 0);
    assert!(mutation_calls(&client).is_empty(), "{:?}", client.calls());

    let resp = test::call_service(
        &app,
        form_request("/apps/new/execute", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/new/review");
}

#[tokio::test]
async fn execute_without_a_name_flashes_and_redirects_to_name_step() {
    let (state, _dir) = test_state().await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let body = get_ok(&app, "/apps/new", &cookie).await;
    let csrf = extract_csrf(&body);

    let resp = test::call_service(
        &app,
        form_request("/apps/new/execute", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/new");
    let cookie = response_cookie(&resp).unwrap_or(cookie);
    let body = get_ok(&app, "/apps/new", &cookie).await;
    assert!(body.contains("Start by naming the app."), "{body}");
}

#[tokio::test]
async fn execute_requires_csrf() {
    let (state, _dir) = test_state().await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        form_request("/apps/new/execute", "csrf_token=not-a-token".to_owned())
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

const WIZARD_GET_ROUTES: [&str; 5] = [
    "/apps/new",
    "/apps/new/build",
    "/apps/new/services",
    "/apps/new/volumes",
    "/apps/new/review",
];

/// Paths and minimal form bodies for every wizard mutation route.
const WIZARD_MUTATION_ROUTES: [(&str, &str); 11] = [
    ("/apps/new", "name=alpha"),
    ("/apps/new/reset", ""),
    ("/apps/new/build", "builder=&buildpacks="),
    ("/apps/new/services/attach", "service_key=redis%3Acandid"),
    ("/apps/new/services/create", "plugin=redis&service=fresh"),
    ("/apps/new/services/remove", "index=0"),
    ("/apps/new/volumes/create", "entry=logs&container=%2Flogs"),
    (
        "/apps/new/volumes/attach-entry",
        "entry=legacy-151e4f1a23&container=%2Fdata",
    ),
    (
        "/apps/new/volumes/attach-path",
        "host=%2Fhost&container=%2Fdata",
    ),
    ("/apps/new/volumes/remove", "index=0"),
    ("/apps/new/execute", ""),
];

#[tokio::test]
async fn every_wizard_get_route_redirects_to_login_when_unauthenticated() {
    let (state, _dir) = test_state().await;
    seed_user_with_role(
        &state,
        "admin@example.com",
        "correct-horse-battery",
        Role::Admin,
    )
    .await;
    let app = test::init_service(build_app(state)).await;

    for path in WIZARD_GET_ROUTES {
        let resp = test::call_service(&app, test::TestRequest::get().uri(path).to_request()).await;
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT, "{path}");
        assert_eq!(location(&resp), "/login", "{path}");
    }
}

#[tokio::test]
async fn every_wizard_mutation_post_is_forbidden_for_viewers() {
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
    let csrf = extract_csrf(&get_ok(&app, "/", &viewer).await);

    for (path, body) in WIZARD_MUTATION_ROUTES {
        let resp = test::call_service(
            &app,
            form_request(path, format!("csrf_token={csrf}&{body}"))
                .cookie(viewer.clone())
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{path}");
    }
}

#[tokio::test]
async fn every_wizard_mutation_post_requires_csrf() {
    let (state, _dir) = test_state().await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    for (path, body) in WIZARD_MUTATION_ROUTES {
        let resp = test::call_service(
            &app,
            form_request(path, format!("csrf_token=not-a-token&{body}"))
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "{path} missing token"
        );
    }
}
