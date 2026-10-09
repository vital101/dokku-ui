use std::collections::HashMap;

use actix_session::Session;
use actix_web::{HttpRequest, HttpResponse, web};
use askama::Template;
use serde::Deserialize;

use crate::dokku::{DokkuError, plugin_services, service_linked_apps, service_logs, service_stats};
use crate::domain::AppName;
use crate::domain::Support;
use crate::domain::command::DokkuCommand;
use crate::domain::job::{JobSpec, ServiceAction as JobServiceAction};
use crate::domain::parse::{LOG_LINES_MAX, LOG_LINES_MIN, clamp_log_lines, parse_service_list};
use crate::domain::service_create::ServiceCreateOptions;
use crate::domain::service_name::ServiceName;
use crate::domain::service_plugin::ServicePlugin;
use crate::domain::types::ServiceInfo;
use crate::error::AppError;
use crate::storage::runs::TargetKind;
use crate::web::csrf_form::{CsrfForm, ensure_csrf};
use crate::web::flash::{FlashLevel, FlashMessage, set_flash, take_flash};
use crate::web::fragments::{
    RunCompletion, RunRefresh, RunRequest, current_user, enqueue_action_run, error_fragment,
    is_htmx, modal_error, reauth_valid_here, run_synchronously, start_action_run,
};
use crate::web::render::{render, see_other};
use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "services/list.html")]
struct ListPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    plugin: ServicePlugin,
    can_manage: bool,
}

#[derive(Template)]
#[template(path = "services/partials/list.html")]
struct ListPartial {
    plugin: ServicePlugin,
    services: Vec<ServiceInfo>,
    can_manage: bool,
}

#[derive(Template)]
#[template(path = "partials/service_nav.html")]
struct ServiceNavPartial {
    plugins: Vec<ServicePlugin>,
}

#[derive(Template)]
#[template(path = "services/new.html")]
struct NewFormPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    plugin: ServicePlugin,
}

#[derive(Template)]
#[template(path = "services/show.html")]
struct ShowPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    plugin: ServicePlugin,
    service: &'a str,
    active_tab: &'static str,
    can_manage: bool,
}

#[derive(Template)]
#[template(path = "services/links.html")]
struct LinksPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    plugin: ServicePlugin,
    service: &'a str,
    active_tab: &'static str,
}

#[derive(Template)]
#[template(path = "services/logs.html")]
struct LogsPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    plugin: ServicePlugin,
    service: &'a str,
    active_tab: &'static str,
    line_count: u32,
    min_lines: u32,
    max_lines: u32,
}

#[derive(Template)]
#[template(path = "services/partials/overview.html")]
struct OverviewPartial<'a> {
    plugin: ServicePlugin,
    service: &'a str,
    csrf_token: &'a str,
    info: ServiceInfo,
    can_manage: bool,
}

#[derive(Template)]
#[template(path = "services/partials/dsn_revealed.html")]
struct DsnRevealedPartial {
    dsn: String,
}

#[derive(Template)]
#[template(path = "services/partials/links.html")]
struct LinksPartial<'a> {
    plugin: ServicePlugin,
    service: &'a str,
    csrf_token: &'a str,
    links: Vec<String>,
    available_apps: Vec<String>,
    can_manage: bool,
}

#[derive(Template)]
#[template(path = "services/partials/logs.html")]
struct LogsPartial {
    lines: Vec<String>,
}

#[derive(Template)]
#[template(path = "services/partials/stats.html")]
struct StatsPartial {
    not_running: bool,
    memory_used_label: String,
    memory_detail_label: String,
    memory_percent: Option<u32>,
    cpu_percent_label: String,
    cpu_total_label: String,
    data_label: String,
    fs_used_label: String,
    fs_total_label: String,
    fs_avail_label: String,
    fs_percent: Option<u32>,
}

impl StatsPartial {
    /// Placeholder values for the not-running state; the template only reads
    /// `not_running` in that branch.
    fn unavailable() -> Self {
        Self {
            not_running: true,
            memory_used_label: "—".into(),
            memory_detail_label: "—".into(),
            memory_percent: None,
            cpu_percent_label: "—".into(),
            cpu_total_label: "—".into(),
            data_label: "—".into(),
            fs_used_label: "—".into(),
            fs_total_label: "—".into(),
            fs_avail_label: "—".into(),
            fs_percent: None,
        }
    }
}

#[derive(Template)]
#[template(path = "services/delete_confirm.html")]
struct DeleteConfirmPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    plugin: ServicePlugin,
    service: &'a str,
}

#[derive(Template)]
#[template(path = "services/partials/delete_confirm_modal.html")]
struct DeleteConfirmModalPartial<'a> {
    plugin: ServicePlugin,
    service: &'a str,
    csrf_token: &'a str,
}

/// Rejects `{plugin}` URL segments that are not supported service plugins.
fn plugin_from_slug(raw: &str) -> Result<ServicePlugin, AppError> {
    ServicePlugin::try_from(raw).map_err(|_| AppError::NotFound)
}

/// Rejects `{service}` URL segments that cannot be service names.
fn service_from_slug(raw: &str) -> Result<ServiceName, AppError> {
    ServiceName::try_from(raw).map_err(|_| AppError::NotFound)
}

fn detail_url(plugin: ServicePlugin, service: &ServiceName) -> String {
    format!("/services/{plugin}/{service}")
}

/// 404s a service the plugin does not list, so full detail pages mirror the
/// unknown-app behavior.
async fn ensure_service_exists(
    state: &AppState,
    plugin: ServicePlugin,
    service: &ServiceName,
) -> Result<(), AppError> {
    let output = state
        .dokku
        .exec(&DokkuCommand::ServiceList { plugin })
        .await?;
    if parse_service_list(&output.stdout)
        .iter()
        .any(|name| name == service.as_str())
    {
        Ok(())
    } else {
        Err(AppError::NotFound)
    }
}

