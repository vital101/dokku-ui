use std::collections::HashMap;

use actix_session::Session;
use actix_web::{HttpRequest, HttpResponse, web};
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
use crate::domain::env_file::{config_diff, parse_env_file};
use crate::domain::job::{AppAction as JobAppAction, JobSpec};
use crate::domain::parse::{LOG_LINES_MAX, LOG_LINES_MIN, clamp_log_lines};
use crate::domain::resource::{is_valid_process_type, is_valid_resource_value};
use crate::domain::types::{EnvVar, ResourceReport, ServiceInfo};
use crate::error::AppError;
use crate::storage::runs::TargetKind;
use crate::web::csrf_form::{CsrfForm, ensure_csrf};
use crate::web::flash::{FlashLevel, FlashMessage, set_flash, take_flash};
use crate::web::fragments::{
    RunCompletion, RunRefresh, RunRequest, current_user, enqueue_action_run, error_fragment,
    is_htmx, modal_error, run_synchronously, start_action_run,
};
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
    live_tail_label: String,
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
#[template(path = "apps/settings.html")]
struct SettingsPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    name: &'a str,
    active_tab: &'static str,
}

#[derive(Template)]
#[template(path = "apps/partials/settings.html")]
struct SettingsPartial<'a> {
    name: &'a str,
    csrf_token: &'a str,
    /// `None` when the app's report is unavailable (unknown, not unlocked).
    locked: Option<bool>,
    maintenance_available: bool,
    maintenance_label: String,
    http_auth_available: bool,
    http_auth_label: String,
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

/// One editable resource row per formation process type (unset values are
/// empty strings, like the report).
struct ResourceRow {
    process_type: String,
    limit_cpu: String,
    limit_memory: String,
    limit_memory_swap: String,
    reserve_cpu: String,
    reserve_memory: String,
}

#[derive(Template)]
#[template(path = "apps/partials/processes.html")]
struct ProcessesPartial<'a> {
    name: &'a str,
    csrf_token: &'a str,
    rows: Vec<ProcessRow>,
    containers: Vec<ContainerRow>,
    resource_rows: Vec<ResourceRow>,
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
struct ConfigPartial<'a> {
    vars: Vec<EnvVar>,
    name: &'a str,
    csrf_token: &'a str,
}

#[derive(Template)]
#[template(path = "apps/partials/logs.html")]
struct LogsPartial {
    lines: Vec<String>,
}

#[derive(Template)]
#[template(path = "apps/partials/delete_confirm_modal.html")]
struct DeleteConfirmModalPartial<'a> {
    name: &'a str,
    csrf_token: &'a str,
}

fn partial_url(name: &str, tab: &str, query: Option<&str>) -> String {
    match query {
        Some(query) => format!("/apps/{name}/partials/{tab}?{query}"),
        None => format!("/apps/{name}/partials/{tab}"),
    }
}

/// Renders the small retry card that htmx swaps in when a partial fetch fails
/// (see [`crate::web::fragments::error_fragment`]).
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

    match run_synchronously(
        &state,
        &session,
        &name,
        "app.create",
        TargetKind::App,
        DokkuCommand::AppsCreate { app },
        &format!("App '{name}' created."),
        &[],
    )
    .await
    {
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
    let csrf_token = ensure_csrf(&session).await?;
    render(&ConfigPartial {
        vars,
        name: &name,
        csrf_token: &csrf_token,
    })
}

/// Reveals the config values under re-auth. Without a valid re-auth window the
/// user is sent through `/reauth` first; with one, the unmasked table is
/// returned (marked no-store so it never lingers in a cache).
pub async fn config_reveal(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
    _form: CsrfForm<ActionForm>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    current_user(&state, &session).await?;
    if !reauth_valid_here(&state, &session).await {
        return Ok(see_other(&format!("/reauth?next=/apps/{name}/config")));
    }
    let retry_url = partial_url(&name, "config", None);
    let (_snapshot, app) = match state.snapshot.resolve_app(&name).await {
        Ok(resolved) => resolved,
        Err(err) => return fragment_for_resolve_error(&name, &retry_url, err),
    };
    let vars = match app_config(&*state.dokku, app).await {
        Ok(vars) => vars,
        Err(err) => return error_fragment(&retry_url, &err.to_string()),
    };
    record_reveal(&state, &session, &name).await;
    let page = ConfigRevealedPartial { vars };
    let mut response = render(&page)?;
    response.headers_mut().insert(
        actix_web::http::header::CACHE_CONTROL,
        actix_web::http::header::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}

#[derive(Template)]
#[template(path = "apps/partials/config_revealed.html")]
struct ConfigRevealedPartial {
    vars: Vec<EnvVar>,
}

/// Records a `config.reveal` audit entry (best-effort, no lines).
async fn record_reveal(state: &AppState, session: &Session, name: &str) {
    let actor = crate::web::fragments::current_actor(state, session).await;
    if let Ok(run_id) = state
        .action_runs
        .insert_with(&crate::storage::runs::NewRun {
            subject: name.to_owned(),
            operation: "config.reveal".to_owned(),
            target_kind: crate::storage::runs::TargetKind::App,
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
                    message: "Config values revealed.".to_owned(),
                    redirect: None,
                },
            )
            .await;
    }
}

