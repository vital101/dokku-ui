use actix_session::Session;
use actix_web::{HttpRequest, HttpResponse, web};
use askama::Template;
use serde::Deserialize;

use crate::dokku::format_age;
use crate::domain::AppName;
use crate::domain::build::{
    BUILDERS, is_valid_builder_property, is_valid_builder_value, is_valid_buildpack,
    is_valid_buildpack_index,
};
use crate::domain::command::DokkuCommand;
use crate::domain::job::JobSpec;
use crate::domain::parse::{parse_builder_report, parse_buildpacks_list};
use crate::domain::types::BuilderReport;
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
#[template(path = "apps/build.html")]
struct BuildPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    name: &'a str,
    active_tab: &'static str,
}

struct BuildpackRow {
    index: usize,
    url: String,
}

#[derive(Template)]
#[template(path = "apps/partials/build.html")]
struct BuildPartial<'a> {
    name: &'a str,
    csrf_token: &'a str,
    buildpacks: Vec<BuildpackRow>,
    builder: BuilderReport,
    builders: Vec<String>,
    updated: String,
}

fn partial_url(name: &str) -> String {
    format!("/apps/{name}/partials/build")
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
    let page = BuildPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        name: &name,
        active_tab: "build",
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
    let buildpacks_output = match state
        .dokku
        .exec(&DokkuCommand::BuildpacksList { app: app.clone() })
        .await
    {
        Ok(output) => output,
        Err(err) => return error_fragment(&retry_url, &err.to_string()),
    };
    let builder_output = match state.dokku.exec(&DokkuCommand::BuilderReport { app }).await {
        Ok(output) => output,
        Err(err) => return error_fragment(&retry_url, &err.to_string()),
    };
    let buildpacks = parse_buildpacks_list(&buildpacks_output.stdout)
        .into_iter()
        .enumerate()
        .map(|(position, url)| BuildpackRow {
            index: position + 1,
            url,
        })
        .collect();
    let csrf_token = ensure_csrf(&session).await?;
    render(&BuildPartial {
        name: &name,
        csrf_token: &csrf_token,
        buildpacks,
        builder: parse_builder_report(&builder_output.stdout),
        builders: BUILDERS.iter().map(|name| (*name).to_owned()).collect(),
        updated: format_age(snapshot.age()),
    })
}

#[derive(Deserialize)]
pub struct BuildpackForm {
    buildpack: String,
    #[serde(default)]
    index: String,
}

#[derive(Deserialize)]
pub struct BuilderForm {
    property: String,
    #[serde(default)]
    value: String,
}

fn parse_index(raw: &str) -> Result<Option<u32>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let index = raw
        .parse::<u32>()
        .map_err(|_| "Position must be a number.".to_owned())?;
    if !is_valid_buildpack_index(index) {
        return Err("Position must be between 1 and 1000.".to_owned());
    }
    Ok(Some(index))
}

pub async fn buildpacks_add(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<BuildpackForm>,
) -> Result<HttpResponse, AppError> {
    buildpacks_mutate(&state, &session, &req, path.into_inner(), "add", form.0).await
}

pub async fn buildpacks_set(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<BuildpackForm>,
) -> Result<HttpResponse, AppError> {
    buildpacks_mutate(&state, &session, &req, path.into_inner(), "set", form.0).await
}

async fn buildpacks_mutate(
    state: &AppState,
    session: &Session,
    req: &HttpRequest,
    name: String,
    action: &str,
    form: BuildpackForm,
) -> Result<HttpResponse, AppError> {
    let buildpack = form.buildpack.trim().to_owned();
    if !is_valid_buildpack(&buildpack) {
        return reject(
            session,
            req,
            &name,
            "Buildpack must be a single-line URL or path without single quotes.",
        );
    }
    let index = match parse_index(&form.index) {
        Ok(index) => index,
        Err(message) => return reject(session, req, &name, &message),
    };
    let spec = if action == "add" {
        JobSpec::BuildpacksAdd {
            app: name.clone(),
            buildpack: buildpack.clone(),
            index,
        }
    } else {
        JobSpec::BuildpacksSet {
            app: name.clone(),
            buildpack: buildpack.clone(),
            index,
        }
    };
    let (operation, title, success, queued) = if action == "add" {
        (
            "buildpacks.add",
            format!("Adding buildpack to {name}…"),
            format!("Added buildpack to '{name}'."),
            format!("Queued: add buildpack to {name}."),
        )
    } else {
        (
            "buildpacks.set",
            format!("Setting buildpack for {name}…"),
            format!("Set buildpack for '{name}'."),
            format!("Queued: set buildpack for {name}."),
        )
    };
    run_spec(
        state,
        session,
        req,
        name,
        vec![spec],
        operation,
        title,
        success,
        queued,
    )
    .await
}

pub async fn buildpacks_remove(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<BuildpackForm>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let buildpack = form.0.buildpack.trim().to_owned();
    if !is_valid_buildpack(&buildpack) {
        return reject(&session, &req, &name, "Invalid buildpack.");
    }
    run_spec(
        &state,
        &session,
        &req,
        name.clone(),
        vec![JobSpec::BuildpacksRemove {
            app: name.clone(),
            buildpack,
        }],
        "buildpacks.remove",
        format!("Removing buildpack from {name}…"),
        format!("Removed buildpack from '{name}'."),
        format!("Queued: remove buildpack from {name}."),
    )
    .await
}

pub async fn buildpacks_clear(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    _form: CsrfForm<super::apps::ActionForm>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    run_spec(
        &state,
        &session,
        &req,
        name.clone(),
        vec![JobSpec::BuildpacksClear { app: name.clone() }],
        "buildpacks.clear",
        format!("Clearing buildpacks for {name}…"),
        format!("Cleared buildpacks for '{name}'."),
        format!("Queued: clear buildpacks for {name}."),
    )
    .await
}

pub async fn builder_set(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<BuilderForm>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let form = form.0;
    let property = form.property.trim().to_owned();
    if !is_valid_builder_property(&property) {
        return reject(&session, &req, &name, "Unknown builder property.");
    }
    let value = form.value.trim().to_owned();
    if !is_valid_builder_value(&property, &value) {
        return reject(&session, &req, &name, "Invalid builder value.");
    }
    run_spec(
        &state,
        &session,
        &req,
        name.clone(),
        vec![JobSpec::BuilderSet {
            app: name.clone(),
            property,
            value: (!value.is_empty()).then_some(value),
        }],
        "builder.set",
        format!("Updating builder for {name}…"),
        format!("Builder updated for '{name}'."),
        format!("Queued: update builder for {name}."),
    )
    .await
}

fn reject(
    session: &Session,
    req: &HttpRequest,
    name: &str,
    message: &str,
) -> Result<HttpResponse, AppError> {
    if is_htmx(req) {
        return modal_error(message);
    }
    set_flash(session, FlashLevel::Error, message.to_owned());
    Ok(see_other(&format!("/apps/{name}/build")))
}

#[allow(clippy::too_many_arguments)]
async fn run_spec(
    state: &AppState,
    session: &Session,
    req: &HttpRequest,
    name: String,
    plan: Vec<JobSpec>,
    operation: &str,
    title: String,
    success_message: String,
    queued_message: String,
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
        operation,
        TargetKind::App,
        &plan,
        &completion,
        &[],
    )
    .await?;
    set_flash(session, FlashLevel::Success, queued_message);
    Ok(see_other(&format!("/apps/{name}/build")))
}