/// Everything the service detail shells need: validated plugin/service, the
/// session user, and a CSRF token.
pub(super) struct DetailContext {
    pub(super) plugin: ServicePlugin,
    pub(super) service: ServiceName,
    pub(super) email: String,
    pub(super) csrf_token: String,
    pub(super) flash: Option<FlashMessage>,
    pub(super) can_manage: bool,
}

pub(super) async fn detail_context(
    state: &AppState,
    session: &Session,
    raw_plugin: &str,
    raw_service: &str,
) -> Result<DetailContext, AppError> {
    let plugin = plugin_from_slug(raw_plugin)?;
    let service = service_from_slug(raw_service)?;
    let user = current_user(state, session).await?;
    ensure_service_exists(state, plugin, &service).await?;
    let csrf_token = ensure_csrf(session).await?;
    let flash = take_flash(session);
    Ok(DetailContext {
        plugin,
        service,
        email: user.email,
        csrf_token,
        flash,
        can_manage: user.role.can_manage_apps(),
    })
}

pub async fn index(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let plugin = plugin_from_slug(&path.into_inner())?;
    let user = current_user(&state, &session).await?;
    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);

    render(&ListPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        plugin,
        can_manage: user.role.can_manage_apps(),
    })
}

pub async fn list_partial(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let plugin = plugin_from_slug(&path.into_inner())?;
    let user = current_user(&state, &session).await?;

    let retry_url = format!("/services/{plugin}/partials/list");
    match plugin_services(&*state.dokku, plugin).await {
        Ok(services) => render(&ListPartial {
            plugin,
            services,
            can_manage: user.role.can_manage_apps(),
        }),
        Err(err) => error_fragment(&retry_url, &err.to_string()),
    }
}

/// Sidebar fragment: renders links only for service plugins the host actually
/// has installed. A failed probe degrades to the full catalog rather than
/// hiding services.
pub async fn service_nav(
    state: web::Data<AppState>,
    session: Session,
) -> Result<HttpResponse, AppError> {
    current_user(&state, &session).await?;

    let plugins: Vec<ServicePlugin> = match state.capabilities.ensure_loaded().await {
        Ok(capabilities) => ServicePlugin::all()
            .filter(|plugin| {
                !matches!(
                    capabilities.supports_plugin(plugin.as_str()),
                    Support::PluginMissing { .. }
                )
            })
            .collect(),
        Err(err) => {
            tracing::warn!(error = %err, "capability probe failed; showing every service plugin");
            ServicePlugin::all().collect()
        }
    };
    render(&ServiceNavPartial { plugins })
}

pub async fn new_form(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let plugin = plugin_from_slug(&path.into_inner())?;
    let user = current_user(&state, &session).await?;
    if !user.role.can_manage_apps() {
        set_flash(
            &session,
            FlashLevel::Error,
            "Your role does not allow creating services.",
        );
        return Ok(see_other(&format!("/services/{plugin}")));
    }
    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);

    render(&NewFormPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        plugin,
    })
}

#[derive(Deserialize)]
pub struct CreateForm {
    name: String,
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    image_version: Option<String>,
    #[serde(default)]
    custom_env: Option<String>,
    #[serde(default)]
    config_options: Option<String>,
}

pub async fn create(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<CreateForm>,
) -> Result<HttpResponse, AppError> {
    let plugin = plugin_from_slug(&path.into_inner())?;
    let form = form.0;
    let name = form.name.trim().to_owned();

    let fail = |message: String| {
        if is_htmx(&req) {
            modal_error(message)
        } else {
            set_flash(&session, FlashLevel::Error, message);
            Ok(see_other(&format!("/services/{plugin}/new")))
        }
    };

    let service = match ServiceName::try_from(name.as_str()) {
        Ok(service) => service,
        Err(err) => return fail(format!("Invalid service name: {err}")),
    };

    let options = match ServiceCreateOptions::parse(
        form.image.as_deref(),
        form.image_version.as_deref(),
        form.custom_env.as_deref(),
        form.config_options.as_deref(),
    ) {
        Ok(options) => options,
        Err(err) => return fail(err.to_string()),
    };
    let redactions = options.redaction_fragments();
    let redirect_to = detail_url(plugin, &service);

    if is_htmx(&req) {
        return start_action_run(
            &state,
            &session,
            &RunRequest {
                subject: service.as_str().to_owned(),
                operation: "service.create".to_owned(),
                target_kind: TargetKind::Service,
                title: format!("Creating {service}…"),
                plan: vec![JobSpec::ServiceCreate {
                    plugin: plugin.as_str().to_owned(),
                    service: service.as_str().to_owned(),
                    options,
                }],
                completion: RunCompletion {
                    success_message: format!("Service '{name}' created."),
                    redirect: Some(redirect_to),
                    refresh: RunRefresh::None,
                },
                redactions,
                refresh_url: None,
            },
        )
        .await;
    }

    match run_synchronously(
        &state,
        &session,
        &name,
        "service.create",
        TargetKind::Service,
        DokkuCommand::ServiceCreate {
            plugin,
            service,
            options,
        },
        &format!("Service '{name}' created."),
        &redactions,
    )
    .await
    {
        Ok(_) => {
            set_flash(
                &session,
                FlashLevel::Success,
                format!("Service '{name}' created."),
            );
            Ok(see_other(&redirect_to))
        }
        Err(err) => {
            set_flash(
                &session,
                FlashLevel::Error,
                format!("Failed to create service: {err}"),
            );
            Ok(see_other(&format!("/services/{plugin}/new")))
        }
    }
}