#[derive(Deserialize)]
pub struct ConfigForm {
    env: String,
}

#[derive(Template)]
#[template(path = "apps/config_edit.html")]
struct ConfigEditPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    name: &'a str,
    active_tab: &'static str,
    env: String,
}

/// The re-auth-gated edit page: shows the current values (revealed) in a
/// `.env`-style textarea for batch set/unset.
pub async fn config_edit(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let user = current_user(&state, &session).await?;
    if !reauth_valid_here(&state, &session).await {
        return Ok(see_other(&format!("/reauth?next=/apps/{name}/config/edit")));
    }
    state.snapshot.resolve_app(&name).await?;

    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);
    let (_snapshot, app) = state.snapshot.resolve_app(&name).await?;
    let vars = app_config(&*state.dokku, app).await.unwrap_or_default();
    let env = vars
        .into_iter()
        .map(|var| format!("{}={}", var.key, var.value))
        .collect::<Vec<_>>()
        .join("\n");
    let page = ConfigEditPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        name: &name,
        active_tab: "config",
        env,
    };
    let mut response = render(&page)?;
    response.headers_mut().insert(
        actix_web::http::header::CACHE_CONTROL,
        actix_web::http::header::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}

/// Applies a `.env`-style edit as a queued job: set + unset in one run, then
/// the app restarts the way `config:set` does on this dokku generation.
pub async fn config_update(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<ConfigForm>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    current_user(&state, &session).await?;
    if !reauth_valid_here(&state, &session).await {
        return Ok(see_other(&format!("/reauth?next=/apps/{name}/config/edit")));
    }

    let (_snapshot, app) = match state.snapshot.resolve_app(&name).await {
        Ok(resolved) => resolved,
        Err(err) => {
            if is_htmx(&req) {
                return modal_error(err.to_string());
            }
            return Err(err.into());
        }
    };
    let current = match app_config(&*state.dokku, app).await {
        Ok(vars) => vars,
        Err(err) => {
            if is_htmx(&req) {
                return modal_error(format!("Failed to read current config: {err}"));
            }
            set_flash(
                &session,
                FlashLevel::Error,
                format!("Failed to read current config: {err}"),
            );
            return Ok(see_other(&format!("/apps/{name}/config")));
        }
    };

    let (desired, skipped) = parse_env_file(&form.0.env);
    if skipped > 0 {
        let message = format!("{skipped} line(s) could not be parsed and were skipped.");
        if is_htmx(&req) {
            return modal_error(message);
        }
        set_flash(&session, FlashLevel::Error, message);
        return Ok(see_other(&format!("/apps/{name}/config/edit")));
    }
    let (to_set, to_unset) = config_diff(&current, &desired);
    if to_set.is_empty() && to_unset.is_empty() {
        let message = "No changes to apply.".to_owned();
        if is_htmx(&req) {
            return modal_error(message);
        }
        set_flash(&session, FlashLevel::Success, message);
        return Ok(see_other(&format!("/apps/{name}/config")));
    }

    let redactions: Vec<String> = to_set.iter().map(|var| var.value.clone()).collect();
    let mut plan = Vec::new();
    if !to_set.is_empty() {
        plan.push(JobSpec::ConfigSet {
            app: name.clone(),
            vars: to_set,
        });
    }
    if !to_unset.is_empty() {
        plan.push(JobSpec::ConfigUnset {
            app: name.clone(),
            keys: to_unset,
        });
    }

    let completion = RunCompletion {
        success_message: format!("Config updated for '{name}'."),
        redirect: None,
        refresh: RunRefresh::Reports,
    };
    if is_htmx(&req) {
        return start_action_run(
            &state,
            &session,
            &RunRequest {
                subject: name.clone(),
                operation: "config.set".to_owned(),
                target_kind: crate::storage::runs::TargetKind::App,
                title: format!("Updating config for {name}…"),
                plan,
                completion,
                redactions: redactions.clone(),
                refresh_url: Some(format!("/apps/{name}/partials/config")),
            },
        )
        .await;
    }
    let _ = enqueue_action_run(
        &state,
        &session,
        &name,
        "config.set",
        crate::storage::runs::TargetKind::App,
        &plan,
        &completion,
        &redactions,
    )
    .await?;
    set_flash(
        &session,
        FlashLevel::Success,
        format!("Queued: config update for {name}."),
    );
    Ok(see_other(&format!("/apps/{name}/config")))
}

