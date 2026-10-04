use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use actix_session::Session;
use actix_web::web::Bytes;
use actix_web::{HttpRequest, HttpResponse, web};
use askama::Template;
use futures_util::Stream;
use futures_util::stream::unfold;
use serde::Deserialize;
use time::OffsetDateTime;

use crate::dokku::{
    ActionRun, ContainerRow, ProcessRow, RunOutcome, SnapshotError, app_config, app_containers,
    app_formation, app_logs, app_resources, app_service_links, container_rows, format_age,
    formation_rows, overview_from_snapshot, parse_scale_form, service_info,
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

#[derive(Template)]
#[template(path = "apps/partials/run.html")]
struct RunPartial<'a> {
    name: &'a str,
    run_id: u64,
    title: String,
    refresh_url: Option<String>,
}

#[derive(Template)]
#[template(path = "apps/partials/modal_error.html")]
struct ModalErrorPartial<'a> {
    message: &'a str,
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

enum RunRefresh {
    Reports,
    All,
}

struct RunCompletion {
    success_message: String,
    redirect: Option<String>,
    refresh: RunRefresh,
}

/// Starts `command`, streaming its output into `run` line by line. On completion
/// refreshes the snapshot and records the outcome the SSE stream delivers.
fn spawn_run(
    state: AppState,
    run: Arc<ActionRun>,
    command: DokkuCommand,
    completion: RunCompletion,
) {
    tokio::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);
        let line_run = run.clone();
        let line_task = tokio::spawn(async move {
            let mut partial = String::new();
            while let Some(chunk) = rx.recv().await {
                partial.push_str(&chunk);
                while let Some(pos) = partial.find('\n') {
                    let line: String = partial.drain(..=pos).collect();
                    line_run
                        .append_line(line.trim_end_matches(['\n', '\r']).to_owned())
                        .await;
                }
            }
            let rest = partial.trim_end_matches(['\n', '\r']);
            if !rest.is_empty() {
                line_run.append_line(rest.to_owned()).await;
            }
        });

        let result = state.dokku.exec_streaming(&command, tx).await;
        let _ = line_task.await;

        let outcome = match result {
            Ok(_) => {
                match completion.refresh {
                    RunRefresh::Reports => {
                        if let Err(err) = state.snapshot.refresh_app_reports(&run.app).await {
                            tracing::warn!(error = %err, "snapshot refresh after action failed");
                        }
                    }
                    RunRefresh::All => {
                        if let Err(err) = state.snapshot.refresh().await {
                            tracing::warn!(error = %err, "snapshot refresh after destroy failed");
                        }
                    }
                }
                RunOutcome {
                    ok: true,
                    message: completion.success_message,
                    redirect: completion.redirect,
                }
            }
            Err(err) => RunOutcome {
                ok: false,
                message: err.to_string(),
                redirect: None,
            },
        };
        run.finish(outcome).await;
    });
}

fn is_htmx(req: &HttpRequest) -> bool {
    req.headers().contains_key("HX-Request")
}

fn modal_error(message: impl Into<String>) -> Result<HttpResponse, AppError> {
    let message = message.into();
    let page = ModalErrorPartial { message: &message };
    render(&page)
}

/// Registers a run, spawns the command, and returns the streaming modal fragment.
async fn start_action_run(
    state: &AppState,
    name: &str,
    title: String,
    command: DokkuCommand,
    completion: RunCompletion,
    refresh_url: Option<String>,
) -> Result<HttpResponse, AppError> {
    let run = state.action_runs.insert(name).await;
    spawn_run(state.clone(), run.clone(), command, completion);
    render(&RunPartial {
        name,
        run_id: run.id,
        title,
        refresh_url,
    })
}

const SSE_KEEPALIVE: Duration = Duration::from_secs(10);

/// Replays buffered output, then follows the run until it finishes. Emits
/// `line` events for output and one `done` event carrying the JSON outcome.
fn action_stream(run: Arc<ActionRun>) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    let follower = run.subscribe();
    unfold(
        (run, 0usize, follower, false),
        |(run, mut cursor, mut follower, done_sent)| async move {
            if done_sent {
                return None;
            }
            loop {
                let (lines, outcome) = run.poll(&mut cursor).await;
                if !lines.is_empty() {
                    return Some((Ok(line_events(&lines)), (run, cursor, follower, false)));
                }
                if let Some(outcome) = outcome {
                    return Some((Ok(done_event(&outcome)), (run, cursor, follower, true)));
                }
                match tokio::time::timeout(SSE_KEEPALIVE, follower.changed()).await {
                    Ok(Ok(())) => continue,
                    Ok(Err(_)) => return None,
                    Err(_) => {
                        return Some((
                            Ok(Bytes::from_static(b": keepalive\n\n")),
                            (run, cursor, follower, false),
                        ));
                    }
                }
            }
        },
    )
}

fn line_events(lines: &[String]) -> Bytes {
    let mut body = String::new();
    for line in lines {
        body.push_str("event: line\ndata: ");
        body.push_str(line);
        body.push_str("\n\n");
    }
    Bytes::from(body)
}

fn done_event(outcome: &RunOutcome) -> Bytes {
    let data = serde_json::to_string(outcome).unwrap_or_else(|_| {
        r#"{"ok":false,"message":"failed to serialize outcome","redirect":null}"#.to_owned()
    });
    Bytes::from(format!("event: done\ndata: {data}\n\n"))
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
            &name,
            format!("Scaling {name}…"),
            DokkuCommand::PsScaleSet {
                app,
                scales: entries,
            },
            RunCompletion {
                success_message: format!("Scaled '{name}'."),
                redirect: None,
                refresh: RunRefresh::Reports,
            },
            Some(format!("/apps/{name}/partials/processes")),
        )
        .await;
    }

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
    req: HttpRequest,
    form: CsrfForm<DestroyForm>,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();

    let app = match AppName::try_from(name.clone()) {
        Ok(app) => app,
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
            &name,
            format!("Deleting {name}…"),
            DokkuCommand::AppsDestroy { app, force: true },
            RunCompletion {
                success_message: format!("App '{name}' destroyed."),
                redirect: Some("/".to_owned()),
                refresh: RunRefresh::All,
            },
            None,
        )
        .await;
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

pub async fn action_events(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<(String, u64)>,
) -> Result<HttpResponse, AppError> {
    let (name, id) = path.into_inner();
    current_user(&state, &session).await?;

    let run = state.action_runs.get(id).await.ok_or(AppError::NotFound)?;
    if run.app != name {
        return Err(AppError::NotFound);
    }

    Ok(HttpResponse::Ok()
        .content_type("text/event-stream")
        .insert_header(("Cache-Control", "no-store"))
        .insert_header(("X-Accel-Buffering", "no"))
        .streaming(action_stream(run)))
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
    let app = match AppName::try_from(name.clone()) {
        Ok(app) => app,
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
            &name,
            format!("{} {}…", action.present_participle(), name),
            action.command(app),
            RunCompletion {
                success_message: format!("App '{name}' {}.", action.past_tense()),
                redirect: None,
                refresh: RunRefresh::Reports,
            },
            Some(format!("/apps/{name}/partials/overview")),
        )
        .await;
    }

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