pub async fn show(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<(String, String)>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    let ctx = detail_context(&state, &session, &raw_plugin, &raw_service).await?;

    render(&ShowPage {
        email: &ctx.email,
        csrf_token: &ctx.csrf_token,
        flash: ctx.flash.as_ref(),
        plugin: ctx.plugin,
        service: ctx.service.as_str(),
        active_tab: "overview",
        can_manage: ctx.can_manage,
    })
}

pub async fn links_page(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<(String, String)>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    let ctx = detail_context(&state, &session, &raw_plugin, &raw_service).await?;

    render(&LinksPage {
        email: &ctx.email,
        csrf_token: &ctx.csrf_token,
        flash: ctx.flash.as_ref(),
        plugin: ctx.plugin,
        service: ctx.service.as_str(),
        active_tab: "links",
    })
}

pub async fn logs_page(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<(String, String)>,
    query: web::Query<HashMap<String, String>>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    let ctx = detail_context(&state, &session, &raw_plugin, &raw_service).await?;
    let line_count = clamp_log_lines(query.get("lines").map(String::as_str));

    render(&LogsPage {
        email: &ctx.email,
        csrf_token: &ctx.csrf_token,
        flash: ctx.flash.as_ref(),
        plugin: ctx.plugin,
        service: ctx.service.as_str(),
        active_tab: "logs",
        line_count,
        min_lines: LOG_LINES_MIN,
        max_lines: LOG_LINES_MAX,
    })
}

pub async fn overview_partial(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<(String, String)>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    let plugin = plugin_from_slug(&raw_plugin)?;
    let service = service_from_slug(&raw_service)?;
    let user = current_user(&state, &session).await?;

    let retry_url = format!("/services/{plugin}/{service}/partials/overview");
    match crate::dokku::service_info(&*state.dokku, plugin.as_str(), service.as_str()).await {
        Ok(Some(info)) => {
            let csrf_token = ensure_csrf(&session).await?;
            render(&OverviewPartial {
                plugin,
                service: service.as_str(),
                csrf_token: &csrf_token,
                info,
                can_manage: user.role.can_manage_apps(),
            })
        }
        Ok(None) => error_fragment(
            &retry_url,
            &format!("Service '{service}' was not found — it may have been destroyed."),
        ),
        Err(err) => error_fragment(&retry_url, &err.to_string()),
    }
}

