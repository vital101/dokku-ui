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
use dokku_ui::dokku::{DokkuError, DokkuOutput, MockClient};
use dokku_ui::domain::ServicePlugin;
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::web::build_app;

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
    let body = get_ok(app, "/apps/new", &cookie).await;
    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        app,
        form_request("/apps/new", format!("csrf_token={csrf}&name=my-app"))
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