/// The re-auth window is in the session (see `src/auth/reauth.rs`).
async fn reauth_valid_here(_state: &AppState, session: &Session) -> bool {
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    match session.get::<i64>(crate::auth::reauth::REAUTH_UNTIL) {
        Ok(Some(until)) => crate::auth::reauth::reauth_valid(now, until),
        _ => false,
    }
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
    let live_tail_label = live_tail_label(&state).await;
    let page = LogsPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        name: &name,
        active_tab: "logs",
        line_count: num_lines,
        min_lines: LOG_LINES_MIN,
        max_lines: LOG_LINES_MAX,
        live_tail_label,
    };
    render(&page)
}

/// The live-tail badge on the logs page, straight from the shared capability
/// row (zero SSH on this path; a cold start renders "unknown" until the probe
/// publishes).
async fn live_tail_label(state: &AppState) -> String {
    let support = state
        .capabilities
        .current()
        .await
        .map(|caps| caps.supports_family(crate::domain::capabilities::CapabilityFamily::Logs));
    format!(
        "Live tail: {}",
        support.map(|s| s.label()).unwrap_or("unknown".to_owned())
    )
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
    let process = match query.get("process").map(String::as_str).map(str::trim) {
        None | Some("") => None,
        Some(process) => {
            if !is_valid_process_type(process) {
                return Err(AppError::BadRequest("invalid process type".into()));
            }
            Some(process.to_owned())
        }
    };
    let retry_query = match &process {
        Some(process) => format!("lines={num_lines}&process={process}"),
        None => format!("lines={num_lines}"),
    };
    let retry_url = partial_url(&name, "logs", Some(&retry_query));
    let (_snapshot, app) = match state.snapshot.resolve_app(&name).await {
        Ok(resolved) => resolved,
        Err(err) => return fragment_for_resolve_error(&name, &retry_url, err),
    };
    let log_lines = match app_logs(&*state.dokku, app, num_lines, process.as_deref()).await {
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
    let resource_rows = merge_resource_rows(&rows, &resources);

    let csrf_token = ensure_csrf(&session).await?;
    let page = ProcessesPartial {
        name: &name,
        csrf_token: &csrf_token,
        rows,
        containers,
        resource_rows,
        scale_note,
        updated: format_age(snapshot.age()),
    };
    render(&page)
}

/// One editable row per formation process type plus any report-only entries
/// (e.g. dokku's `_default_` type set without `--process-type`), so a value
/// shown by `resource:report` is always visible and clearable.
fn merge_resource_rows(rows: &[ProcessRow], resources: &[ResourceReport]) -> Vec<ResourceRow> {
    let mut merged: Vec<ResourceRow> = rows
        .iter()
        .map(|row| {
            let report = resources
                .iter()
                .find(|report| report.process_type == row.process_type);
            ResourceRow {
                process_type: row.process_type.clone(),
                limit_cpu: report.map(|r| r.limit_cpu.clone()).unwrap_or_default(),
                limit_memory: report.map(|r| r.limit_memory.clone()).unwrap_or_default(),
                limit_memory_swap: report
                    .map(|r| r.limit_memory_swap.clone())
                    .unwrap_or_default(),
                reserve_cpu: report.map(|r| r.reserve_cpu.clone()).unwrap_or_default(),
                reserve_memory: report.map(|r| r.reserve_memory.clone()).unwrap_or_default(),
            }
        })
        .collect();
    for report in resources {
        if merged
            .iter()
            .any(|row| row.process_type == report.process_type)
        {
            continue;
        }
        merged.push(ResourceRow {
            process_type: report.process_type.clone(),
            limit_cpu: report.limit_cpu.clone(),
            limit_memory: report.limit_memory.clone(),
            limit_memory_swap: report.limit_memory_swap.clone(),
            reserve_cpu: report.reserve_cpu.clone(),
            reserve_memory: report.reserve_memory.clone(),
        });
    }
    merged
}

#[derive(Deserialize)]
pub struct ResourceForm {
    process_type: String,
    #[serde(default)]
    limit_cpu: String,
    #[serde(default)]
    limit_memory: String,
    #[serde(default)]
    limit_memory_swap: String,
    #[serde(default)]
    reserve_cpu: String,
    #[serde(default)]
    reserve_memory: String,
    #[serde(default)]
    action: String,
}

/// Sets or clears limits/reservations for one process type. Empty fields are
/// left unchanged; `Clear` removes all limits and reservations for the type
/// (dokku has no per-key clear on this generation).
pub async fn update_resources(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<ResourceForm>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    if let Err(err) = AppName::try_from(name.clone()) {
        if is_htmx(&req) {
            return modal_error(format!("Invalid app name: {err}"));
        }
        set_flash(
            &session,
            FlashLevel::Error,
            format!("Invalid app name: {err}"),
        );
        return Ok(see_other("/"));
    }
    let form = form.0;
    let process_type = form.process_type.trim().to_owned();
    if !is_valid_process_type(&process_type) {
        let message = format!("Invalid process type: {process_type}");
        if is_htmx(&req) {
            return modal_error(message);
        }
        set_flash(&session, FlashLevel::Error, message);
        return Ok(see_other(&format!("/apps/{name}/processes")));
    }
    let opt = |value: &str| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    };
    let (plan, operation, success_message) = if form.action == "clear" {
        // Clear ignores the field values entirely (the form carries the
        // current report back; those values may use units this UI predates).
        (
            vec![
                JobSpec::ResourceLimitClear {
                    app: name.clone(),
                    process_type: process_type.clone(),
                },
                JobSpec::ResourceReserveClear {
                    app: name.clone(),
                    process_type: process_type.clone(),
                },
            ],
            "resource.clear",
            format!("Cleared resources for '{name}' ({process_type})."),
        )
    } else {
        for value in [
            &form.limit_cpu,
            &form.limit_memory,
            &form.limit_memory_swap,
            &form.reserve_cpu,
            &form.reserve_memory,
        ] {
            let value = value.trim();
            if !value.is_empty() && !is_valid_resource_value(value) {
                let message = format!("Invalid resource value: {value}");
                if is_htmx(&req) {
                    return modal_error(message);
                }
                set_flash(&session, FlashLevel::Error, message);
                return Ok(see_other(&format!("/apps/{name}/processes")));
            }
        }
        let limit = JobSpec::ResourceLimit {
            app: name.clone(),
            process_type: process_type.clone(),
            cpu: opt(&form.limit_cpu),
            memory: opt(&form.limit_memory),
            memory_swap: opt(&form.limit_memory_swap),
        };
        let reserve = JobSpec::ResourceReserve {
            app: name.clone(),
            process_type: process_type.clone(),
            cpu: opt(&form.reserve_cpu),
            memory: opt(&form.reserve_memory),
        };
        let limit_empty = matches!(
            &limit,
            JobSpec::ResourceLimit {
                cpu: None,
                memory: None,
                memory_swap: None,
                ..
            }
        );
        let reserve_empty = matches!(
            &reserve,
            JobSpec::ResourceReserve {
                cpu: None,
                memory: None,
                ..
            }
        );
        if limit_empty && reserve_empty {
            let message = "Enter at least one value, or use Clear.".to_owned();
            if is_htmx(&req) {
                return modal_error(message);
            }
            set_flash(&session, FlashLevel::Error, message);
            return Ok(see_other(&format!("/apps/{name}/processes")));
        }
        let mut plan = Vec::new();
        if !limit_empty {
            plan.push(limit);
        }
        if !reserve_empty {
            plan.push(reserve);
        }
        (
            plan,
            "resource.set",
            format!("Resources updated for '{name}' ({process_type})."),
        )
    };

    let completion = RunCompletion {
        success_message,
        redirect: None,
        refresh: RunRefresh::None,
    };
    if is_htmx(&req) {
        return start_action_run(
            &state,
            &session,
            &RunRequest {
                subject: name.clone(),
                operation: operation.to_owned(),
                target_kind: TargetKind::App,
                title: format!("Updating resources for {name}…"),
                plan,
                completion,
                redactions: Vec::new(),
                refresh_url: Some(format!("/apps/{name}/partials/processes")),
            },
        )
        .await;
    }
    let _ = enqueue_action_run(
        &state,
        &session,
        &name,
        operation,
        TargetKind::App,
        &plan,
        &completion,
        &[],
    )
    .await?;
    set_flash(
        &session,
        FlashLevel::Success,
        format!("Queued: update resources for {name}."),
    );
    Ok(see_other(&format!("/apps/{name}/processes")))
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
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<std::collections::HashMap<String, String>>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let (_snapshot, app) = match state.snapshot.resolve_app(&name).await {
        Ok(resolved) => resolved,
        Err(err) => {
            if is_htmx(&req) {
                return modal_error(err.to_string());
            }
            return Err(err.into());
        }
    };
    let redirect_to = format!("/apps/{name}/processes");

    let formation = match app_formation(&*state.dokku, app.clone()).await {
        Ok(formation) => formation,
        Err(err) => {
            if is_htmx(&req) {
                return modal_error(format!("Failed to read current scale: {err}"));
            }
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
            if is_htmx(&req) {
                return modal_error(err.to_string());
            }
            set_flash(&session, FlashLevel::Error, err.to_string());
            return Ok(see_other(&redirect_to));
        }
    };

    if is_htmx(&req) {
        return start_action_run(
            &state,
            &session,
            &RunRequest {
                subject: name.clone(),
                operation: "app.scale".to_owned(),
                target_kind: TargetKind::App,
                title: format!("Scaling {name}…"),
                plan: vec![JobSpec::AppScale {
                    app: name.clone(),
                    scales: entries,
                }],
                completion: RunCompletion {
                    success_message: format!("Scaled '{name}'."),
                    redirect: None,
                    refresh: RunRefresh::Reports,
                },
                redactions: Vec::new(),
                refresh_url: Some(format!("/apps/{name}/partials/processes")),
            },
        )
        .await;
    }

    let _ = enqueue_action_run(
        &state,
        &session,
        &name,
        "app.scale",
        TargetKind::App,
        &[JobSpec::AppScale {
            app: name.clone(),
            scales: entries,
        }],
        &RunCompletion {
            success_message: format!("Scaled '{name}'."),
            redirect: None,
            refresh: RunRefresh::Reports,
        },
        &[],
    )
    .await?;
    set_flash(
        &session,
        FlashLevel::Success,
        format!("Queued: scale {name}."),
    );
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

pub async fn settings(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let user = current_user(&state, &session).await?;
    state.snapshot.resolve_app(&name).await?;

    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);
    let page = SettingsPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        name: &name,
        active_tab: "settings",
    };
    render(&page)
}