/// Reveals the service's connection string under re-auth, marked `no-store`,
/// and records a `service.dsn.reveal` audit entry. Without a valid re-auth
/// window the user is sent through `/reauth` first.
pub async fn dsn_reveal(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<(String, String)>,
    _form: CsrfForm<super::apps::ActionForm>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    let plugin = plugin_from_slug(&raw_plugin)?;
    let service = service_from_slug(&raw_service)?;
    current_user(&state, &session).await?;
    let retry_url = format!("/services/{plugin}/{service}/partials/overview");
    if !reauth_valid_here(&session) {
        return Ok(see_other(&format!(
            "/reauth?next=/services/{plugin}/{service}"
        )));
    }
    let info =
        match crate::dokku::service_info(&*state.dokku, plugin.as_str(), service.as_str()).await {
            Ok(Some(info)) => info,
            Ok(None) => {
                return error_fragment(&retry_url, "The service was not found.");
            }
            Err(err) => return error_fragment(&retry_url, &err.to_string()),
        };
    let Some(dsn) = info.dsn else {
        return error_fragment(
            &retry_url,
            "No connection string was reported for this service.",
        );
    };
    record_dsn_reveal(&state, &session, service.as_str()).await;
    let mut response = render(&DsnRevealedPartial { dsn })?;
    response.headers_mut().insert(
        actix_web::http::header::CACHE_CONTROL,
        actix_web::http::header::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}

/// Best-effort `service.dsn.reveal` audit entry (no lines; the DSN itself
/// never reaches the audit trail).
async fn record_dsn_reveal(state: &AppState, session: &Session, service: &str) {
    let actor = crate::web::fragments::current_actor(state, session).await;
    if let Ok(run_id) = state
        .action_runs
        .insert_with(&crate::storage::runs::NewRun {
            subject: service.to_owned(),
            operation: "service.dsn.reveal".to_owned(),
            target_kind: crate::storage::runs::TargetKind::Service,
            actor,
            parent_run_id: None,
        })
        .await
    {
        let _ = state
            .action_runs
            .finish(
                &run_id,
                &crate::storage::runs::RunOutcome {
                    ok: true,
                    message: "Connection string revealed.".to_owned(),
                    redirect: None,
                },
            )
            .await;
    }
}

pub async fn links_partial(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<(String, String)>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    let plugin = plugin_from_slug(&raw_plugin)?;
    let service = service_from_slug(&raw_service)?;
    let user = current_user(&state, &session).await?;

    let retry_url = format!("/services/{plugin}/{service}/partials/links");
    let links = match service_linked_apps(&*state.dokku, plugin, &service).await {
        Ok(links) => links,
        Err(err) => return error_fragment(&retry_url, &err.to_string()),
    };

    // The link form picks from the app snapshot; a snapshot failure degrades
    // to "no apps available" rather than taking the whole panel down.
    let mut available_apps = state
        .snapshot
        .ensure_loaded()
        .await
        .map(|snapshot| snapshot.apps.clone())
        .unwrap_or_default();
    available_apps.retain(|app| !links.iter().any(|linked| linked == app));

    let csrf_token = ensure_csrf(&session).await?;
    render(&LinksPartial {
        plugin,
        service: service.as_str(),
        csrf_token: &csrf_token,
        links,
        available_apps,
        can_manage: user.role.can_manage_apps(),
    })
}

pub async fn logs_partial(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<(String, String)>,
    query: web::Query<HashMap<String, String>>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    let plugin = plugin_from_slug(&raw_plugin)?;
    let service = service_from_slug(&raw_service)?;
    current_user(&state, &session).await?;

    let num_lines = clamp_log_lines(query.get("lines").map(String::as_str));
    let retry_url = format!("/services/{plugin}/{service}/partials/logs?lines={num_lines}");
    match service_logs(&*state.dokku, plugin, &service, num_lines).await {
        Ok(logs) => render(&LogsPartial {
            lines: logs.as_slice().to_vec(),
        }),
        Err(err) => error_fragment(&retry_url, &err.to_string()),
    }
}

/// Streams a service's logs live over SSE (`<plugin>:logs <svc> --tail 200`).
/// Each viewer owns one SSH channel; dropping the stream aborts the channel
/// without harming the shared session, exactly like the app log stream.
pub async fn log_stream(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<(String, String)>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    let plugin = plugin_from_slug(&raw_plugin)?;
    let service = service_from_slug(&raw_service)?;
    current_user(&state, &session).await?;

    // A plugin the capability probe says is absent gets an explanatory SSE
    // error instead of a failed SSH attempt.
    if let Some(caps) = state.capabilities.current().await {
        let support = caps.supports_plugin(plugin.as_str());
        if matches!(support, Support::PluginMissing { .. }) {
            let message = support.label();
            let event = actix_web::web::Bytes::from(format!("event: error\ndata: {message}\n\n"));
            let chunk: Result<actix_web::web::Bytes, std::io::Error> = Ok(event);
            return Ok(HttpResponse::Ok()
                .content_type("text/event-stream")
                .insert_header(("Cache-Control", "no-store"))
                .insert_header(("X-Accel-Buffering", "no"))
                .streaming(futures_util::stream::iter(vec![chunk])));
        }
    }

    let command = DokkuCommand::ServiceLogs {
        plugin,
        service,
        num_lines: 200,
        follow: true,
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);
    let client = state.dokku.clone();
    let task = tokio::spawn(async move {
        let _ = client.exec_streaming(&command, tx).await;
    });

    Ok(HttpResponse::Ok()
        .content_type("text/event-stream")
        .insert_header(("Cache-Control", "no-store"))
        .insert_header(("X-Accel-Buffering", "no"))
        .streaming(crate::web::logs::live_stream(rx, task)))
}

/// Live resource usage, fetched on demand by the Overview tab's Resources
/// card. A stopped container is a state, not an error.
pub async fn stats_partial(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<(String, String)>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    let plugin = plugin_from_slug(&raw_plugin)?;
    let service = service_from_slug(&raw_service)?;
    current_user(&state, &session).await?;

    let retry_url = format!("/services/{plugin}/{service}/partials/stats");
    match service_stats(&*state.dokku, plugin, &service).await {
        Ok(Some(stats)) => render(&StatsPartial {
            not_running: false,
            memory_used_label: stats.memory_used_label(),
            memory_detail_label: stats.memory_detail_label(),
            memory_percent: stats.memory_percent_rounded(),
            cpu_percent_label: stats.cpu_percent_label(),
            cpu_total_label: stats.cpu_total_label(),
            data_label: stats.data_label(),
            fs_used_label: stats.fs_used_label(),
            fs_total_label: stats.fs_total_label(),
            fs_avail_label: stats.fs_avail_label(),
            fs_percent: stats.fs_percent_rounded(),
        }),
        Ok(None) => error_fragment(
            &retry_url,
            "No stats were reported by the service container.",
        ),
        Err(err) if service_not_running(&err) => render(&StatsPartial::unavailable()),
        Err(err) => error_fragment(&retry_url, &err.to_string()),
    }
}

/// The service plugins fail `enter` with these messages when the container is
/// stopped or was removed; the stats card renders that as a state.
fn service_not_running(err: &DokkuError) -> bool {
    matches!(
        err,
        DokkuError::Exit { stderr, .. }
            if stderr.contains("not running") || stderr.contains("does not exist")
    )
}

/// No-JS confirmation page, mirroring the app delete flow. The modal fragment
/// (`delete_confirm_modal`) serves the same purpose when htmx is available.
pub async fn delete_confirm(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<(String, String)>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    let plugin = plugin_from_slug(&raw_plugin)?;
    let service = service_from_slug(&raw_service)?;
    let user = current_user(&state, &session).await?;
    if !user.role.can_manage_apps() {
        set_flash(
            &session,
            FlashLevel::Error,
            "Your role does not allow destroying services.",
        );
        return Ok(see_other(&detail_url(plugin, &service)));
    }
    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);

    render(&DeleteConfirmPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        plugin,
        service: service.as_str(),
    })
}

pub async fn delete_confirm_modal(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<(String, String)>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    let plugin = plugin_from_slug(&raw_plugin)?;
    let service = service_from_slug(&raw_service)?;
    let user = current_user(&state, &session).await?;
    if !user.role.can_manage_apps() {
        return error_fragment(
            &format!("/services/{plugin}/{service}/partials/overview"),
            "Your role does not allow destroying services.",
        );
    }
    let csrf_token = ensure_csrf(&session).await?;

    render(&DeleteConfirmModalPartial {
        plugin,
        service: service.as_str(),
        csrf_token: &csrf_token,
    })
}

#[derive(Deserialize)]
pub struct ActionForm {}

#[derive(Clone, Copy)]
enum ServiceAction {
    Start,
    Stop,
    Restart,
}

