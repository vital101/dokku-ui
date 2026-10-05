mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use actix_web::http::StatusCode;
use actix_web::test;

use common::{
    TestStatePair, complete_setup, extract_csrf, form_request, get_body, run_url,
    states_over_shared_db, test_state_pair,
};

use dokku_ui::dokku::{DokkuClient, DokkuError, DokkuOutput, MockClient};
use dokku_ui::domain::AppName;
use dokku_ui::domain::capabilities::CapabilityFamily;
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::web::build_app;

fn app_name(name: &str) -> AppName {
    AppName::try_from(name).expect("valid app name")
}

fn ps_report(running: bool, deployed: bool, processes: i64) -> DokkuOutput {
    DokkuOutput::ok(format!(
        r#"{{"deployed": "{deployed}", "running": "{running}", "processes": "{processes}"}}"#
    ))
}

fn apps_report() -> DokkuOutput {
    DokkuOutput::ok(r#"{"app-created-at": "1791023796", "app-locked": "false"}"#)
}

fn alpha_client() -> MockClient {
    MockClient::new()
        .stub(
            DokkuCommand::AppsList,
            Ok(DokkuOutput::ok("=====> My Apps\nalpha")),
        )
        .stub(
            DokkuCommand::AppsReport {
                app: app_name("alpha"),
            },
            Ok(apps_report()),
        )
        .stub(
            DokkuCommand::PsReport {
                app: app_name("alpha"),
            },
            Ok(ps_report(true, true, 2)),
        )
        .stub(
            DokkuCommand::PsRestart {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok("-----> restarting\n-----> done\n")),
        )
        .stub(
            DokkuCommand::PsStart {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok("-----> starting\n")),
        )
}

/// A client whose `ps:report` flips alpha to stopped the moment a restart is
/// issued — mimicking the real host changing state under a mutating action.
struct RestartFlipsReportClient {
    inner: MockClient,
    restarted: AtomicBool,
}

impl RestartFlipsReportClient {
    fn new() -> Self {
        Self {
            inner: alpha_client(),
            restarted: AtomicBool::new(false),
        }
    }
}

#[async_trait::async_trait]
impl DokkuClient for RestartFlipsReportClient {
    async fn exec(&self, command: &DokkuCommand) -> Result<DokkuOutput, DokkuError> {
        if matches!(command, DokkuCommand::PsRestart { .. }) {
            self.restarted.store(true, Ordering::SeqCst);
        }
        if let DokkuCommand::PsReport { app } = command {
            if app.as_str() == "alpha" && self.restarted.load(Ordering::SeqCst) {
                return Ok(ps_report(false, true, 0));
            }
        }
        self.inner.exec(command).await
    }
}

/// A client that errors on any call — any SSH attempt would fail the request.
fn no_ssh_client() -> MockClient {
    MockClient::with_default(Err(DokkuError::Connect("must not be called".into())))
}

fn hx_form_request(path: &str, body: String) -> actix_web::test::TestRequest {
    form_request(path, body).insert_header(("HX-Request", "true"))
}

async fn csrf_from<B, E>(
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
            .uri("/apps/alpha")
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
async fn run_streamed_from_a_second_process() {
    let TestStatePair {
        a,
        b,
        client_a,
        client_b: _client_b,
        _dir,
    } = test_state_pair(alpha_client(), MockClient::new()).await;
    let app_a = test::init_service(build_app(a)).await;
    let app_b = test::init_service(build_app(b)).await;
    let cookie = complete_setup(&app_b).await;

    let csrf = csrf_from(&app_a, &cookie).await;
    let resp = test::call_service(
        &app_a,
        hx_form_request("/apps/alpha/restart", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let run_url = run_url(&get_body(resp).await);

    let (status, events) = sse_events(&app_b, &run_url, &cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        events.contains("event: line\ndata: -----> restarting\n\n"),
        "{events}"
    );
    assert!(events.contains("event: done\n"), "{events}");
    assert!(events.contains(r#""ok":true"#), "{events}");
    assert!(
        client_a.calls().contains(&DokkuCommand::PsRestart {
            app: app_name("alpha")
        }),
        "the action ran on process A"
    );
}

#[tokio::test]
async fn run_ids_are_distinct_across_processes_and_streams_do_not_cross() {
    let TestStatePair {
        a,
        b,
        client_a: _client_a,
        client_b: _client_b,
        _dir,
    } = test_state_pair(alpha_client(), alpha_client()).await;
    let app_a = test::init_service(build_app(a)).await;
    let app_b = test::init_service(build_app(b)).await;
    let cookie = complete_setup(&app_b).await;

    let csrf_a = csrf_from(&app_a, &cookie).await;
    let csrf_b = csrf_from(&app_b, &cookie).await;

    let resp = test::call_service(
        &app_a,
        hx_form_request("/apps/alpha/restart", format!("csrf_token={csrf_a}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let url_a = run_url(&get_body(resp).await);

    let resp = test::call_service(
        &app_b,
        hx_form_request("/apps/alpha/start", format!("csrf_token={csrf_b}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    let url_b = run_url(&get_body(resp).await);

    assert_ne!(
        url_a, url_b,
        "random run ids never collide across processes"
    );

    let (_, events_a) = sse_events(&app_b, &url_a, &cookie).await;
    assert!(
        events_a.contains("restarting"),
        "B follows A's run: {events_a}"
    );
    assert!(
        !events_a.contains("event: line\ndata: -----> starting"),
        "A's stream has A's output only: {events_a}"
    );

    let (_, events_b) = sse_events(&app_a, &url_b, &cookie).await;
    assert!(
        events_b.contains("starting"),
        "A follows B's run: {events_b}"
    );
    assert!(
        !events_b.contains("event: line\ndata: -----> restarting"),
        "B's stream has B's output only: {events_b}"
    );
}

#[tokio::test]
async fn restart_on_one_process_is_visible_to_a_second_without_ssh() {
    let (a, b, _dir) = states_over_shared_db(
        Arc::new(RestartFlipsReportClient::new()),
        Arc::new(no_ssh_client()),
    )
    .await;
    let app_a = test::init_service(build_app(a)).await;
    let app_b = test::init_service(build_app(b)).await;
    let cookie = complete_setup(&app_b).await;

    let csrf = csrf_from(&app_a, &cookie).await;
    let resp = test::call_service(
        &app_a,
        hx_form_request("/apps/alpha/restart", format!("csrf_token={csrf}"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let (_, events) = sse_events(&app_a, &run_url(&get_body(resp).await), &cookie).await;
    assert!(events.contains(r#""ok":true"#), "{events}");

    let resp = test::call_service(
        &app_b,
        test::TestRequest::get()
            .uri("/")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "B never touched SSH");
    let body = get_body(resp).await;
    assert!(body.contains("alpha"), "{body}");
    assert!(
        body.contains("Stopped"),
        "B's dashboard reflects A's mutation from the shared row: {body}"
    );
}

#[tokio::test]
async fn cold_start_serves_the_shared_row_without_ssh() {
    let (a, b, _dir) =
        states_over_shared_db(Arc::new(alpha_client()), Arc::new(no_ssh_client())).await;
    let app_a = test::init_service(build_app(a)).await;
    let app_b = test::init_service(build_app(b)).await;
    let cookie = complete_setup(&app_b).await;

    let resp = test::call_service(
        &app_a,
        test::TestRequest::get()
            .uri("/")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "A warms the shared row");

    let resp = test::call_service(
        &app_b,
        test::TestRequest::get()
            .uri("/")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "B cold-starts from the row");
    let body = get_body(resp).await;
    assert!(body.contains("alpha"), "{body}");
    assert!(body.contains("Running"), "{body}");
}

const DOKKU_VERSION: &str = include_str!("fixtures/dokku_version.txt");
const LOGS_HELP: &str = include_str!("fixtures/logs_help.txt");

fn capabilities_client() -> MockClient {
    MockClient::new()
        .stub(
            DokkuCommand::DokkuVersion,
            Ok(DokkuOutput::ok(DOKKU_VERSION)),
        )
        .stub(
            DokkuCommand::PluginList,
            Ok(DokkuOutput::ok(
                "=====> Plugins\n  apps 0.38.4 enabled dokku core apps plugin\n  nginx-vhosts 0.38.4 enabled dokku core nginx-vhosts plugin\n",
            )),
        )
        .stub(
            DokkuCommand::Help {
                family: CapabilityFamily::Logs,
            },
            Ok(DokkuOutput::ok(LOGS_HELP)),
        )
        .stub(
            DokkuCommand::Help {
                family: CapabilityFamily::NginxAccessLogs,
            },
            Err(DokkuError::Exit {
                code: 1,
                stderr: "unknown command".into(),
            }),
        )
        .stub(
            DokkuCommand::Help {
                family: CapabilityFamily::NginxErrorLogs,
            },
            Err(DokkuError::Exit {
                code: 1,
                stderr: "unknown command".into(),
            }),
        )
}

#[tokio::test]
async fn capabilities_cold_start_serves_the_shared_row_without_ssh() {
    let (a, b, _dir) =
        states_over_shared_db(Arc::new(capabilities_client()), Arc::new(no_ssh_client())).await;

    let caps_a = a
        .capabilities
        .ensure_loaded()
        .await
        .expect("A probes the host");
    assert!(caps_a.enabled_plugins.contains(&"nginx-vhosts".into()));

    let caps_b = b
        .capabilities
        .ensure_loaded()
        .await
        .expect("B serves the shared row");
    assert_eq!(
        caps_b.dokku_version.map(|version| version.to_string()),
        Some("0.38.4".to_owned())
    );
    assert_eq!(
        caps_a.log_sources, caps_b.log_sources,
        "both processes answer identically"
    );
}