pub async fn settings_partial(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    current_user(&state, &session).await?;

    let retry_url = partial_url(&name, "settings", None);
    let (snapshot, _app) = match state.snapshot.resolve_app(&name).await {
        Ok(resolved) => resolved,
        Err(err) => return fragment_for_resolve_error(&name, &retry_url, err),
    };
    let locked = snapshot.app_info(&name).map(|info| info.locked);
    let caps = state.capabilities.current().await;
    let support_of = |plugin: &str| match &caps {
        Some(caps) => caps.supports_plugin(plugin),
        None => crate::domain::capabilities::Support::Unknown,
    };
    let maintenance = support_of("maintenance");
    let http_auth = support_of("http-auth");
    let maintenance_available = maintenance == crate::domain::capabilities::Support::Supported;
    let http_auth_available = http_auth == crate::domain::capabilities::Support::Supported;
    let csrf_token = ensure_csrf(&session).await?;
    render(&SettingsPartial {
        name: &name,
        csrf_token: &csrf_token,
        locked,
        maintenance_available,
        maintenance_label: maintenance.label(),
        http_auth_available,
        http_auth_label: http_auth.label(),
    })
}

/// Deploy lock/unlock. Both are quick config writes (bounded timeout) and
/// stream through the shared run modal like every other mutation.
async fn lock_action(
    state: &AppState,
    session: &Session,
    req: &HttpRequest,
    name: String,
    lock: bool,
) -> Result<HttpResponse, AppError> {
    let verb = if lock { "lock" } else { "unlock" };
    match AppName::try_from(name.clone()) {
        Ok(_) => {}
        Err(err) => {
            if is_htmx(req) {
                return modal_error(format!("Invalid app name: {err}"));
            }
            set_flash(
                session,
                FlashLevel::Error,
                format!("Invalid app name: {err}"),
            );
            return Ok(see_other("/"));
        }
    }
    let plan = vec![if lock {
        JobSpec::AppLock { app: name.clone() }
    } else {
        JobSpec::AppUnlock { app: name.clone() }
    }];
    let completion = RunCompletion {
        success_message: format!("Deploy lock {verb}ed for '{name}'."),
        redirect: None,
        refresh: RunRefresh::Reports,
    };
    if is_htmx(req) {
        return start_action_run(
            state,
            session,
            &RunRequest {
                subject: name.clone(),
                operation: format!("app.{verb}"),
                target_kind: TargetKind::App,
                title: format!("{} {name}…", if lock { "Locking" } else { "Unlocking" }),
                plan,
                completion,
                redactions: Vec::new(),
                refresh_url: Some(format!("/apps/{name}/partials/settings")),
            },
        )
        .await;
    }
    let _ = enqueue_action_run(
        state,
        session,
        &name,
        &format!("app.{verb}"),
        TargetKind::App,
        &plan,
        &completion,
        &[],
    )
    .await?;
    set_flash(
        session,
        FlashLevel::Success,
        format!("Queued: {verb} {name}."),
    );
    Ok(see_other(&format!("/apps/{name}/settings")))
}