impl ServiceAction {
    fn verb(self) -> &'static str {
        match self {
            ServiceAction::Start => "start",
            ServiceAction::Stop => "stop",
            ServiceAction::Restart => "restart",
        }
    }

    fn past_tense(self) -> &'static str {
        match self {
            ServiceAction::Start => "started",
            ServiceAction::Stop => "stopped",
            ServiceAction::Restart => "restarted",
        }
    }

    fn present_participle(self) -> &'static str {
        match self {
            ServiceAction::Start => "Starting",
            ServiceAction::Stop => "Stopping",
            ServiceAction::Restart => "Restarting",
        }
    }

    /// The serializable job form of this action (used by the queued run path).
    fn job(self, plugin: &ServicePlugin, service: &ServiceName) -> JobSpec {
        let action = match self {
            ServiceAction::Start => JobServiceAction::Start,
            ServiceAction::Stop => JobServiceAction::Stop,
            ServiceAction::Restart => JobServiceAction::Restart,
        };
        JobSpec::ServiceAction {
            plugin: plugin.as_str().to_owned(),
            service: service.as_str().to_owned(),
            action,
        }
    }
}

pub async fn start(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<(String, String)>,
    _form: CsrfForm<ActionForm>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    process_action(
        &state,
        &session,
        &req,
        &raw_plugin,
        &raw_service,
        ServiceAction::Start,
    )
    .await
}

pub async fn stop(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<(String, String)>,
    _form: CsrfForm<ActionForm>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    process_action(
        &state,
        &session,
        &req,
        &raw_plugin,
        &raw_service,
        ServiceAction::Stop,
    )
    .await
}

pub async fn restart(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<(String, String)>,
    _form: CsrfForm<ActionForm>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    process_action(
        &state,
        &session,
        &req,
        &raw_plugin,
        &raw_service,
        ServiceAction::Restart,
    )
    .await
}

async fn process_action(
    state: &AppState,
    session: &Session,
    req: &HttpRequest,
    raw_plugin: &str,
    raw_service: &str,
    action: ServiceAction,
) -> Result<HttpResponse, AppError> {
    let plugin = plugin_from_slug(raw_plugin)?;
    let service = service_from_slug(raw_service)?;
    let redirect_to = detail_url(plugin, &service);

    if is_htmx(req) {
        return start_action_run(
            state,
            session,
            &RunRequest {
                subject: service.as_str().to_owned(),
                operation: format!("service.{}", action.verb()),
                target_kind: TargetKind::Service,
                title: format!("{} {service}…", action.present_participle()),
                plan: vec![action.job(&plugin, &service)],
                completion: RunCompletion {
                    success_message: format!("Service '{service}' {}.", action.past_tense()),
                    redirect: None,
                    refresh: RunRefresh::None,
                },
                redactions: Vec::new(),
                refresh_url: Some(format!("/services/{plugin}/{service}/partials/overview")),
            },
        )
        .await;
    }

    let _ = enqueue_action_run(
        state,
        session,
        service.as_str(),
        &format!("service.{}", action.verb()),
        TargetKind::Service,
        &[action.job(&plugin, &service)],
        &RunCompletion {
            success_message: format!("Service '{service}' {}.", action.past_tense()),
            redirect: None,
            refresh: RunRefresh::None,
        },
        &[],
    )
    .await?;
    set_flash(
        session,
        FlashLevel::Success,
        format!("Queued: {} {service}.", action.verb()),
    );
    Ok(see_other(&redirect_to))
}

#[derive(Deserialize)]
pub struct DestroyForm {
    name: String,
}

pub async fn destroy(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<(String, String)>,
    form: CsrfForm<DestroyForm>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    let plugin = plugin_from_slug(&raw_plugin)?;
    let service = service_from_slug(&raw_service)?;

    if form.0.name.trim() != service.as_str() {
        let message = format!("Type `{service}` to confirm destruction.");
        if is_htmx(&req) {
            return modal_error(message);
        }
        set_flash(&session, FlashLevel::Error, message);
        return Ok(see_other(&format!("/services/{plugin}/{service}/delete")));
    }

    // Pro blocks destroying a linked service (unlinking unsets env vars and
    // restarts the app). Fail closed when the check itself cannot run.
    let linked = match service_linked_apps(&*state.dokku, plugin, &service).await {
        Ok(links) => links,
        Err(err) => {
            let message = format!("Could not verify service links: {err}");
            if is_htmx(&req) {
                return modal_error(message);
            }
            set_flash(&session, FlashLevel::Error, message);
            return Ok(see_other(&detail_url(plugin, &service)));
        }
    };
    if !linked.is_empty() {
        let message = format!(
            "Unlink {} first: {}. Destroying the service would break those apps.",
            if linked.len() == 1 {
                "this app"
            } else {
                "these apps"
            },
            linked.join(", ")
        );
        if is_htmx(&req) {
            return modal_error(message);
        }
        set_flash(&session, FlashLevel::Error, message);
        return Ok(see_other(&detail_url(plugin, &service)));
    }

    let list_url = format!("/services/{plugin}");
    if is_htmx(&req) {
        return start_action_run(
            &state,
            &session,
            &RunRequest {
                subject: service.as_str().to_owned(),
                operation: "service.destroy".to_owned(),
                target_kind: TargetKind::Service,
                title: format!("Destroying {service}…"),
                plan: vec![JobSpec::ServiceDestroy {
                    plugin: plugin.as_str().to_owned(),
                    service: service.as_str().to_owned(),
                }],
                completion: RunCompletion {
                    success_message: format!("Service '{service}' destroyed."),
                    redirect: Some(list_url.clone()),
                    refresh: RunRefresh::None,
                },
                redactions: Vec::new(),
                refresh_url: None,
            },
        )
        .await;
    }

    let _ = enqueue_action_run(
        &state,
        &session,
        service.as_str(),
        "service.destroy",
        TargetKind::Service,
        &[JobSpec::ServiceDestroy {
            plugin: plugin.as_str().to_owned(),
            service: service.as_str().to_owned(),
        }],
        &RunCompletion {
            success_message: format!("Service '{service}' destroyed."),
            redirect: Some(list_url.clone()),
            refresh: RunRefresh::None,
        },
        &[],
    )
    .await?;
    set_flash(
        &session,
        FlashLevel::Success,
        format!("Queued: destroy {service}."),
    );
    Ok(see_other(&list_url))
}

