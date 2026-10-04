use std::collections::HashMap;

use actix_session::Session;
use actix_web::{HttpResponse, web};
use askama::Template;
use serde::Deserialize;
use time::OffsetDateTime;

use crate::dokku::{
    ContainerRow, ProcessRow, SnapshotError, app_config, app_containers, app_formation, app_logs,
    app_resources, app_service_links, container_rows, format_age, formation_rows,
    overview_from_snapshot, parse_scale_form, service_info,
};
use crate::domain::AppName;
use crate::domain::command::DokkuCommand;
use crate::domain::parse::{LOG_LINES_MAX, LOG_LINES_MIN, clamp_log_lines};
use crate::domain::types::{EnvVar, ResourceReport, ServiceInfo};
use crate::error::AppError;
use crate::storage::users::{SqliteUsersRepo, UsersRepo};
use crate::web::auth_middleware::SESSION_USER_ID;
use crate::web::csrf_form::{CsrfForm, ensure_csrf};
use crate::web::flash::{FlashLevel, FlashMessage, set_flash, take_flash};
use crate::web::render::{render, see_other};
use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "apps/new.html")]
struct NewFormPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
}

#[derive(Deserialize)]
pub struct CreateForm {
    name: String,
}

#[derive(Template)]
#[template(path = "apps/show.html")]
struct ShowPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    name: &'a str,
    active_tab: &'static str,
}

#[derive(Template)]
#[template(path = "apps/config.html")]
struct ConfigPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    name: &'a str,
    active_tab: &'static str,
}

#[derive(Template)]
#[template(path = "apps/delete_confirm.html")]
struct DeleteConfirmPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    name: &'a str,
}

#[derive(Template)]
#[template(path = "apps/logs.html")]
struct LogsPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    name: &'a str,
    active_tab: &'static str,
    line_count: u32,
    min_lines: u32,
    max_lines: u32,
}

#[derive(Template)]
#[template(path = "apps/processes.html")]
struct ProcessesPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    name: &'a str,
    active_tab: &'static str,
}

#[derive(Template)]
#[template(path = "apps/services.html")]
struct ServicesPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    name: &'a str,
    active_tab: &'static str,
}

#[derive(Template)]
#[template(path = "apps/partials/overview.html")]
struct OverviewPartial<'a> {
    health_label: &'static str,
    health_css: &'static str,
    process_label: String,
    deployed: bool,
    created_at: &'a str,
    locked_label: &'static str,
    image_status_label: &'static str,
    last_build_label: String,
    link_exists_label: &'static str,
    domains_label: String,
    dns_record_exists_label: &'static str,
    updated: String,
}

#[derive(Template)]
#[template(path = "apps/partials/processes.html")]
struct ProcessesPartial<'a> {
    name: &'a str,
    csrf_token: &'a str,
    rows: Vec<ProcessRow>,
    containers: Vec<ContainerRow>,
    resources: Vec<ResourceReport>,
    scale_note: Option<String>,
    updated: String,
}

#[derive(Template)]
#[template(path = "apps/partials/services.html")]
struct ServicesPartial<'a> {
    name: &'a str,
    services: Vec<ServiceInfo>,
    links_unknown: bool,
    updated: String,
}

#[derive(Template)]
#[template(path = "apps/partials/config.html")]
struct ConfigPartial {
    vars: Vec<EnvVar>,
}

#[derive(Template)]
#[template(path = "apps/partials/logs.html")]
struct LogsPartial {
    lines: Vec<String>,
}

#[derive(Template)]
#[template(path = "apps/partials/error.html")]
struct ErrorPartial<'a> {
    message: &'a str,
    retry_url: String,
}

fn partial_url(name: &str, tab: &str, query: Option<&str>) -> String {
    match query {
        Some(query) => format!("/apps/{name}/partials/{tab}?{query}"),
        None => format!("/apps/{name}/partials/{tab}"),
    }
}

