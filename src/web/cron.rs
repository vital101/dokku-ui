use actix_session::Session;
use actix_web::{HttpRequest, HttpResponse, web};
use askama::Template;
use serde::Deserialize;

use crate::dokku::format_age;
use crate::domain::AppName;
use crate::domain::command::DokkuCommand;
use crate::domain::cron::is_valid_cron_id;
use crate::domain::job::JobSpec;
use crate::domain::parse::parse_cron_tasks;
use crate::error::AppError;
use crate::storage::runs::TargetKind;
use crate::web::csrf_form::{CsrfForm, ensure_csrf};
use crate::web::flash::{FlashLevel, FlashMessage, set_flash, take_flash};
use crate::web::fragments::{
    RunCompletion, RunRefresh, RunRequest, current_user, enqueue_action_run, error_fragment,
    is_htmx, modal_error, start_action_run,
};
use crate::web::render::{render, see_other};
use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "apps/cron.html")]
struct CronPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    name: &'a str,
    active_tab: &'static str,
}

struct CronRow {
    id: String,
    schedule: String,
    command: String,
    concurrency_policy: String,
    runs_in_maintenance: bool,
    suspended: bool,
}

#[derive(Template)]
#[template(path = "apps/partials/cron.html")]
struct CronPartial<'a> {
    name: &'a str,
    csrf_token: &'a str,
    rows: Vec<CronRow>,
    updated: String,
}

fn partial_url(name: &str) -> String {
    format!("/apps/{name}/partials/cron")
}

pub async fn page(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let user = current_user(&state, &session).await?;
    state.snapshot.resolve_app(&name).await?;

    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);
    let page = CronPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        name: &name,
        active_tab: "cron",
    };
    render(&page)
}

pub async fn partial(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    current_user(&state, &session).await?;

    let retry_url = partial_url(&name);
    let (snapshot, app) = match state.snapshot.resolve_app(&name).await {
        Ok(resolved) => resolved,
        Err(err) => {
            let message = if err.is_not_found() {
                format!("App '{name}' was not found — it may have been deleted.")
            } else {
                err.to_string()
            };
            return error_fragment(&retry_url, &message);
        }
    };
    let output = match state.dokku.exec(&DokkuCommand::CronList { app }).await {
        Ok(output) => output,
        Err(err) => return error_fragment(&retry_url, &err.to_string()),
    };
    let rows = parse_cron_tasks(&output.stdout)
        .into_iter()
        .map(|task| CronRow {
            id: task.id,
            schedule: task.schedule,
            command: task.command,
            concurrency_policy: task.concurrency_policy,
            runs_in_maintenance: task.maintenance,
            suspended: task.task_in_maintenance,
        })
        .collect();
    let csrf_token = ensure_csrf(&session).await?;
    render(&CronPartial {
        name: &name,
        csrf_token: &csrf_token,
        rows,
        updated: format_age(snapshot.age()),
    })
}

#[derive(Deserialize)]
pub struct CronForm {
    cron_id: String,
}

pub async fn run(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<CronForm>,
) -> Result<HttpResponse, AppError> {
    mutate(
        &state,
        &session,
        &req,
        path.into_inner(),
        "run",
        &form.0.cron_id,
    )
    .await
}

pub async fn suspend(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<CronForm>,
) -> Result<HttpResponse, AppError> {
    mutate(
        &state,
        &session,
        &req,
        path.into_inner(),
        "suspend",
        &form.0.cron_id,
    )
    .await
}

pub async fn resume(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<CronForm>,
) -> Result<HttpResponse, AppError> {
    mutate(
        &state,
        &session,
        &req,
        path.into_inner(),
        "resume",
        &form.0.cron_id,
    )
    .await
}

async fn mutate(
    state: &AppState,
    session: &Session,
    req: &HttpRequest,
    name: String,
    action: &str,
    cron_id: &str,
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
    let cron_id = cron_id.trim();
    if !is_valid_cron_id(cron_id) {
        let message = "Unknown cron task id.".to_owned();
        if is_htmx(req) {
            return modal_error(message);
        }
        set_flash(session, FlashLevel::Error, message);
        return Ok(see_other(&format!("/apps/{name}/cron")));
    }

    let spec = match action {
        "suspend" => JobSpec::CronSuspend {
            app: name.clone(),
            cron_id: cron_id.to_owned(),
        },
        "resume" => JobSpec::CronResume {
            app: name.clone(),
            cron_id: cron_id.to_owned(),
        },
        _ => JobSpec::CronRun {
            app: name.clone(),
            cron_id: cron_id.to_owned(),
        },
    };
    let (verb, past) = match action {
        "suspend" => ("suspend", "suspended"),
        "resume" => ("resume", "resumed"),
        _ => ("run", "ran"),
    };
    let completion = RunCompletion {
        success_message: format!("Cron task '{cron_id}' {past} for '{name}'."),
        redirect: None,
        refresh: RunRefresh::None,
    };
    let plan = vec![spec];
    if is_htmx(req) {
        return start_action_run(
            state,
            session,
            &RunRequest {
                subject: name.clone(),
                operation: format!("cron.{verb}"),
                target_kind: TargetKind::App,
                title: format!("Running cron action for {name}…"),
                plan,
                completion,
                redactions: Vec::new(),
                refresh_url: Some(partial_url(&name)),
            },
        )
        .await;
    }
    let _ = enqueue_action_run(
        state,
        session,
        &name,
        &format!("cron.{verb}"),
        TargetKind::App,
        &plan,
        &completion,
        &[],
    )
    .await?;
    set_flash(
        session,
        FlashLevel::Success,
        format!("Queued: {verb} cron task {cron_id}."),
    );
    Ok(see_other(&format!("/apps/{name}/cron")))
}