#[derive(Deserialize)]
pub struct ExposeForm {
    ports: String,
}

pub async fn expose(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<(String, String)>,
    form: CsrfForm<ExposeForm>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    let plugin = plugin_from_slug(&raw_plugin)?;
    let service = service_from_slug(&raw_service)?;
    let ports = form.0.ports.trim().to_owned();

    if ports.is_empty()
        || ports
            .chars()
            .any(|c| !(c.is_ascii_digit() || c == '.' || c == ':'))
    {
        let message = "Enter a port, or an address and port (e.g. `127.0.0.1:5432`).".to_owned();
        if is_htmx(&req) {
            return modal_error(message);
        }
        set_flash(&session, FlashLevel::Error, message);
        return Ok(see_other(&detail_url(plugin, &service)));
    }

    if is_htmx(&req) {
        return start_action_run(
            &state,
            &session,
            &RunRequest {
                subject: service.as_str().to_owned(),
                operation: "service.expose".to_owned(),
                target_kind: TargetKind::Service,
                title: format!("Exposing {service}…"),
                plan: vec![JobSpec::ServiceExpose {
                    plugin: plugin.as_str().to_owned(),
                    service: service.as_str().to_owned(),
                    ports: ports.clone(),
                }],
                completion: RunCompletion {
                    success_message: format!("Service '{service}' exposed on {ports}."),
                    redirect: None,
                    refresh: RunRefresh::None,
                },
                redactions: Vec::new(),
                refresh_url: Some(format!("/services/{plugin}/{service}/partials/overview")),
            },
        )
        .await;
    }

    let _ = enqueue_action_run(
        &state,
        &session,
        service.as_str(),
        "service.expose",
        TargetKind::Service,
        &[JobSpec::ServiceExpose {
            plugin: plugin.as_str().to_owned(),
            service: service.as_str().to_owned(),
            ports,
        }],
        &RunCompletion {
            success_message: format!("Service '{service}' exposed."),
            redirect: None,
            refresh: RunRefresh::None,
        },
        &[],
    )
    .await?;
    set_flash(
        &session,
        FlashLevel::Success,
        format!("Queued: expose {service}."),
    );
    Ok(see_other(&detail_url(plugin, &service)))
}

pub async fn unexpose(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<(String, String)>,
    _form: CsrfForm<ActionForm>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    let plugin = plugin_from_slug(&raw_plugin)?;
    let service = service_from_slug(&raw_service)?;
    let redirect_to = detail_url(plugin, &service);

    if is_htmx(&req) {
        return start_action_run(
            &state,
            &session,
            &RunRequest {
                subject: service.as_str().to_owned(),
                operation: "service.unexpose".to_owned(),
                target_kind: TargetKind::Service,
                title: format!("Unexposing {service}…"),
                plan: vec![JobSpec::ServiceUnexpose {
                    plugin: plugin.as_str().to_owned(),
                    service: service.as_str().to_owned(),
                }],
                completion: RunCompletion {
                    success_message: format!("Service '{service}' unexposed."),
                    redirect: None,
                    refresh: RunRefresh::None,
                },
                redactions: Vec::new(),
                refresh_url: Some(format!("/services/{plugin}/{service}/partials/overview")),
            },
        )
        .await;
    }

    let _ = enqueue_action_run(
        &state,
        &session,
        service.as_str(),
        "service.unexpose",
        TargetKind::Service,
        &[JobSpec::ServiceUnexpose {
            plugin: plugin.as_str().to_owned(),
            service: service.as_str().to_owned(),
        }],
        &RunCompletion {
            success_message: format!("Service '{service}' unexposed."),
            redirect: None,
            refresh: RunRefresh::None,
        },
        &[],
    )
    .await?;
    set_flash(
        &session,
        FlashLevel::Success,
        format!("Queued: unexpose {service}."),
    );
    Ok(see_other(&redirect_to))
}

#[derive(Deserialize)]
pub struct LinkForm {
    app: String,
}

pub async fn link(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<(String, String)>,
    form: CsrfForm<LinkForm>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    let plugin = plugin_from_slug(&raw_plugin)?;
    let service = service_from_slug(&raw_service)?;

    let app = match AppName::try_from(form.0.app.trim()) {
        Ok(app) => app,
        Err(err) => {
            let message = format!("Invalid app name: {err}");
            if is_htmx(&req) {
                return modal_error(message);
            }
            set_flash(&session, FlashLevel::Error, message);
            return Ok(see_other(&format!("/services/{plugin}/{service}/links")));
        }
    };
    let links_url = format!("/services/{plugin}/{service}/links");

    if is_htmx(&req) {
        return start_action_run(
            &state,
            &session,
            &RunRequest {
                subject: service.as_str().to_owned(),
                operation: "service.link".to_owned(),
                target_kind: TargetKind::Service,
                title: format!("Linking {service} to {app}…"),
                plan: vec![JobSpec::ServiceLink {
                    plugin: plugin.as_str().to_owned(),
                    service: service.as_str().to_owned(),
                    app: app.as_str().to_owned(),
                }],
                completion: RunCompletion {
                    success_message: format!("Linked '{service}' to '{app}'."),
                    redirect: None,
                    refresh: RunRefresh::None,
                },
                redactions: Vec::new(),
                refresh_url: Some(format!("/services/{plugin}/{service}/partials/links")),
            },
        )
        .await;
    }

    let _ = enqueue_action_run(
        &state,
        &session,
        service.as_str(),
        "service.link",
        TargetKind::Service,
        &[JobSpec::ServiceLink {
            plugin: plugin.as_str().to_owned(),
            service: service.as_str().to_owned(),
            app: app.as_str().to_owned(),
        }],
        &RunCompletion {
            success_message: format!("Linked '{service}' to '{app}'."),
            redirect: None,
            refresh: RunRefresh::None,
        },
        &[],
    )
    .await?;
    set_flash(
        &session,
        FlashLevel::Success,
        format!("Queued: link {service} to {app}."),
    );
    Ok(see_other(&links_url))
}