/// Renders the small retry card that htmx swaps in when a partial fetch fails.
/// Always 200: htmx does not swap 4xx/5xx responses, so an error status would
/// leave the loading skeleton spinning forever.
fn error_fragment(retry_url: &str, message: &str) -> Result<HttpResponse, AppError> {
    let page = ErrorPartial {
        message,
        retry_url: retry_url.to_owned(),
    };
    render(&page)
}

fn not_found_fragment(name: &str, retry_url: &str) -> Result<HttpResponse, AppError> {
    error_fragment(
        retry_url,
        &format!("App '{name}' was not found — it may have been deleted."),
    )
}

/// Maps a snapshot miss / SSH failure from `resolve_app` onto the retry card.
fn fragment_for_resolve_error(
    name: &str,
    retry_url: &str,
    err: SnapshotError,
) -> Result<HttpResponse, AppError> {
    if err.is_not_found() {
        not_found_fragment(name, retry_url)
    } else {
        error_fragment(retry_url, &err.to_string())
    }
}

async fn current_user(
    state: &AppState,
    session: &Session,
) -> Result<crate::storage::users::User, AppError> {
    let user_id = session
        .get::<i64>(SESSION_USER_ID)
        .map_err(|err| AppError::Internal(err.to_string()))?
        .ok_or_else(|| AppError::Internal("session has no user id".into()))?;
    let repo = SqliteUsersRepo::new(state.db.clone());
    repo.find_by_id(user_id)
        .await?
        .ok_or_else(|| AppError::Internal("session user no longer exists".into()))
}

pub async fn new_form(
    state: web::Data<AppState>,
    session: Session,
) -> Result<HttpResponse, AppError> {
    let user = current_user(&state, &session).await?;
    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);

    let page = NewFormPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
    };
    render(&page)
}

pub async fn create(
    state: web::Data<AppState>,
    session: Session,
    form: CsrfForm<CreateForm>,
) -> Result<HttpResponse, AppError> {
    let name = form.0.name.trim().to_owned();

    let app = match AppName::try_from(name.clone()) {
        Ok(app) => app,
        Err(err) => {
            set_flash(
                &session,
                FlashLevel::Error,
                format!("Invalid app name: {err}"),
            );
            return Ok(see_other("/apps/new"));
        }
    };

    match state.dokku.exec(&DokkuCommand::AppsCreate { app }).await {
        Ok(_) => {
            if let Err(err) = state.snapshot.refresh_app_reports(&name).await {
                tracing::warn!(error = %err, "snapshot refresh after create failed");
            }
            set_flash(
                &session,
                FlashLevel::Success,
                format!("App '{}' created.", name),
            );
            Ok(see_other(&format!("/apps/{}", name)))
        }
        Err(err) => {
            set_flash(
                &session,
                FlashLevel::Error,
                format!("Failed to create app: {err}"),
            );
            Ok(see_other("/apps/new"))
        }
    }
}

pub async fn show(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let user = current_user(&state, &session).await?;
    state.snapshot.resolve_app(&name).await?;

    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);
    let page = ShowPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        name: &name,
        active_tab: "overview",
    };
    render(&page)
}

pub async fn overview_partial(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    current_user(&state, &session).await?;

    let retry_url = partial_url(&name, "overview", None);
    if let Err(err) = state.snapshot.refresh_app(&name).await {
        return error_fragment(&retry_url, &err.to_string());
    }
    let snapshot = state.snapshot.ensure_loaded().await?;
    let overview = match overview_from_snapshot(&snapshot, &name) {
        Ok(overview) => overview,
        // The app vanished between the shell render and this fragment (e.g.
        // deleted via the CLI); `refresh_app` already removed it above.
        Err(_) => return not_found_fragment(&name, &retry_url),
    };

    let process_label = match overview.process_count {
        -1 => "—".to_owned(),
        count => count.to_string(),
    };
    let app_info = overview.app_info.as_ref();
    let page = OverviewPartial {
        health_label: overview.health.label(),
        health_css: overview.health.badge_css(),
        process_label,
        deployed: overview
            .ps_report
            .as_ref()
            .map(|r| r.deployed)
            .unwrap_or(false),
        created_at: app_info.map(|a| a.created_at.as_str()).unwrap_or("unknown"),
        locked_label: app_info.map(|a| a.locked_label()).unwrap_or("unknown"),
        image_status_label: app_info
            .map(|a| a.image_status_label())
            .unwrap_or("unknown"),
        last_build_label: app_info
            .map(|a| a.last_build_label())
            .unwrap_or_else(|| "unknown".to_owned()),
        link_exists_label: app_info.map(|a| a.link_exists_label()).unwrap_or("unknown"),
        domains_label: app_info
            .map(|a| a.domains_label())
            .unwrap_or_else(|| "—".to_owned()),
        dns_record_exists_label: app_info
            .map(|a| a.dns_record_exists_label())
            .unwrap_or("unknown"),
        updated: format_age(snapshot.age()),
    };
    render(&page)
}