pub async fn lock(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    _form: CsrfForm<ActionForm>,
) -> Result<HttpResponse, AppError> {
    lock_action(&state, &session, &req, path.into_inner(), true).await
}

pub async fn unlock(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    _form: CsrfForm<ActionForm>,
) -> Result<HttpResponse, AppError> {
    lock_action(&state, &session, &req, path.into_inner(), false).await
}

#[derive(Deserialize)]
pub struct RenameForm {
    name: String,
}

/// Renames the app. The run redirects to the new app page on completion; the
/// no-JS path queues and stays on the (still-existing) old settings page.
pub async fn rename(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<RenameForm>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    if let Err(err) = AppName::try_from(name.clone()) {
        if is_htmx(&req) {
            return modal_error(format!("Invalid app name: {err}"));
        }
        set_flash(
            &session,
            FlashLevel::Error,
            format!("Invalid app name: {err}"),
        );
        return Ok(see_other("/"));
    }
    let new_name = match AppName::try_from(form.0.name.trim()) {
        Ok(new_name) => new_name,
        Err(err) => {
            let message = format!("Invalid new name: {err}");
            if is_htmx(&req) {
                return modal_error(message);
            }
            set_flash(&session, FlashLevel::Error, message);
            return Ok(see_other(&format!("/apps/{name}/settings")));
        }
    };
    if new_name.as_str() == name {
        let message = "The new name is the same as the current name.".to_owned();
        if is_htmx(&req) {
            return modal_error(message);
        }
        set_flash(&session, FlashLevel::Error, message);
        return Ok(see_other(&format!("/apps/{name}/settings")));
    }

    let plan = vec![JobSpec::AppRename {
        app: name.clone(),
        new_name: new_name.as_str().to_owned(),
    }];
    let completion = RunCompletion {
        success_message: format!("Renamed '{name}' to '{new_name}'."),
        redirect: Some(format!("/apps/{new_name}")),
        refresh: RunRefresh::All,
    };
    if is_htmx(&req) {
        return start_action_run(
            &state,
            &session,
            &RunRequest {
                subject: name.clone(),
                operation: "app.rename".to_owned(),
                target_kind: TargetKind::App,
                title: format!("Renaming {name} to {new_name}…"),
                plan,
                completion,
                redactions: Vec::new(),
                refresh_url: None,
            },
        )
        .await;
    }
    let _ = enqueue_action_run(
        &state,
        &session,
        &name,
        "app.rename",
        TargetKind::App,
        &plan,
        &completion,
        &[],
    )
    .await?;
    set_flash(
        &session,
        FlashLevel::Success,
        format!("Queued: rename {name} to {new_name}."),
    );
    Ok(see_other(&format!("/apps/{name}/settings")))
}