pub async fn unlink(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<(String, String)>,
    form: CsrfForm<LinkForm>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    let plugin = plugin_from_slug(&raw_plugin)?;
    let service = service_from_slug(&raw_service)?;

    let app = match AppName::try_from(form.0.app.trim()) {
        Ok(app) => app,
        Err(err) => {
            let message = format!("Invalid app name: {err}");
            if is_htmx(&req) {
                return modal_error(message);
            }
            set_flash(&session, FlashLevel::Error, message);
            return Ok(see_other(&format!("/services/{plugin}/{service}/links")));
        }
    };
    let links_url = format!("/services/{plugin}/{service}/links");

    if is_htmx(&req) {
        return start_action_run(
            &state,
            &session,
            &RunRequest {
                subject: service.as_str().to_owned(),
                operation: "service.unlink".to_owned(),
                target_kind: TargetKind::Service,
                title: format!("Unlinking {service} from {app}…"),
                plan: vec![JobSpec::ServiceUnlink {
                    plugin: plugin.as_str().to_owned(),
                    service: service.as_str().to_owned(),
                    app: app.as_str().to_owned(),
                }],
                completion: RunCompletion {
                    success_message: format!("Unlinked '{service}' from '{app}'."),
                    redirect: None,
                    refresh: RunRefresh::None,
                },
                redactions: Vec::new(),
                refresh_url: Some(format!("/services/{plugin}/{service}/partials/links")),
            },
        )
        .await;
    }

    let _ = enqueue_action_run(
        &state,
        &session,
        service.as_str(),
        "service.unlink",
        TargetKind::Service,
        &[JobSpec::ServiceUnlink {
            plugin: plugin.as_str().to_owned(),
            service: service.as_str().to_owned(),
            app: app.as_str().to_owned(),
        }],
        &RunCompletion {
            success_message: format!("Unlinked '{service}' from '{app}'."),
            redirect: None,
            refresh: RunRefresh::None,
        },
        &[],
    )
    .await?;
    set_flash(
        &session,
        FlashLevel::Success,
        format!("Queued: unlink {service} from {app}."),
    );
    Ok(see_other(&links_url))
}

/// Promotes the service to the app's primary URL variable (`<plugin>:promote`),
/// restarting the app. Mirrors the link/unlink plumbing.
pub async fn promote(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<(String, String)>,
    form: CsrfForm<LinkForm>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    let plugin = plugin_from_slug(&raw_plugin)?;
    let service = service_from_slug(&raw_service)?;

    let app = match AppName::try_from(form.0.app.trim()) {
        Ok(app) => app,
        Err(err) => {
            let message = format!("Invalid app name: {err}");
            if is_htmx(&req) {
                return modal_error(message);
            }
            set_flash(&session, FlashLevel::Error, message);
            return Ok(see_other(&format!("/services/{plugin}/{service}/links")));
        }
    };
    let links_url = format!("/services/{plugin}/{service}/links");
    let success_message = format!("Promoted '{service}' for '{app}'.");

    if is_htmx(&req) {
        return start_action_run(
            &state,
            &session,
            &RunRequest {
                subject: service.as_str().to_owned(),
                operation: "service.promote".to_owned(),
                target_kind: TargetKind::Service,
                title: format!("Promoting {service} for {app}…"),
                plan: vec![JobSpec::ServicePromote {
                    plugin: plugin.as_str().to_owned(),
                    service: service.as_str().to_owned(),
                    app: app.as_str().to_owned(),
                }],
                completion: RunCompletion {
                    success_message,
                    redirect: None,
                    refresh: RunRefresh::None,
                },
                redactions: Vec::new(),
                refresh_url: Some(format!("/services/{plugin}/{service}/partials/links")),
            },
        )
        .await;
    }

    let _ = enqueue_action_run(
        &state,
        &session,
        service.as_str(),
        "service.promote",
        TargetKind::Service,
        &[JobSpec::ServicePromote {
            plugin: plugin.as_str().to_owned(),
            service: service.as_str().to_owned(),
            app: app.as_str().to_owned(),
        }],
        &RunCompletion {
            success_message,
            redirect: None,
            refresh: RunRefresh::None,
        },
        &[],
    )
    .await?;
    set_flash(
        &session,
        FlashLevel::Success,
        format!("Queued: promote {service} for {app}."),
    );
    Ok(see_other(&links_url))
}

#[derive(Deserialize)]
pub struct UpgradeForm {
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    image_version: Option<String>,
    #[serde(default)]
    custom_env: Option<String>,
    #[serde(default)]
    config_options: Option<String>,
    /// The form checkbox is present only when checked; any submitted value
    /// restarts the linked apps around the upgrade.
    #[serde(default)]
    restart_apps: Option<String>,
}