pub async fn config(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let user = current_user(&state, &session).await?;
    state.snapshot.resolve_app(&name).await?;

    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);
    let page = ConfigPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        name: &name,
        active_tab: "config",
    };
    render(&page)
}

pub async fn config_partial(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    current_user(&state, &session).await?;

    let retry_url = partial_url(&name, "config", None);
    let (_snapshot, app) = match state.snapshot.resolve_app(&name).await {
        Ok(resolved) => resolved,
        Err(err) => return fragment_for_resolve_error(&name, &retry_url, err),
    };
    let vars = match app_config(&*state.dokku, app).await {
        Ok(vars) => vars,
        Err(err) => return error_fragment(&retry_url, &err.to_string()),
    };
    render(&ConfigPartial { vars })
}

pub async fn logs(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
    query: web::Query<HashMap<String, String>>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let user = current_user(&state, &session).await?;
    state.snapshot.resolve_app(&name).await?;

    let num_lines = clamp_log_lines(query.get("lines").map(String::as_str));
    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);
    let page = LogsPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        name: &name,
        active_tab: "logs",
        line_count: num_lines,
        min_lines: LOG_LINES_MIN,
        max_lines: LOG_LINES_MAX,
    };
    render(&page)
}

pub async fn logs_partial(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
    query: web::Query<HashMap<String, String>>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    current_user(&state, &session).await?;

    let num_lines = clamp_log_lines(query.get("lines").map(String::as_str));
    let retry_url = partial_url(&name, "logs", Some(&format!("lines={num_lines}")));
    let (_snapshot, app) = match state.snapshot.resolve_app(&name).await {
        Ok(resolved) => resolved,
        Err(err) => return fragment_for_resolve_error(&name, &retry_url, err),
    };
    let log_lines = match app_logs(&*state.dokku, app, num_lines).await {
        Ok(log_lines) => log_lines,
        Err(err) => return error_fragment(&retry_url, &err.to_string()),
    };
    render(&LogsPartial {
        lines: log_lines.as_slice().to_vec(),
    })
}

pub async fn processes(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let user = current_user(&state, &session).await?;
    state.snapshot.resolve_app(&name).await?;

    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);
    let page = ProcessesPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        name: &name,
        active_tab: "processes",
    };
    render(&page)
}

pub async fn processes_partial(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    current_user(&state, &session).await?;

    let retry_url = partial_url(&name, "processes", None);
    let (snapshot, app) = match state.snapshot.resolve_app(&name).await {
        Ok(resolved) => resolved,
        Err(err) => return fragment_for_resolve_error(&name, &retry_url, err),
    };
    let ps_report = snapshot.ps_report(&name);

    // Detail commands are best-effort: a failure degrades to an empty section
    // rather than taking the whole panel down.
    let formation = app_formation(&*state.dokku, app.clone())
        .await
        .unwrap_or_default();
    let containers = app_containers(&*state.dokku, app.clone())
        .await
        .unwrap_or_default();
    let resources = app_resources(&*state.dokku, app).await.unwrap_or_default();

    let rows = formation_rows(&formation, ps_report);
    let containers = container_rows(&containers, OffsetDateTime::now_utc());
    let scale_note = scale_note(ps_report, &formation);

    let csrf_token = ensure_csrf(&session).await?;
    let page = ProcessesPartial {
        name: &name,
        csrf_token: &csrf_token,
        rows,
        containers,
        resources,
        scale_note,
        updated: format_age(snapshot.age()),
    };
    render(&page)
}