#[derive(Deserialize)]
pub struct HttpAuthUserForm {
    username: String,
    password: String,
}

#[derive(Deserialize)]
pub struct HttpAuthRemoveForm {
    username: String,
}

/// Shared dispatch for the plugin-backed settings actions (maintenance,
/// http-auth): validates the app, refuses unsupported plugins with an
/// explanatory card, and queues the plan (htmx or redirect).
#[allow(clippy::too_many_arguments)]
async fn plugin_run(
    state: &AppState,
    session: &Session,
    req: &HttpRequest,
    name: String,
    plugin: &str,
    plan: Vec<JobSpec>,
    operation: &str,
    title: String,
    success_message: String,
    queued_message: String,
    redactions: Vec<String>,
) -> Result<HttpResponse, AppError> {
    if let Err(err) = AppName::try_from(name.clone()) {
        if is_htmx(req) {
            return modal_error(format!("Invalid app name: {err}"));
        }
        set_flash(
            session,
            FlashLevel::Error,
            format!("Invalid app name: {err}"),
        );
        return Ok(see_other("/"));
    }
    let support = state
        .capabilities
        .current()
        .await
        .map(|caps| caps.supports_plugin(plugin))
        .unwrap_or(crate::domain::capabilities::Support::Unknown);
    if support != crate::domain::capabilities::Support::Supported {
        let message = format!("Unavailable on this host: {}", support.label());
        if is_htmx(req) {
            return modal_error(message);
        }
        set_flash(session, FlashLevel::Error, message);
        return Ok(see_other(&format!("/apps/{name}/settings")));
    }

    let completion = RunCompletion {
        success_message,
        redirect: None,
        refresh: RunRefresh::None,
    };
    if is_htmx(req) {
        return start_action_run(
            state,
            session,
            &RunRequest {
                subject: name.clone(),
                operation: operation.to_owned(),
                target_kind: TargetKind::App,
                title,
                plan,
                completion,
                redactions,
                refresh_url: Some(format!("/apps/{name}/partials/settings")),
            },
        )
        .await;
    }
    let _ = enqueue_action_run(
        state,
        session,
        &name,
        operation,
        TargetKind::App,
        &plan,
        &completion,
        &redactions,
    )
    .await?;
    set_flash(session, FlashLevel::Success, queued_message);
    Ok(see_other(&format!("/apps/{name}/settings")))
}

pub async fn maintenance_enable(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    _form: CsrfForm<ActionForm>,
) -> Result<HttpResponse, AppError> {
    maintenance_toggle(&state, &session, &req, path.into_inner(), true).await
}

pub async fn maintenance_disable(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    _form: CsrfForm<ActionForm>,
) -> Result<HttpResponse, AppError> {
    maintenance_toggle(&state, &session, &req, path.into_inner(), false).await
}

