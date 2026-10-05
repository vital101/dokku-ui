mod common;

use actix_web::http::StatusCode;
use actix_web::test;

use common::{
    complete_setup, form_request, get_body, location, response_cookie, run_url,
    test_state_with_shared_client,
};

use dokku_ui::dokku::{DokkuError, DokkuOutput, MockClient};
use dokku_ui::domain::AppName;
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::domain::mount_spec::MountSpec;
use dokku_ui::web::build_app;

const STORAGE_REPORT: &str = include_str!("fixtures/storage_report.txt");

fn app_name(name: &str) -> AppName {
    AppName::try_from(name).expect("valid app name")
}

fn mount(spec: &str) -> MountSpec {
    MountSpec::try_from(spec).expect("valid mount spec")
}

fn mounts_client() -> MockClient {
    MockClient::new()
        .stub(
            DokkuCommand::StorageReport,
            Ok(DokkuOutput::ok(STORAGE_REPORT)),
        )
        .stub(
            DokkuCommand::AppsList,
            Ok(DokkuOutput::ok("=====> My Apps\nalpha\ndokku-ui\nstarwars")),
        )
}

fn hx_form_request(path: &str, body: String) -> actix_web::test::TestRequest {
    form_request(path, body).insert_header(("HX-Request", "true"))
}

async fn shell_csrf<B, E>(
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
            .uri("/volumes")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    common::extract_csrf(&get_body(resp).await)
}

async fn sse_events<B, E>(
    app: &impl actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse<B>,
        Error = E,
    >,
    uri: &str,
    cookie: &actix_web::cookie::Cookie<'static>,
) -> String
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
    assert_eq!(resp.status(), StatusCode::OK);
    get_body(resp).await
}