fn scale_note(
    ps_report: Option<&crate::domain::types::PsReport>,
    formation: &[crate::domain::types::ScaleEntry],
) -> Option<String> {
    if formation.is_empty() {
        return Some(
            "No formation found yet — deploy the app before scaling its processes.".to_owned(),
        );
    }
    match ps_report.and_then(|report| report.can_scale) {
        Some(false) => Some(
            "Scaling is managed by the app's app.json formation and cannot be changed here."
                .to_owned(),
        ),
        _ => None,
    }
}

pub async fn scale(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
    form: CsrfForm<std::collections::HashMap<String, String>>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let (_snapshot, app) = state.snapshot.resolve_app(&name).await?;
    let redirect_to = format!("/apps/{name}/processes");

    let formation = match app_formation(&*state.dokku, app.clone()).await {
        Ok(formation) => formation,
        Err(err) => {
            set_flash(
                &session,
                FlashLevel::Error,
                format!("Failed to read current scale: {err}"),
            );
            return Ok(see_other(&redirect_to));
        }
    };

    let entries = match parse_scale_form(&formation, &form.0) {
        Ok(entries) => entries,
        Err(err) => {
            set_flash(&session, FlashLevel::Error, err.to_string());
            return Ok(see_other(&redirect_to));
        }
    };

    match state
        .dokku
        .exec(&DokkuCommand::PsScaleSet {
            app: app.clone(),
            scales: entries,
        })
        .await
    {
        Ok(_) => {
            if let Err(err) = state.snapshot.refresh_app_reports(app.as_str()).await {
                tracing::warn!(error = %err, "snapshot refresh after scale failed");
            }
            set_flash(&session, FlashLevel::Success, format!("Scaled '{}'.", name));
        }
        Err(err) => {
            set_flash(
                &session,
                FlashLevel::Error,
                format!("Failed to scale app: {err}"),
            );
        }
    }
    Ok(see_other(&redirect_to))
}

pub async fn services(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let user = current_user(&state, &session).await?;
    state.snapshot.resolve_app(&name).await?;

    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);
    let page = ServicesPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        name: &name,
        active_tab: "services",
    };
    render(&page)
}

pub async fn services_partial(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    current_user(&state, &session).await?;

    let retry_url = partial_url(&name, "services", None);
    let (snapshot, app) = match state.snapshot.resolve_app(&name).await {
        Ok(resolved) => resolved,
        Err(err) => return fragment_for_resolve_error(&name, &retry_url, err),
    };

    // Links are fetched live so the services tab never depends on background data.
    let links = app_service_links(&*state.dokku, app).await;
    let links_unknown = links.is_none();
    let links = links.unwrap_or_default();

    let mut services = Vec::with_capacity(links.len());
    for link in &links {
        let info = match service_info(&*state.dokku, &link.plugin, &link.service).await {
            Ok(Some(info)) => info,
            _ => ServiceInfo::unknown(link.plugin.clone(), link.service.clone()),
        };
        services.push(info);
    }

    let page = ServicesPartial {
        name: &name,
        services,
        links_unknown,
        updated: format_age(snapshot.age()),
    };
    render(&page)
}

pub async fn delete_confirm(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let user = current_user(&state, &session).await?;
    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);

    let page = DeleteConfirmPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        name: &name,
    };

    render(&page)
}