/// Upgrades the service to a new image version (`<plugin>:upgrade`). An empty
/// form is meaningful: the plugin moves the service to the latest release of
/// its current major version.
pub async fn upgrade(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<(String, String)>,
    form: CsrfForm<UpgradeForm>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    let plugin = plugin_from_slug(&raw_plugin)?;
    let service = service_from_slug(&raw_service)?;
    let form = form.0;

    let fail = |message: String| {
        if is_htmx(&req) {
            modal_error(message)
        } else {
            set_flash(&session, FlashLevel::Error, message);
            Ok(see_other(&detail_url(plugin, &service)))
        }
    };

    let options = match ServiceCreateOptions::parse(
        form.image.as_deref(),
        form.image_version.as_deref(),
        form.custom_env.as_deref(),
        form.config_options.as_deref(),
    ) {
        Ok(options) => options,
        Err(err) => return fail(err.to_string()),
    };
    let restart_apps = form.restart_apps.is_some();
    let redactions = options.redaction_fragments();
    let success_message = format!("Service '{service}' upgraded.");
    let refresh_url = format!("/services/{plugin}/{service}/partials/overview");

    if is_htmx(&req) {
        return start_action_run(
            &state,
            &session,
            &RunRequest {
                subject: service.as_str().to_owned(),
                operation: "service.upgrade".to_owned(),
                target_kind: TargetKind::Service,
                title: format!("Upgrading {service}…"),
                plan: vec![JobSpec::ServiceUpgrade {
                    plugin: plugin.as_str().to_owned(),
                    service: service.as_str().to_owned(),
                    options,
                    restart_apps,
                }],
                completion: RunCompletion {
                    success_message,
                    redirect: None,
                    refresh: RunRefresh::None,
                },
                redactions,
                refresh_url: Some(refresh_url),
            },
        )
        .await;
    }

    let _ = enqueue_action_run(
        &state,
        &session,
        service.as_str(),
        "service.upgrade",
        TargetKind::Service,
        &[JobSpec::ServiceUpgrade {
            plugin: plugin.as_str().to_owned(),
            service: service.as_str().to_owned(),
            options,
            restart_apps,
        }],
        &RunCompletion {
            success_message,
            redirect: None,
            refresh: RunRefresh::None,
        },
        &redactions,
    )
    .await?;
    set_flash(
        &session,
        FlashLevel::Success,
        format!("Queued: upgrade {service}."),
    );
    Ok(see_other(&detail_url(plugin, &service)))
}

#[derive(Deserialize)]
pub struct CloneForm {
    name: String,
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    image_version: Option<String>,
    #[serde(default)]
    custom_env: Option<String>,
    #[serde(default)]
    config_options: Option<String>,
}

/// Clones the service into a new one (`<plugin>:clone`), copying its data.
/// Succeeds by redirecting to the new service's page.
pub async fn clone(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<(String, String)>,
    form: CsrfForm<CloneForm>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    let plugin = plugin_from_slug(&raw_plugin)?;
    let service = service_from_slug(&raw_service)?;
    let form = form.0;
    let name = form.name.trim().to_owned();

    let fail = |message: String| {
        if is_htmx(&req) {
            modal_error(message)
        } else {
            set_flash(&session, FlashLevel::Error, message);
            Ok(see_other(&detail_url(plugin, &service)))
        }
    };

    let new_service = match ServiceName::try_from(name.as_str()) {
        Ok(new_service) => new_service,
        Err(err) => return fail(format!("Invalid service name: {err}")),
    };
    if new_service == service {
        return fail("Choose a name that differs from the source service.".to_owned());
    }

    let options = match ServiceCreateOptions::parse(
        form.image.as_deref(),
        form.image_version.as_deref(),
        form.custom_env.as_deref(),
        form.config_options.as_deref(),
    ) {
        Ok(options) => options,
        Err(err) => return fail(err.to_string()),
    };
    let redactions = options.redaction_fragments();
    let redirect_to = detail_url(plugin, &new_service);
    let success_message = format!("Service '{service}' cloned to '{new_service}'.");

    if is_htmx(&req) {
        return start_action_run(
            &state,
            &session,
            &RunRequest {
                subject: service.as_str().to_owned(),
                operation: "service.clone".to_owned(),
                target_kind: TargetKind::Service,
                title: format!("Cloning {service} to {new_service}…"),
                plan: vec![JobSpec::ServiceClone {
                    plugin: plugin.as_str().to_owned(),
                    service: service.as_str().to_owned(),
                    new_service: new_service.as_str().to_owned(),
                    options,
                }],
                completion: RunCompletion {
                    success_message,
                    redirect: Some(redirect_to.clone()),
                    refresh: RunRefresh::None,
                },
                redactions,
                refresh_url: None,
            },
        )
        .await;
    }

    let source_detail = detail_url(plugin, &service);
    // Attributed to the source service on both paths; the subject is copied
    // out before `service` moves into the command.
    let source_subject = service.as_str().to_owned();
    let command = DokkuCommand::ServiceClone {
        plugin,
        service,
        new_service,
        options,
    };
    match run_synchronously(
        &state,
        &session,
        &source_subject,
        "service.clone",
        TargetKind::Service,
        command,
        &success_message,
        &redactions,
    )
    .await
    {
        Ok(_) => {
            set_flash(&session, FlashLevel::Success, success_message);
            Ok(see_other(&redirect_to))
        }
        Err(err) => {
            set_flash(
                &session,
                FlashLevel::Error,
                format!("Failed to clone service: {err}"),
            );
            Ok(see_other(&source_detail))
        }
    }
}