#[tokio::test]
async fn volumes_shell_renders_panel_and_mount_form() {
    let (state, _client, _dir) = test_state_with_shared_client(mounts_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/volumes")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("<!doctype html>"), "full page");
    assert!(body.contains(r#"hx-get="/volumes/partials/list""#));
    assert!(body.contains(r#"href="/volumes""#), "sidebar nav");
}

#[tokio::test]
async fn volumes_partial_lists_mounts_per_app() {
    let (state, _client, _dir) = test_state_with_shared_client(mounts_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/volumes/partials/list")
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
    assert!(body.contains(r#"href="/apps/dokku-ui""#), "{body}");
    assert!(body.contains("/var/lib/dokku/data/services/dokku-ui"));
    assert!(body.contains("/app/data"));
    assert!(body.contains("deploy, run"));
    assert!(body.contains("4 apps with mounts"));
    assert!(body.contains("4 mounts"));
    assert!(body.contains(r#"hx-post="/volumes/unmount""#));
    assert!(
        body.contains(r#"value="/var/lib/dokku/data/services/dokku-ui:/app/data""#),
        "unmount carries the locator: {body}"
    );
    assert!(body.contains(r#"href="/apps/candid""#), "{body}");
    assert!(
        !body.contains(r#"href="/apps/starwars""#),
        "apps without mounts are not listed: {body}"
    );
    assert!(body.contains(r#"<option value="alpha">alpha</option>"#));
}

#[tokio::test]
async fn volumes_partial_returns_retry_card_when_report_fails() {
    let client = MockClient::new().stub(
        DokkuCommand::StorageReport,
        Err(DokkuError::Exit {
            code: 1,
            stderr: "boom".into(),
        }),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/volumes/partials/list")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "200 so htmx swaps it in");
    let body = get_body(resp).await;

    assert!(body.contains("Could not load this section."));
    assert!(body.contains(r#"hx-get="/volumes/partials/list""#));
    assert!(body.contains("boom"));
}

#[tokio::test]
async fn hx_volume_mount_validates_spec_without_calling_dokku() {
    let (state, client, _dir) = test_state_with_shared_client(mounts_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/volumes/mount",
            format!("csrf_token={csrf}&app=alpha&host=/host&container=relative&options="),
        )
        .cookie(cookie)
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("data-modal-error"), "{body}");
    assert!(body.contains("must be absolute"), "{body}");
    assert!(
        client
            .calls()
            .iter()
            .all(|call| !matches!(call, DokkuCommand::StorageMount { .. })),
        "no mount command for invalid spec"
    );
}

#[tokio::test]
async fn hx_volume_mount_streams_and_refreshes_list() {
    let client = mounts_client().stub(
        DokkuCommand::StorageMount {
            app: app_name("alpha"),
            mount: mount("/host/data:/srv/data:ro"),
        },
        Ok(DokkuOutput::ok("-----> mounted\n")),
    );
    let (state, client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/volumes/mount",
            format!(
                "csrf_token={csrf}&app=alpha&host=%2Fhost%2Fdata&container=%2Fsrv%2Fdata&options=ro"
            ),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains(r#"data-run-url="/actions/runs/"#), "{body}");
    assert!(body.contains(r#"data-refresh="/volumes/partials/list""#));
    assert!(body.contains("Mounting volume into alpha"));

    let events = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(
        events.contains("event: line\ndata: -----> mounted"),
        "{events}"
    );
    assert!(events.contains(r#""ok":true"#), "{events}");
    assert!(client.calls().contains(&DokkuCommand::StorageMount {
        app: app_name("alpha"),
        mount: mount("/host/data:/srv/data:ro"),
    }));
}

#[tokio::test]
async fn volume_mount_non_htmx_flashes_restart_note() {
    let client = mounts_client().stub(
        DokkuCommand::StorageMount {
            app: app_name("alpha"),
            mount: mount("/host/data:/srv/data"),
        },
        Ok(DokkuOutput::ok("")),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        form_request(
            "/volumes/mount",
            format!(
                "csrf_token={csrf}&app=alpha&host=%2Fhost%2Fdata&container=%2Fsrv%2Fdata&options="
            ),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/volumes");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/volumes")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;

    assert!(body.contains("Mounted"), "{body}");
    assert!(
        body.contains("restart alpha for the change to take effect"),
        "{body}"
    );
}

#[tokio::test]
async fn hx_volume_unmount_streams_with_the_locator() {
    let client = mounts_client().stub(
        DokkuCommand::StorageUnmount {
            app: app_name("dokku-ui"),
            mount: mount("/var/lib/dokku/data/services/dokku-ui:/app/data:ro"),
        },
        Ok(DokkuOutput::ok("-----> unmounted\n")),
    );
    let (state, client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/volumes/unmount",
            format!(
                "csrf_token={csrf}&app=dokku-ui&spec=%2Fvar%2Flib%2Fdokku%2Fdata%2Fservices%2Fdokku-ui%3A%2Fapp%2Fdata%3Aro"
            ),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Unmounting volume from dokku-ui"));

    let events = sse_events(&app, &run_url(&body), &cookie).await;
    assert!(events.contains(r#""ok":true"#), "{events}");
    assert!(client.calls().contains(&DokkuCommand::StorageUnmount {
        app: app_name("dokku-ui"),
        mount: mount("/var/lib/dokku/data/services/dokku-ui:/app/data:ro"),
    }));
}

#[tokio::test]
async fn hx_volume_unmount_rejects_invalid_spec() {
    let (state, client, _dir) = test_state_with_shared_client(mounts_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        hx_form_request(
            "/volumes/unmount",
            format!("csrf_token={csrf}&app=dokku-ui&spec=not-a-mount"),
        )
        .cookie(cookie)
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("data-modal-error"), "{body}");
    assert!(
        client
            .calls()
            .iter()
            .all(|call| !matches!(call, DokkuCommand::StorageUnmount { .. })),
        "no unmount command for invalid spec"
    );
}

#[tokio::test]
async fn volume_unmount_non_htmx_flashes_and_redirects() {
    let client = mounts_client().stub(
        DokkuCommand::StorageUnmount {
            app: app_name("alpha"),
            mount: mount("/host/data:/srv/data"),
        },
        Ok(DokkuOutput::ok("")),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        form_request(
            "/volumes/unmount",
            format!("csrf_token={csrf}&app=alpha&spec=%2Fhost%2Fdata%3A%2Fsrv%2Fdata"),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/volumes");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/volumes")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("Unmounted"), "{body}");
}

#[tokio::test]
async fn volume_mount_non_htmx_invalid_spec_flashes_without_dokku() {
    let (state, client, _dir) = test_state_with_shared_client(mounts_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;
    let csrf = shell_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        form_request(
            "/volumes/mount",
            format!("csrf_token={csrf}&app=alpha&host=/host&container=relative&options="),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/volumes");
    let cookie = response_cookie(&resp).unwrap_or(cookie);

    assert!(
        client
            .calls()
            .iter()
            .all(|call| !matches!(call, DokkuCommand::StorageMount { .. })),
        "no mount command for invalid spec"
    );

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/volumes")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("must be absolute"), "{body}");
}

const LIST_ENTRIES: &str = include_str!("fixtures/list_entries.json");
const VOLUME_USAGE: &str = include_str!("fixtures/volume_usage.txt");

#[tokio::test]
async fn volumes_partial_maps_mounts_to_storage_entries() {
    let client = mounts_client().stub(
        DokkuCommand::StorageListEntries,
        Ok(DokkuOutput::ok(LIST_ENTRIES)),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/volumes/partials/list")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(
        body.contains(r#"hx-get="/volumes/partials/usage?entry=legacy-90db719326""#),
        "dokku-ui mount maps to its entry: {body}"
    );
    assert!(
        body.contains(r#"hx-get="/volumes/partials/usage?entry=legacy-151e4f1a23""#),
        "candid mount maps to its entry: {body}"
    );
}

#[tokio::test]
async fn usage_partial_renders_used_label_and_fs_tooltip() {
    let client = mounts_client().stub(
        DokkuCommand::StorageUsage {
            entry: "legacy-90db719326".into(),
        },
        Ok(DokkuOutput::ok(VOLUME_USAGE)),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/volumes/partials/usage?entry=legacy-90db719326")
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
    assert!(body.contains("164 KiB"), "{body}");
    assert!(body.contains(r#"title="81% of 154.9 GiB used""#), "{body}");
}

#[tokio::test]
async fn usage_partial_rejects_unknown_entry_without_calling_dokku() {
    let (state, client, _dir) = test_state_with_shared_client(mounts_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/volumes/partials/usage?entry=../evil")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("Unknown storage entry."), "{body}");
    assert!(
        client
            .calls()
            .iter()
            .all(|call| !matches!(call, DokkuCommand::StorageUsage { .. })),
        "no storage:exec for a tampered entry name"
    );
}

#[tokio::test]
async fn usage_partial_failure_degrades_to_a_dash() {
    let client = mounts_client().stub(
        DokkuCommand::StorageUsage {
            entry: "legacy-90db719326".into(),
        },
        Err(DokkuError::Exit {
            code: 1,
            stderr: "docker: not found".into(),
        }),
    );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/volumes/partials/usage?entry=legacy-90db719326")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("—"), "{body}");
    assert!(!body.contains("Could not load"), "{body}");
}

#[tokio::test]
async fn disk_summary_renders_host_filesystem_card() {
    let client = mounts_client()
        .stub(
            DokkuCommand::StorageListEntries,
            Ok(DokkuOutput::ok(LIST_ENTRIES)),
        )
        .stub(
            DokkuCommand::StorageUsage {
                entry: "legacy-151e4f1a23".into(),
            },
            Ok(DokkuOutput::ok(VOLUME_USAGE)),
        );
    let (state, _client, _dir) = test_state_with_shared_client(client).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/volumes/partials/disk-summary")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.contains("Host disk"), "{body}");
    assert!(body.contains("126.2 GiB used of 154.9 GiB"), "{body}");
    assert!(body.contains("(28.7 GiB free)"), "{body}");
    assert!(body.contains("<progress"), "{body}");
}

#[tokio::test]
async fn disk_summary_hidden_without_storage_entries() {
    let (state, _client, _dir) = test_state_with_shared_client(mounts_client()).await;
    let app = test::init_service(build_app(state)).await;
    let cookie = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/volumes/partials/disk-summary")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;

    assert!(body.trim().is_empty(), "no entries, no card: {body:?}");
}