async fn maintenance_toggle(
    state: &AppState,
    session: &Session,
    req: &HttpRequest,
    name: String,
    enabled: bool,
) -> Result<HttpResponse, AppError> {
    let plan = vec![if enabled {
        JobSpec::MaintenanceEnable { app: name.clone() }
    } else {
        JobSpec::MaintenanceDisable { app: name.clone() }
    }];
    let (verb, title) = if enabled {
        ("enable", format!("Enabling maintenance mode for {name}…"))
    } else {
        ("disable", format!("Disabling maintenance mode for {name}…"))
    };
    plugin_run(
        state,
        session,
        req,
        name.clone(),
        "maintenance",
        plan,
        &format!("maintenance.{verb}"),
        title,
        format!("Maintenance mode {verb}d for '{name}'."),
        format!("Queued: {verb} maintenance mode for {name}."),
        Vec::new(),
    )
    .await
}

pub async fn http_auth_enable(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    _form: CsrfForm<ActionForm>,
) -> Result<HttpResponse, AppError> {
    http_auth_toggle(&state, &session, &req, path.into_inner(), true).await
}

pub async fn http_auth_disable(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    _form: CsrfForm<ActionForm>,
) -> Result<HttpResponse, AppError> {
    http_auth_toggle(&state, &session, &req, path.into_inner(), false).await
}

async fn http_auth_toggle(
    state: &AppState,
    session: &Session,
    req: &HttpRequest,
    name: String,
    enabled: bool,
) -> Result<HttpResponse, AppError> {
    let plan = vec![if enabled {
        JobSpec::HttpAuthEnable { app: name.clone() }
    } else {
        JobSpec::HttpAuthDisable { app: name.clone() }
    }];
    let (verb, title) = if enabled {
        ("enable", format!("Enabling basic auth for {name}…"))
    } else {
        ("disable", format!("Disabling basic auth for {name}…"))
    };
    plugin_run(
        state,
        session,
        req,
        name.clone(),
        "http-auth",
        plan,
        &format!("http-auth.{verb}"),
        title,
        format!("Basic auth {verb}d for '{name}'."),
        format!("Queued: {verb} basic auth for {name}."),
        Vec::new(),
    )
    .await
}

pub async fn http_auth_add_user(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<HttpAuthUserForm>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let form = form.0;
    let username = form.username.trim().to_owned();
    if !crate::domain::http_auth::is_valid_username(&username) {
        let message =
            "Username may contain letters, digits, dots, underscores, and hyphens only.".to_owned();
        if is_htmx(&req) {
            return modal_error(message);
        }
        set_flash(&session, FlashLevel::Error, message);
        return Ok(see_other(&format!("/apps/{name}/settings")));
    }
    if !crate::domain::is_valid_config_value(&form.password) {
        let message = "Password must be single-line and must not contain single quotes.".to_owned();
        if is_htmx(&req) {
            return modal_error(message);
        }
        set_flash(&session, FlashLevel::Error, message);
        return Ok(see_other(&format!("/apps/{name}/settings")));
    }
    let plan = vec![JobSpec::HttpAuthAddUser {
        app: name.clone(),
        username: username.clone(),
        password: form.password.clone(),
    }];
    plugin_run(
        &state,
        &session,
        &req,
        name.clone(),
        "http-auth",
        plan,
        "http-auth.add-user",
        format!("Adding basic-auth user {username} to {name}…"),
        format!("Added basic-auth user '{username}' to '{name}'."),
        format!("Queued: add basic-auth user {username} to {name}."),
        vec![form.password.clone()],
    )
    .await
}