pub async fn destroy(
    state: web::Data<AppState>,
    session: Session,
    form: CsrfForm<DestroyForm>,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();

    let app = match AppName::try_from(name.clone()) {
        Ok(app) => app,
        Err(err) => {
            set_flash(
                &session,
                FlashLevel::Error,
                format!("Invalid app name: {err}"),
            );
            return Ok(see_other("/"));
        }
    };

    if form.0.name.trim() != name {
        set_flash(
            &session,
            FlashLevel::Error,
            format!("Type `{name}` to confirm deletion."),
        );
        return Ok(see_other(&format!("/apps/{}/delete", name)));
    }

    match state
        .dokku
        .exec(&DokkuCommand::AppsDestroy { app, force: true })
        .await
    {
        Ok(_) => {
            if let Err(err) = state.snapshot.refresh().await {
                tracing::warn!(error = %err, "snapshot refresh after destroy failed");
            }
            set_flash(
                &session,
                FlashLevel::Success,
                format!("App '{}' destroyed.", name),
            );
            Ok(see_other("/"))
        }
        Err(err) => {
            set_flash(
                &session,
                FlashLevel::Error,
                format!("Failed to destroy app: {err}"),
            );
            Ok(see_other(&format!("/apps/{}/delete", name)))
        }
    }
}

#[derive(Deserialize)]
pub struct DestroyForm {
    name: String,
}

#[derive(Deserialize)]
pub struct ActionForm {}

#[derive(Clone, Copy)]
enum AppAction {
    Start,
    Stop,
    Restart,
    Rebuild,
}

impl AppAction {
    fn verb(self) -> &'static str {
        match self {
            AppAction::Start => "start",
            AppAction::Stop => "stop",
            AppAction::Restart => "restart",
            AppAction::Rebuild => "rebuild",
        }
    }

    fn past_tense(self) -> &'static str {
        match self {
            AppAction::Start => "started",
            AppAction::Stop => "stopped",
            AppAction::Restart => "restarted",
            AppAction::Rebuild => "rebuilt",
        }
    }

    fn command(self, app: AppName) -> DokkuCommand {
        match self {
            AppAction::Start => DokkuCommand::PsStart { app },
            AppAction::Stop => DokkuCommand::PsStop { app },
            AppAction::Restart => DokkuCommand::PsRestart { app },
            AppAction::Rebuild => DokkuCommand::PsRebuild { app },
        }
    }
}

pub async fn start(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
    _form: CsrfForm<ActionForm>,
) -> Result<HttpResponse, AppError> {
    process_action(&state, &session, path.into_inner(), AppAction::Start).await
}

pub async fn stop(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
    _form: CsrfForm<ActionForm>,
) -> Result<HttpResponse, AppError> {
    process_action(&state, &session, path.into_inner(), AppAction::Stop).await
}

pub async fn restart(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
    _form: CsrfForm<ActionForm>,
) -> Result<HttpResponse, AppError> {
    process_action(&state, &session, path.into_inner(), AppAction::Restart).await
}

pub async fn rebuild(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
    _form: CsrfForm<ActionForm>,
) -> Result<HttpResponse, AppError> {
    process_action(&state, &session, path.into_inner(), AppAction::Rebuild).await
}

async fn process_action(
    state: &AppState,
    session: &Session,
    name: String,
    action: AppAction,
) -> Result<HttpResponse, AppError> {
    let app = match AppName::try_from(name.clone()) {
        Ok(app) => app,
        Err(err) => {
            set_flash(
                session,
                FlashLevel::Error,
                format!("Invalid app name: {err}"),
            );
            return Ok(see_other("/"));
        }
    };

    match state.dokku.exec(&action.command(app.clone())).await {
        Ok(_) => {
            if let Err(err) = state.snapshot.refresh_app_reports(app.as_str()).await {
                tracing::warn!(error = %err, "snapshot refresh after action failed");
            }
            set_flash(
                session,
                FlashLevel::Success,
                format!("App '{name}' {}.", action.past_tense()),
            );
            Ok(see_other(&format!("/apps/{name}")))
        }
        Err(err) => {
            set_flash(
                session,
                FlashLevel::Error,
                format!("Failed to {} app: {err}", action.verb()),
            );
            Ok(see_other(&format!("/apps/{name}")))
        }
    }
}
