mod common;

use std::sync::Arc;

use actix_web::http::StatusCode;
use actix_web::test;

use common::{
    complete_setup, extract_csrf, form_request, get_body, location, response_cookie, run_url,
    test_state_with_shared_client,
};

use dokku_ui::dokku::{DokkuError, DokkuOutput, MockClient};
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::domain::{ServiceCreateOptions, ServiceName, ServicePlugin};
use dokku_ui::web::build_app;

const REDIS_LIST: &str = include_str!("fixtures/redis_list.txt");
const REDIS_INFO: &str = include_str!("fixtures/redis_info.txt");
const SERVICE_LIST_EMPTY: &str = include_str!("fixtures/service_list_empty.txt");

fn redis() -> ServicePlugin {
    ServicePlugin::try_from("redis").expect("plugin")
}

fn candid() -> ServiceName {
    ServiceName::try_from("candid").expect("service name")
}

/// Polls the mock client until the executor task has run `command` (jobs
/// execute asynchronously now), failing after a short budget.
async fn wait_for_call(client: &Arc<MockClient>, command: &DokkuCommand) {
    for _ in 0..200 {
        if client.calls().contains(command) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("command never called: {command:?}");
}

fn service_list_stub() -> MockClient {
    MockClient::new().stub(
        DokkuCommand::ServiceList { plugin: redis() },
        Ok(DokkuOutput::ok(REDIS_LIST)),
    )
}

fn exit_error(code: i32, stderr: &str) -> DokkuError {
    DokkuError::Exit {
        code,
        stderr: stderr.to_owned(),
    }
}

fn hx_form_request(path: &str, body: String) -> actix_web::test::TestRequest {
    form_request(path, body).insert_header(("HX-Request", "true"))
}

async fn service_shell_csrf<B, E>(
    app: &impl actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse<B>,
        Error = E,
    >,
    cookie: &actix_web::cookie::Cookie<'static>,
) -> String
where
    B: actix_web::body::MessageBody,
    E: std::fmt::Debug,
{
    let resp = test::call_service(
        app,
        test::TestRequest::get()
            .uri("/services/redis/candid")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    extract_csrf(&get_body(resp).await)
}

async fn sse_events<B, E>(
    app: &impl actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse<B>,
        Error = E,
    >,
    uri: &str,
    cookie: &actix_web::cookie::Cookie<'static>,
) -> (StatusCode, String)
where
    B: actix_web::body::MessageBody,
    E: std::fmt::Debug,
{
    let resp = test::call_service(
        app,
        test::TestRequest::get()
            .uri(uri)
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let status = resp.status();
    (status, get_body(resp).await)
}

#[tokio::test]
async fn service_list_shell_renders_panel_and_new_button() {
    let (state, _client, _dir) = test_state_with_shared_client(service_list_stub()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("<!doctype html>"), "full page");
    assert!(body.contains("Redis services"));
    assert!(body.contains(r#"hx-get="/services/redis/partials/list""#));
    assert!(body.contains(r#"href="/services/redis/new""#));
    assert!(
        body.contains(r#"hx-get="/partials/service-nav""#),
        "sidebar nav is capability-loaded"
    );
    assert!(body.contains(r#"href="/volumes""#), "sidebar nav");
}

#[tokio::test]
async fn service_list_partial_renders_rows_without_the_dsn() {
    let client = service_list_stub().stub(
        DokkuCommand::ServiceInfo {
            plugin: "redis".into(),
            service: "candid".into(),
        },
        Ok(DokkuOutput::ok(REDIS_INFO)),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis/partials/list")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(
        !body.contains("<!doctype html>"),
        "fragment, not a full page"
    );
    assert!(body.contains(r#"href="/services/redis/candid""#));
    assert!(body.contains("running"));
    assert!(body.contains("redis:7.2.4"));
    assert!(!body.contains("redis://"), "dsn never rendered: {body}");
}

#[tokio::test]
async fn service_list_partial_degrades_info_failure_to_unknown() {
    let client = service_list_stub().stub(
        DokkuCommand::ServiceInfo {
            plugin: "redis".into(),
            service: "candid".into(),
        },
        Err(exit_error(1, "boom")),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis/partials/list")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;

    assert!(body.contains("candid"));
    assert!(body.contains("unknown"), "{body}");
}

#[tokio::test]
async fn service_list_partial_renders_empty_state() {
    let client = MockClient::new().stub(
        DokkuCommand::ServiceList { plugin: redis() },
        Ok(DokkuOutput::ok(SERVICE_LIST_EMPTY)),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis/partials/list")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;

    assert!(body.contains("No Redis services yet."));
    assert!(body.contains(r#"href="/services/redis/new""#));
}

#[tokio::test]
async fn service_list_partial_returns_retry_card_when_list_fails() {
    let client = MockClient::new().stub(
        DokkuCommand::ServiceList { plugin: redis() },
        Err(exit_error(1, "plugin not installed")),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis/partials/list")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "200 so htmx swaps it in");
    let body = get_body(resp).await;

    assert!(body.contains("Could not load this section."));
    assert!(body.contains(r#"hx-get="/services/redis/partials/list""#));
    assert!(body.contains("plugin not installed"));
}

#[tokio::test]
async fn service_shell_renders_tabs_and_actions() {
    let (state, _client, _dir) = test_state_with_shared_client(service_list_stub()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis/candid")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;

    assert!(body.contains(r#"href="/services/redis/candid/links""#));
    assert!(body.contains(r#"href="/services/redis/candid/logs""#));
    assert!(body.contains(r#"hx-get="/services/redis/candid/partials/overview""#));
    assert!(body.contains(r#"hx-post="/services/redis/candid/start""#));
    assert!(body.contains(r#"hx-post="/services/redis/candid/stop""#));
    assert!(body.contains(r#"hx-post="/services/redis/candid/restart""#));
    assert!(body.contains(r#"hx-get="/services/redis/candid/partials/delete-confirm""#));
}

#[tokio::test]
async fn service_shell_404s_unknown_service_and_plugin() {
    let (state, _client, _dir) = test_state_with_shared_client(service_list_stub()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    for path in ["/services/redis/ghost", "/services/wordpress/candid"] {
        let resp = test::call_service(
            &app,
            test::TestRequest::get()
                .uri(path)
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn service_overview_partial_renders_details_and_expose_form() {
    let client = MockClient::new().stub(
        DokkuCommand::ServiceInfo {
            plugin: "redis".into(),
            service: "candid".into(),
        },
        Ok(DokkuOutput::ok(REDIS_INFO)),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis/candid/partials/overview")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;

    assert!(body.contains("running"));
    assert!(body.contains("redis:7.2.4"));
    assert!(body.contains("candid"), "linked apps");
    assert!(body.contains(r#"hx-post="/services/redis/candid/expose""#));
    assert!(body.contains("Expose"));
    assert!(
        body.contains(r#"hx-get="/services/redis/candid/partials/stats""#),
        "stats card loads lazily: {body}"
    );
    assert!(!body.contains("redis://"), "dsn never rendered: {body}");
}

#[tokio::test]
async fn service_overview_partial_unknown_service_returns_retry_card() {
    let client = MockClient::new().stub(
        DokkuCommand::ServiceInfo {
            plugin: "redis".into(),
            service: "candid".into(),
        },
        Ok(DokkuOutput::ok("nonsense")),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis/candid/partials/overview")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("was not found — it may have been destroyed."));
    assert!(body.contains(r#"hx-get="/services/redis/candid/partials/overview""#));
}

#[tokio::test]
async fn service_links_partial_lists_links_and_available_apps() {
    let client = MockClient::new()
        .stub(
            DokkuCommand::ServiceLinks {
                plugin: redis(),
                service: candid(),
            },
            Ok(DokkuOutput::ok("alpha\n")),
        )
        .stub(
            DokkuCommand::AppsList,
            Ok(DokkuOutput::ok("=====> My Apps\nalpha\nbeta")),
        );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis/candid/partials/links")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;

    assert!(
        !body.contains("<!doctype html>"),
        "fragment, not a full page"
    );
    assert!(body.contains(r#"href="/apps/alpha""#));
    assert!(body.contains(r#"hx-post="/services/redis/candid/unlink""#));
    assert!(body.contains(r#"<option value="beta">beta</option>"#));
    assert!(
        !body.contains(r#"<option value="alpha">alpha</option>"#),
        "linked app is not offered again: {body}"
    );
}

#[tokio::test]
async fn service_links_partial_renders_empty_state() {
    let client = MockClient::new().stub(
        DokkuCommand::ServiceLinks {
            plugin: redis(),
            service: candid(),
        },
        Ok(DokkuOutput::ok("")),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis/candid/partials/links")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;

    assert!(body.contains("No apps are linked to this service."));
    assert!(body.contains("No unlinked apps available."));
}

#[tokio::test]
async fn service_logs_partial_renders_lines() {
    let client = MockClient::new().stub(
        DokkuCommand::ServiceLogs {
            plugin: redis(),
            service: candid(),
            num_lines: 200,
        },
        Ok(DokkuOutput::ok(include_str!("fixtures/redis_logs.txt"))),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis/candid/partials/logs?lines=200")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;

    assert!(body.contains("Ready to accept connections"));
    assert!(
        !body.contains("<!doctype html>"),
        "fragment, not a full page"
    );
}

#[tokio::test]
async fn hx_service_start_returns_run_fragment_and_streams_output() {
    let client = service_list_stub().stub(
        DokkuCommand::ServiceStart {
            plugin: redis(),
            service: candid(),
        },
        Ok(DokkuOutput::ok("-----> starting\n")),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = service_shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request("/services/redis/candid/start", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains(r#"data-run-url="/actions/runs/"#), "{body}");
    assert!(body.contains(r#"data-refresh="/services/redis/candid/partials/overview""#));
    assert!(body.contains("Starting candid"));
    assert!(body.contains("data-run-log"));

    let (status, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        events.contains("event: line\ndata: -----> starting"),
        "{events}"
    );
    assert!(events.contains("event: done"), "{events}");
    assert!(events.contains(r#""ok":true"#), "{events}");
}

#[tokio::test]
async fn service_actions_are_queued_flash_and_redirect_to_detail() {
    for (path, command, verb) in [
        (
            "/services/redis/candid/start",
            DokkuCommand::ServiceStart {
                plugin: redis(),
                service: candid(),
            },
            "start",
        ),
        (
            "/services/redis/candid/stop",
            DokkuCommand::ServiceStop {
                plugin: redis(),
                service: candid(),
            },
            "stop",
        ),
        (
            "/services/redis/candid/restart",
            DokkuCommand::ServiceRestart {
                plugin: redis(),
                service: candid(),
            },
            "restart",
        ),
    ] {
        let client = service_list_stub().stub(command.clone(), Ok(DokkuOutput::ok("")));
        let (state, client, _dir) = test_state_with_shared_client(client).await;
        let app = test::init_service(build_app(state)).await;
        let cookie = complete_setup(&app).await;
        let csrf = service_shell_csrf(&app, &cookie).await;

        let resp = test::call_service(
            &app,
            form_request(path, format!("csrf_token={csrf}"))
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER, "{path}");
        assert_eq!(location(&resp), "/services/redis/candid", "{path}");
        let cookie = response_cookie(&resp).unwrap_or(cookie);

        wait_for_call(&client, &command).await;

        let resp = test::call_service(
            &app,
            test::TestRequest::get()
                .uri("/services/redis/candid")
                .cookie(cookie)
                .to_request(),
        )
        .await;
        let body = get_body(resp).await;
        assert!(
            body.contains(&format!("Queued: {verb} candid.")),
            "{path} queued flash: {body}"
        );
    }
}

#[tokio::test]
async fn service_action_error_is_queued_and_fails_in_the_audit_trail() {
    let client = service_list_stub().stub(
        DokkuCommand::ServiceStart {
            plugin: redis(),
            service: candid(),
        },
        Err(exit_error(1, "service is missing")),
    );
    let (state, client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = service_shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        form_request("/services/redis/candid/start", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    wait_for_call(
        &client,
        &DokkuCommand::ServiceStart {
            plugin: redis(),
            service: candid(),
        },
    )
    .await;

    // The executor finishes the run asynchronously after the command call;
    // poll until the outcome lands rather than racing a single read.
    let mut body = String::new();
    for _ in 0..200 {
        let resp = test::call_service(
            &app,
            test::TestRequest::get()
                .uri("/services/redis/candid/activity")
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        body = get_body(resp).await;
        if body.contains("failed") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    assert!(body.contains("service.start"), "{body}");
    assert!(
        body.contains("failed"),
        "the failed run is in the audit trail: {body}"
    );
}

#[tokio::test]
async fn hx_service_destroy_requires_typed_name() {
    let (state, client, _dir) = test_state_with_shared_client(service_list_stub()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = service_shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/services/redis/candid/destroy",
            format!("csrf_token={csrf}&name=wrong"),
        )
        .cookie(cookie)
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "200 so htmx swaps it in");
    let body = get_body(resp).await;

    assert!(body.contains("data-modal-error"), "{body}");
    assert!(
        body.contains("Type `candid` to confirm destruction."),
        "{body}"
    );
    assert!(
        client
            .calls()
            .iter()
            .all(|call| !matches!(call, DokkuCommand::ServiceDestroy { .. })),
        "no destroy command for mismatched confirmation"
    );
}

#[tokio::test]
async fn hx_service_destroy_streams_and_redirects_to_list() {
    let client = service_list_stub().stub(
        DokkuCommand::ServiceDestroy {
            plugin: redis(),
            service: candid(),
            force: true,
        },
        Ok(DokkuOutput::ok("-----> destroying\n")),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = service_shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/services/redis/candid/destroy",
            format!("csrf_token={csrf}&name=candid"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Destroying candid"));
    assert!(!body.contains("data-refresh"), "destroy redirects instead");

    let (_, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(
        events.contains(r#""redirect":"/services/redis""#),
        "{events}"
    );
}

#[tokio::test]
async fn hx_service_expose_validates_ports_without_calling_dokku() {
    let (state, client, _dir) = test_state_with_shared_client(service_list_stub()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = service_shell_csrf(&app, &cookie).await;

    for ports in ["", "not a port", "5432;rm"] {
        let resp = test::call_service(
            &app,
            hx_form_request(
                "/services/redis/candid/expose",
                format!("csrf_token={csrf}&ports={}", ports.replace(' ', "+")),
            )
            .cookie(cookie.clone())
            .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK, "{ports}");
        let body = get_body(resp).await;
        assert!(body.contains("data-modal-error"), "{ports}: {body}");
    }
    assert!(
        client
            .calls()
            .iter()
            .all(|call| !matches!(call, DokkuCommand::ServiceExpose { .. })),
        "no expose command for invalid ports"
    );
}

#[tokio::test]
async fn hx_service_expose_and_unexpose_stream() {
    let client = service_list_stub()
        .stub(
            DokkuCommand::ServiceExpose {
                plugin: redis(),
                service: candid(),
                ports: "6379".into(),
            },
            Ok(DokkuOutput::ok("-----> exposed\n")),
        )
        .stub(
            DokkuCommand::ServiceUnexpose {
                plugin: redis(),
                service: candid(),
            },
            Ok(DokkuOutput::ok("-----> unexposed\n")),
        );
    let (state, client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = service_shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/services/redis/candid/expose",
            format!("csrf_token={csrf}&ports=6379"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Exposing candid"));
    let (_, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(events.contains(r#""ok":true"#), "{events}");

    let csrf = service_shell_csrf(&app, &cookie).await;
    let resp = test::call_service(
        &app,
        hx_form_request(
            "/services/redis/candid/unexpose",
            format!("csrf_token={csrf}"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Unexposing candid"));
    let (_, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(events.contains(r#""ok":true"#), "{events}");

    assert!(client.calls().contains(&DokkuCommand::ServiceExpose {
        plugin: redis(),
        service: candid(),
        ports: "6379".into(),
    }));
    assert!(client.calls().contains(&DokkuCommand::ServiceUnexpose {
        plugin: redis(),
        service: candid(),
    }));
}

#[tokio::test]
async fn hx_service_link_validates_app_and_streams() {
    let client = service_list_stub()
        .stub(
            DokkuCommand::ServiceLink {
                plugin: redis(),
                service: candid(),
                app: dokku_ui::domain::AppName::try_from("alpha").expect("app"),
            },
            Ok(DokkuOutput::ok("-----> linked\n")),
        )
        .stub(
            DokkuCommand::ServiceUnlink {
                plugin: redis(),
                service: candid(),
                app: dokku_ui::domain::AppName::try_from("alpha").expect("app"),
            },
            Ok(DokkuOutput::ok("-----> unlinked\n")),
        );
    let (state, client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = service_shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/services/redis/candid/link",
            format!("csrf_token={csrf}&app=Bad_App"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("data-modal-error"), "{body}");

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/services/redis/candid/link",
            format!("csrf_token={csrf}&app=alpha"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Linking candid to alpha"));
    let (_, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(events.contains(r#""ok":true"#), "{events}");

    let csrf = service_shell_csrf(&app, &cookie).await;
    let resp = test::call_service(
        &app,
        hx_form_request(
            "/services/redis/candid/unlink",
            format!("csrf_token={csrf}&app=alpha"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Unlinking candid from alpha"));
    let (_, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(events.contains(r#""ok":true"#), "{events}");

    assert!(client.calls().contains(&DokkuCommand::ServiceLink {
        plugin: redis(),
        service: candid(),
        app: dokku_ui::domain::AppName::try_from("alpha").expect("app"),
    }));
    assert!(client.calls().contains(&DokkuCommand::ServiceUnlink {
        plugin: redis(),
        service: candid(),
        app: dokku_ui::domain::AppName::try_from("alpha").expect("app"),
    }));
}

#[tokio::test]
async fn service_new_form_renders_create_form() {
    let (state, _client, _dir) = test_state_with_shared_client(service_list_stub()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis/new")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains(r#"action="/services/redis""#));
    assert!(body.contains(r#"hx-post="/services/redis""#));
    assert!(body.contains(r#"name="name""#));
    assert!(body.contains(r#"name="image""#));
    assert!(body.contains(r#"name="custom_env""#));
    assert!(body.contains(r#"name="config_options""#));
}

#[tokio::test]
async fn service_nav_lists_only_installed_plugins() {
    let client = service_list_stub()
        .stub(
            DokkuCommand::DokkuVersion,
            Ok(DokkuOutput::ok("dokku version 0.38.4\n")),
        )
        .stub(
            DokkuCommand::PluginList,
            Ok(DokkuOutput::ok(
                "postgres 1.36.4 enabled dokku postgres service plugin\nredis 1.42.1 enabled dokku redis service plugin\n",
            )),
        );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/partials/service-nav")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("/services/postgres"), "{body}");
    assert!(body.contains("/services/redis"), "{body}");
    assert!(!body.contains("/services/mysql"), "{body}");
    assert!(!body.contains("/services/clickhouse"), "{body}");
}

#[tokio::test]
async fn hx_create_service_streams_and_redirects_to_detail() {
    let client = service_list_stub().stub(
        DokkuCommand::ServiceCreate {
            plugin: redis(),
            service: ServiceName::try_from("cache").expect("service"),
            options: ServiceCreateOptions::default(),
        },
        Ok(DokkuOutput::ok("-----> pulling image\n-----> created\n")),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = common::extract_csrf(
        &get_body(
            test::call_service(
                &app,
                test::TestRequest::get()
                    .uri("/services/redis/new")
                    .cookie(cookie.clone())
                    .to_request(),
            )
            .await,
        )
        .await,
    );

    let resp = test::call_service(
        &app,
        hx_form_request("/services/redis", format!("csrf_token={csrf}&name=cache"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("Creating cache"));
    assert!(!body.contains("data-refresh"), "create redirects instead");

    let (_, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(
        events.contains("event: line\ndata: -----> pulling image"),
        "{events}"
    );
    assert!(
        events.contains(r#""redirect":"/services/redis/cache""#),
        "{events}"
    );
}

#[tokio::test]
async fn create_service_non_htmx_flashes_and_redirects() {
    let client = service_list_stub().stub(
        DokkuCommand::ServiceCreate {
            plugin: redis(),
            service: ServiceName::try_from("cache").expect("service"),
            options: ServiceCreateOptions::default(),
        },
        Ok(DokkuOutput::ok("")),
    );
    let (state, client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = common::extract_csrf(
        &get_body(
            test::call_service(
                &app,
                test::TestRequest::get()
                    .uri("/services/redis/new")
                    .cookie(cookie.clone())
                    .to_request(),
            )
            .await,
        )
        .await,
    );

    let resp = test::call_service(
        &app,
        form_request("/services/redis", format!("csrf_token={csrf}&name=cache"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/services/redis/cache");
    let cookie = response_cookie(&resp).unwrap_or(cookie);
    assert!(client.calls().contains(&DokkuCommand::ServiceCreate {
        plugin: redis(),
        service: ServiceName::try_from("cache").expect("service"),
        options: ServiceCreateOptions::default(),
    }));

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Service &#39;cache&#39; created."), "{body}");
}

#[tokio::test]
async fn create_service_rejects_invalid_name_without_calling_dokku() {
    let (state, client, _dir) = test_state_with_shared_client(service_list_stub()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = common::extract_csrf(
        &get_body(
            test::call_service(
                &app,
                test::TestRequest::get()
                    .uri("/services/redis/new")
                    .cookie(cookie.clone())
                    .to_request(),
            )
            .await,
        )
        .await,
    );

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/services/redis",
            format!("csrf_token={csrf}&name=Bad.Name"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("data-modal-error"), "{body}");

    let resp = test::call_service(
        &app,
        form_request(
            "/services/redis",
            format!("csrf_token={csrf}&name=Bad.Name"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/services/redis/new");
    assert!(
        client
            .calls()
            .iter()
            .all(|call| !matches!(call, DokkuCommand::ServiceCreate { .. })),
        "no create command for invalid name"
    );
}

#[tokio::test]
async fn hx_create_service_passes_advanced_options_and_redacts_env_values() {
    let options = ServiceCreateOptions::parse(
        Some("redis"),
        Some("7.2"),
        Some("USER=alpha-secret"),
        Some("--appendonly yes"),
    )
    .expect("valid options");
    let client = service_list_stub().stub(
        DokkuCommand::ServiceCreate {
            plugin: redis(),
            service: ServiceName::try_from("cache").expect("service"),
            options: options.clone(),
        },
        Ok(DokkuOutput::ok("creating with USER=alpha-secret\n")),
    );
    let (state, client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = common::extract_csrf(
        &get_body(
            test::call_service(
                &app,
                test::TestRequest::get()
                    .uri("/services/redis/new")
                    .cookie(cookie.clone())
                    .to_request(),
            )
            .await,
        )
        .await,
    );

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/services/redis",
            format!(
                "csrf_token={csrf}&name=cache&image=redis&image_version=7.2&custom_env=USER%3Dalpha-secret&config_options=--appendonly+yes"
            ),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    let (_, events) = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(events.contains("••••••••"), "value is masked: {events}");
    assert!(!events.contains("alpha-secret"), "secret leaked: {events}");
    assert!(
        client.calls().contains(&DokkuCommand::ServiceCreate {
            plugin: redis(),
            service: ServiceName::try_from("cache").expect("service"),
            options,
        }),
        "advanced options reach dokku"
    );
}

#[tokio::test]
async fn create_service_rejects_invalid_advanced_options_without_calling_dokku() {
    let (state, client, _dir) = test_state_with_shared_client(service_list_stub()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = common::extract_csrf(
        &get_body(
            test::call_service(
                &app,
                test::TestRequest::get()
                    .uri("/services/redis/new")
                    .cookie(cookie.clone())
                    .to_request(),
            )
            .await,
        )
        .await,
    );

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/services/redis",
            format!("csrf_token={csrf}&name=cache&custom_env=USER%3Dhas%27quote"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("data-modal-error"), "{body}");

    let resp = test::call_service(
        &app,
        form_request(
            "/services/redis",
            format!("csrf_token={csrf}&name=cache&image=-flag"),
        )
        .cookie(cookie)
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/services/redis/new");
    assert!(
        client
            .calls()
            .iter()
            .all(|call| !matches!(call, DokkuCommand::ServiceCreate { .. })),
        "no create command for invalid options"
    );
}

#[tokio::test]
async fn service_links_and_logs_shells_render_tab_panels() {
    let (state, _client, _dir) = test_state_with_shared_client(service_list_stub()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis/candid/links")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains(r#"hx-get="/services/redis/candid/partials/links""#));
    assert!(
        body.contains("border-emerald-500"),
        "links tab active: {body}"
    );

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis/candid/logs?lines=50")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains(r#"hx-get="/services/redis/candid/partials/logs?lines=50""#));
    assert!(body.contains(r#"value="50""#));
}

#[tokio::test]
async fn service_partials_return_retry_cards_on_dokku_errors() {
    let client = MockClient::new()
        .stub(
            DokkuCommand::ServiceInfo {
                plugin: "redis".into(),
                service: "candid".into(),
            },
            Err(exit_error(1, "info boom")),
        )
        .stub(
            DokkuCommand::ServiceLinks {
                plugin: redis(),
                service: candid(),
            },
            Err(exit_error(1, "links boom")),
        )
        .stub(
            DokkuCommand::ServiceLogs {
                plugin: redis(),
                service: candid(),
                num_lines: 200,
            },
            Err(exit_error(1, "logs boom")),
        );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    for (path, message) in [
        ("/services/redis/candid/partials/overview", "info boom"),
        ("/services/redis/candid/partials/links", "links boom"),
        ("/services/redis/candid/partials/logs", "logs boom"),
    ] {
        let resp = test::call_service(
            &app,
            test::TestRequest::get()
                .uri(path)
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "{path} 200 so htmx swaps it in"
        );
        let body = get_body(resp).await;
        assert!(body.contains("Could not load this section."), "{path}");
        assert!(body.contains(message), "{path}: {body}");
    }
}

#[tokio::test]
async fn delete_confirm_modal_renders_typed_name_form() {
    let (state, _client, _dir) = test_state_with_shared_client(service_list_stub()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis/candid/partials/delete-confirm")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains(r#"hx-post="/services/redis/candid/destroy""#));
    assert!(body.contains(r#"name="name""#));
    assert!(body.contains("container and its data will be deleted"));
}

#[tokio::test]
async fn delete_confirm_page_renders_typed_name_form() {
    let (state, _client, _dir) = test_state_with_shared_client(service_list_stub()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis/candid/delete")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(
        body.contains("<!doctype html>"),
        "full page, no-JS fallback"
    );
    assert!(body.contains(r#"action="/services/redis/candid/destroy""#));
    assert!(body.contains(r#"name="name""#));
    assert!(body.contains("container and its data will be deleted"));
    assert!(
        body.contains(r#"href="/services/redis/candid""#),
        "cancel link back to detail"
    );
}

#[tokio::test]
async fn destroy_service_non_htmx_requires_typed_name() {
    let (state, client, _dir) = test_state_with_shared_client(service_list_stub()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = service_shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        form_request(
            "/services/redis/candid/destroy",
            format!("csrf_token={csrf}&name=wrong"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/services/redis/candid/delete");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    assert!(
        client
            .calls()
            .iter()
            .all(|call| !matches!(call, DokkuCommand::ServiceDestroy { .. })),
        "no destroy command for mismatched confirmation"
    );

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis/candid/delete")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(
        body.contains("Type `candid` to confirm destruction."),
        "{body}"
    );
}

#[tokio::test]
async fn destroy_service_non_htmx_queues_and_redirects_to_list() {
    let client = service_list_stub().stub(
        DokkuCommand::ServiceDestroy {
            plugin: redis(),
            service: candid(),
            force: true,
        },
        Ok(DokkuOutput::ok("")),
    );
    let (state, client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = service_shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        form_request(
            "/services/redis/candid/destroy",
            format!("csrf_token={csrf}&name=candid"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/services/redis");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    wait_for_call(
        &client,
        &DokkuCommand::ServiceDestroy {
            plugin: redis(),
            service: candid(),
            force: true,
        },
    )
    .await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Queued: destroy candid."), "{body}");
}

#[tokio::test]
async fn destroy_is_blocked_while_the_service_has_linked_apps() {
    let client = service_list_stub().stub(
        DokkuCommand::ServiceLinks {
            plugin: redis(),
            service: candid(),
        },
        Ok(DokkuOutput::ok("web\nworker\n")),
    );
    let (state, client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = service_shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/services/redis/candid/destroy",
            format!("csrf_token={csrf}&name=candid"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("data-modal-error"), "{body}");
    assert!(body.contains("web, worker"), "{body}");

    let resp = test::call_service(
        &app,
        form_request(
            "/services/redis/candid/destroy",
            format!("csrf_token={csrf}&name=candid"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/services/redis/candid");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    assert!(
        client
            .calls()
            .iter()
            .all(|call| !matches!(call, DokkuCommand::ServiceDestroy { .. })),
        "a linked service is never destroyed"
    );

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis/candid")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Unlink these apps first"), "{body}");
}

#[tokio::test]
async fn destroy_fails_closed_when_the_link_check_fails() {
    let client = service_list_stub().stub(
        DokkuCommand::ServiceLinks {
            plugin: redis(),
            service: candid(),
        },
        Err(exit_error(1, "links boom")),
    );
    let (state, client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = service_shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/services/redis/candid/destroy",
            format!("csrf_token={csrf}&name=candid"),
        )
        .cookie(cookie)
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("Could not verify service links"), "{body}");
    assert!(
        client
            .calls()
            .iter()
            .all(|call| !matches!(call, DokkuCommand::ServiceDestroy { .. })),
        "destroy must not run when links cannot be verified"
    );
}

#[tokio::test]
async fn service_stats_partial_renders_resource_labels() {
    let client = MockClient::new().stub(
        DokkuCommand::ServiceStats {
            plugin: redis(),
            service: candid(),
        },
        Ok(DokkuOutput::ok(include_str!("fixtures/redis_stats.txt"))),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis/candid/partials/stats")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(
        !body.contains("<!doctype html>"),
        "fragment, not a full page"
    );
    assert!(body.contains("Resources"), "{body}");
    assert!(body.contains("6.7 MiB"), "{body}");
    assert!(
        body.contains("of 7.8 GiB host (3.7 GiB available)"),
        "{body}"
    );
    assert!(body.contains("0.1%"), "{body}");
    assert!(body.contains("6h 39m total"), "{body}");
    assert!(body.contains("12 KiB"), "{body}");
    assert!(body.contains("126.0 GiB used of 154.9 GiB"), "{body}");
    assert!(body.contains("(28.8 GiB free)"), "{body}");
    assert!(body.contains("<progress"), "{body}");
}

#[tokio::test]
async fn service_stats_partial_not_running_is_a_state() {
    let client = MockClient::new().stub(
        DokkuCommand::ServiceStats {
            plugin: redis(),
            service: candid(),
        },
        Err(exit_error(1, "Service container is not running")),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis/candid/partials/stats")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("Service is not running"), "{body}");
    assert!(!body.contains("Could not load"), "{body}");
}

#[tokio::test]
async fn service_stats_partial_failure_returns_retry_card() {
    let client = MockClient::new().stub(
        DokkuCommand::ServiceStats {
            plugin: redis(),
            service: candid(),
        },
        Err(DokkuError::Connect("refused".into())),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis/candid/partials/stats")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("Could not load this section."), "{body}");
    assert!(
        body.contains(r#"hx-get="/services/redis/candid/partials/stats""#),
        "{body}"
    );
}

#[tokio::test]
async fn service_stats_partial_unrecognised_output_returns_retry_card() {
    let client = MockClient::new().stub(
        DokkuCommand::ServiceStats {
            plugin: redis(),
            service: candid(),
        },
        Ok(DokkuOutput::ok("-----> nothing useful here\n")),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/services/redis/candid/partials/stats")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("No stats were reported"), "{body}");
}