pub async fn http_auth_remove_user(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<HttpAuthRemoveForm>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let username = form.0.username.trim().to_owned();
    if !crate::domain::http_auth::is_valid_username(&username) {
        let message = "Invalid username.".to_owned();
        if is_htmx(&req) {
            return modal_error(message);
        }
        set_flash(&session, FlashLevel::Error, message);
        return Ok(see_other(&format!("/apps/{name}/settings")));
    }
    let plan = vec![JobSpec::HttpAuthRemoveUser {
        app: name.clone(),
        username: username.clone(),
    }];
    plugin_run(
        &state,
        &session,
        &req,
        name.clone(),
        "http-auth",
        plan,
        "http-auth.remove-user",
        format!("Removing basic-auth user {username} from {name}…"),
        format!("Removed basic-auth user '{username}' from '{name}'."),
        format!("Queued: remove basic-auth user {username} from {name}."),
        Vec::new(),
    )
    .await
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
    req: HttpRequest,
    form: CsrfForm<DestroyForm>,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();

    match AppName::try_from(name.clone()) {
        Ok(_) => {}
        Err(err) => {
            if is_htmx(&req) {
                return modal_error(format!("Invalid app name: {err}"));
            }
            set_flash(
                &session,
                FlashLevel::Error,
                format!("Invalid app name: {err}"),
            );
            return Ok(see_other("/"));
        }
    };

    if form.0.name.trim() != name {
        if is_htmx(&req) {
            return modal_error(format!("Type `{name}` to confirm deletion."));
        }
        set_flash(
            &session,
            FlashLevel::Error,
            format!("Type `{name}` to confirm deletion."),
        );
        return Ok(see_other(&format!("/apps/{}/delete", name)));
    }

    if is_htmx(&req) {
        return start_action_run(
            &state,
            &session,
            &RunRequest {
                subject: name.clone(),
                operation: "app.destroy".to_owned(),
                target_kind: TargetKind::App,
                title: format!("Deleting {name}…"),
                plan: vec![JobSpec::AppDestroy { app: name.clone() }],
                completion: RunCompletion {
                    success_message: format!("App '{name}' destroyed."),
                    redirect: Some("/".to_owned()),
                    refresh: RunRefresh::All,
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
        &name,
        "app.destroy",
        TargetKind::App,
        &[JobSpec::AppDestroy { app: name.clone() }],
        &RunCompletion {
            success_message: format!("App '{name}' destroyed."),
            redirect: Some("/".to_owned()),
            refresh: RunRefresh::All,
        },
        &[],
    )
    .await?;
    set_flash(
        &session,
        FlashLevel::Success,
        format!("Queued: destroy {name}."),
    );
    Ok(see_other("/"))
}

pub async fn delete_confirm_modal(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    current_user(&state, &session).await?;
    state.snapshot.resolve_app(&name).await?;
    let csrf_token = ensure_csrf(&session).await?;

    render(&DeleteConfirmModalPartial {
        name: &name,
        csrf_token: &csrf_token,
    })
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

    fn present_participle(self) -> &'static str {
        match self {
            AppAction::Start => "Starting",
            AppAction::Stop => "Stopping",
            AppAction::Restart => "Restarting",
            AppAction::Rebuild => "Rebuilding",
        }
    }

    /// The serializable job form of this action (used by the queued run path).
    fn job(self, app: &str) -> JobSpec {
        let action = match self {
            AppAction::Start => JobAppAction::Start,
            AppAction::Stop => JobAppAction::Stop,
            AppAction::Restart => JobAppAction::Restart,
            AppAction::Rebuild => JobAppAction::Rebuild,
        };
        JobSpec::AppAction {
            app: app.to_owned(),
            action,
        }
    }
}

pub async fn start(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    _form: CsrfForm<ActionForm>,
) -> Result<HttpResponse, AppError> {
    process_action(&state, &session, &req, path.into_inner(), AppAction::Start).await
}

pub async fn stop(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    _form: CsrfForm<ActionForm>,
) -> Result<HttpResponse, AppError> {
    process_action(&state, &session, &req, path.into_inner(), AppAction::Stop).await
}

pub async fn restart(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    _form: CsrfForm<ActionForm>,
) -> Result<HttpResponse, AppError> {
    process_action(
        &state,
        &session,
        &req,
        path.into_inner(),
        AppAction::Restart,
    )
    .await
}

pub async fn rebuild(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    _form: CsrfForm<ActionForm>,
) -> Result<HttpResponse, AppError> {
    process_action(
        &state,
        &session,
        &req,
        path.into_inner(),
        AppAction::Rebuild,
    )
    .await
}

async fn process_action(
    state: &AppState,
    session: &Session,
    req: &HttpRequest,
    name: String,
    action: AppAction,
) -> Result<HttpResponse, AppError> {
    match AppName::try_from(name.clone()) {
        Ok(_) => {}
        Err(err) => {
            if is_htmx(req) {
                return modal_error(format!("Invalid app name: {err}"));
            }
            set_flash(
                session,
                FlashLevel::Error,
                format!("Invalid app name: {err}"),
            );
            return Ok(see_other("/"));
        }
    };

    if is_htmx(req) {
        return start_action_run(
            state,
            session,
            &RunRequest {
                subject: name.clone(),
                operation: format!("app.{}", action.verb()),
                target_kind: TargetKind::App,
                title: format!("{} {}…", action.present_participle(), name),
                plan: vec![action.job(&name)],
                completion: RunCompletion {
                    success_message: format!("App '{name}' {}.", action.past_tense()),
                    redirect: None,
                    refresh: RunRefresh::Reports,
                },
                redactions: Vec::new(),
                refresh_url: Some(format!("/apps/{name}/partials/overview")),
            },
        )
        .await;
    }

    let _ = enqueue_action_run(
        state,
        session,
        &name,
        &format!("app.{}", action.verb()),
        TargetKind::App,
        &[action.job(&name)],
        &RunCompletion {
            success_message: format!("App '{name}' {}.", action.past_tense()),
            redirect: None,
            refresh: RunRefresh::Reports,
        },
        &[],
    )
    .await?;
    set_flash(
        session,
        FlashLevel::Success,
        format!("Queued: {} {name}.", action.verb()),
    );
    Ok(see_other(&format!("/apps/{name}")))
}
