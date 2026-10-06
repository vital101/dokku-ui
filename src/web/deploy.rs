use actix_session::Session;
use actix_web::{HttpRequest, HttpResponse, web};
use askama::Template;
use serde::Deserialize;

use crate::dokku::format_age;
use crate::domain::AppName;
use crate::domain::command::DokkuCommand;
use crate::domain::git::{
    GitBuildMode, is_valid_archive_url, is_valid_git_ref, is_valid_git_remote, is_valid_image_ref,
    remote_secret_fragments,
};
use crate::domain::job::JobSpec;
use crate::domain::parse::{parse_git_public_key, parse_git_report, parse_logs_failed};
use crate::domain::types::GitReport;
use crate::error::AppError;
use crate::settings::Settings;
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
#[template(path = "apps/deploy.html")]
struct DeployPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    name: &'a str,
    active_tab: &'static str,
}

#[derive(Template)]
#[template(path = "apps/partials/deploy.html")]
struct DeployPartial<'a> {
    name: &'a str,
    csrf_token: &'a str,
    push_url: String,
    public_key: Option<String>,
    git: GitReport,
    webhook: Option<super::webhooks::WebhookView>,
    webhook_url: String,
    updated: String,
}

fn partial_url(name: &str) -> String {
    format!("/apps/{name}/partials/deploy")
}

/// The git remote for pushing to this app. A non-standard SSH port needs the
/// `ssh://` URL form; scp-style syntax cannot carry a port.
fn push_url(name: &str, settings: &Settings) -> String {
    if settings.dokku_ssh_port == 22 {
        format!(
            "{}@{}:{}",
            settings.dokku_ssh_user, settings.dokku_host, name
        )
    } else {
        format!(
            "ssh://{}@{}:{}/{}",
            settings.dokku_ssh_user, settings.dokku_host, settings.dokku_ssh_port, name
        )
    }
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
    let page = DeployPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        name: &name,
        active_tab: "deploy",
    };
    render(&page)
}

pub async fn partial(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    render_partial(&state, &session, &req, path.into_inner()).await
}

/// The deploy panel: git summary, push URL/key, webhook config, and the lazy
/// failed-logs placeholder. All reads are per-request; nothing is snapshotted.
async fn render_partial(
    state: &AppState,
    session: &Session,
    req: &HttpRequest,
    name: String,
) -> Result<HttpResponse, AppError> {
    current_user(state, session).await?;

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
    let report_output = match state.dokku.exec(&DokkuCommand::GitReport { app }).await {
        Ok(output) => output,
        Err(err) => return error_fragment(&retry_url, &err.to_string()),
    };
    // A host without a generated deploy key exits 1 here; that is a normal
    // state the card explains, not a panel failure.
    let public_key = state
        .dokku
        .exec(&DokkuCommand::GitPublicKey)
        .await
        .ok()
        .and_then(|output| parse_git_public_key(&output.stdout));
    let webhook = match state.webhooks.get(&name).await {
        Ok(webhook) => webhook,
        Err(err) => {
            tracing::warn!(error = %err, "failed to load webhook config");
            None
        }
    };
    let csrf_token = ensure_csrf(session).await?;
    let connection = req.connection_info();
    render(&DeployPartial {
        name: &name,
        csrf_token: &csrf_token,
        push_url: push_url(&name, &state.settings),
        public_key,
        git: parse_git_report(&report_output.stdout),
        webhook: webhook
            .as_ref()
            .map(crate::web::webhooks::WebhookView::from),
        webhook_url: format!(
            "{}://{}/webhooks/github/{}",
            connection.scheme(),
            connection.host(),
            name
        ),
        updated: format_age(snapshot.age()),
    })
}

#[derive(Deserialize)]
pub struct DeployBranchForm {
    #[serde(default)]
    deploy_branch: String,
}

#[derive(Template)]
#[template(path = "apps/partials/failed_logs.html")]
struct FailedLogsPartial<'a> {
    name: &'a str,
    lines: Vec<String>,
}

/// Lazily loaded by the Deploy tab: the last failed deploy's log, fetched on
/// its own so the main panel never waits on it.
pub async fn failed_logs_partial(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    current_user(&state, &session).await?;
    let retry_url = format!("/apps/{name}/partials/failed-logs");
    let (_snapshot, app) = match state.snapshot.resolve_app(&name).await {
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
    let output = match state.dokku.exec(&DokkuCommand::LogsFailed { app }).await {
        Ok(output) => output,
        Err(err) => return error_fragment(&retry_url, &err.to_string()),
    };
    render(&FailedLogsPartial {
        name: &name,
        lines: parse_logs_failed(&output.stdout).as_slice().to_vec(),
    })
}

#[derive(Deserialize)]
pub struct SyncForm {
    repo: String,
    #[serde(default)]
    git_ref: String,
    #[serde(default)]
    build_mode: String,
}

#[derive(Deserialize)]
pub struct ImageForm {
    image: String,
}

#[derive(Deserialize)]
pub struct ArchiveForm {
    archive_url: String,
}

fn parse_build_mode(raw: &str) -> Option<GitBuildMode> {
    match raw {
        "build" => Some(GitBuildMode::Build),
        "build-if-changes" => Some(GitBuildMode::BuildIfChanges),
        "" | "no-build" => Some(GitBuildMode::NoBuild),
        _ => None,
    }
}

pub async fn sync(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<SyncForm>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let form = form.0;
    let repo = form.repo.trim().to_owned();
    if !is_valid_git_remote(&repo) {
        return reject(
            &session,
            &req,
            &name,
            "Repository must be an https://, ssh://, or git@host:path remote.",
        );
    }
    let git_ref = form.git_ref.trim().to_owned();
    if !git_ref.is_empty() && !is_valid_git_ref(&git_ref) {
        return reject(
            &session,
            &req,
            &name,
            "Git ref may only contain letters, digits, '.', '_', '/', and '-'.",
        );
    }
    let Some(build_mode) = parse_build_mode(form.build_mode.trim()) else {
        return reject(&session, &req, &name, "Unknown build mode.");
    };
    if let Err(err) = AppName::try_from(name.clone()) {
        return reject(&session, &req, &name, &format!("Invalid app name: {err}"));
    }
    let redactions = remote_secret_fragments(&repo);
    run_deploy(
        &state,
        &session,
        &req,
        name.clone(),
        vec![JobSpec::GitSync {
            app: name,
            repo,
            git_ref: (!git_ref.is_empty()).then_some(git_ref),
            build_mode,
        }],
        "git.sync",
        "Syncing from git for",
        "Synced from git",
        "sync from git",
        redactions,
    )
    .await
}

pub async fn from_image(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<ImageForm>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let image = form.0.image.trim().to_owned();
    if !is_valid_image_ref(&image) {
        return reject(
            &session,
            &req,
            &name,
            "Image must be a registry reference like ghcr.io/org/app:tag.",
        );
    }
    if let Err(err) = AppName::try_from(name.clone()) {
        return reject(&session, &req, &name, &format!("Invalid app name: {err}"));
    }
    run_deploy(
        &state,
        &session,
        &req,
        name.clone(),
        vec![JobSpec::GitFromImage { app: name, image }],
        "git.from-image",
        "Deploying from image for",
        "Deployed from image",
        "deploy from image",
        Vec::new(),
    )
    .await
}

pub async fn from_archive(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<ArchiveForm>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let archive_url = form.0.archive_url.trim().to_owned();
    if !is_valid_archive_url(&archive_url) {
        return reject(
            &session,
            &req,
            &name,
            "Archive URL must be an http(s) URL to a tar, tar.gz, or zip file.",
        );
    }
    if let Err(err) = AppName::try_from(name.clone()) {
        return reject(&session, &req, &name, &format!("Invalid app name: {err}"));
    }
    let redactions = remote_secret_fragments(&archive_url);
    run_deploy(
        &state,
        &session,
        &req,
        name.clone(),
        vec![JobSpec::GitFromArchive {
            app: name,
            archive_url,
        }],
        "git.from-archive",
        "Deploying from archive for",
        "Deployed from archive",
        "deploy from archive",
        redactions,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_deploy(
    state: &AppState,
    session: &Session,
    req: &HttpRequest,
    name: String,
    plan: Vec<JobSpec>,
    operation: &str,
    title: &str,
    success: &str,
    queued: &str,
    redactions: Vec<String>,
) -> Result<HttpResponse, AppError> {
    let title = format!("{title} {name}…");
    let success_message = format!("{success} ('{name}')");
    let completion = RunCompletion {
        success_message,
        redirect: None,
        refresh: RunRefresh::Reports,
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
                refresh_url: Some(partial_url(&name)),
            },
        )
        .await;
    }
    enqueue_action_run(
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
    set_flash(
        session,
        FlashLevel::Success,
        format!("Queued: {queued} for {name}."),
    );
    Ok(see_other(&format!("/apps/{name}/deploy")))
}

pub async fn set_branch(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<DeployBranchForm>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let value = form.0.deploy_branch.trim().to_owned();
    if !value.is_empty() && !is_valid_git_ref(&value) {
        return reject(
            &session,
            &req,
            &name,
            "Deploy branch may only contain letters, digits, '.', '_', '/', and '-'.",
        );
    }
    if let Err(err) = AppName::try_from(name.clone()) {
        return reject(&session, &req, &name, &format!("Invalid app name: {err}"));
    }
    let plan = vec![JobSpec::GitSet {
        app: name.clone(),
        property: crate::domain::git::DEPLOY_BRANCH_PROPERTY.to_owned(),
        value: (!value.is_empty()).then_some(value),
    }];
    let completion = RunCompletion {
        success_message: format!("Deploy branch updated for '{name}'."),
        redirect: None,
        refresh: RunRefresh::None,
    };
    if is_htmx(&req) {
        return start_action_run(
            &state,
            &session,
            &RunRequest {
                subject: name.clone(),
                operation: "git.set".to_owned(),
                target_kind: TargetKind::App,
                title: format!("Updating deploy branch for {name}…"),
                plan,
                completion,
                redactions: Vec::new(),
                refresh_url: Some(partial_url(&name)),
            },
        )
        .await;
    }
    enqueue_action_run(
        &state,
        &session,
        &name,
        "git.set",
        TargetKind::App,
        &plan,
        &completion,
        &[],
    )
    .await?;
    set_flash(
        &session,
        FlashLevel::Success,
        format!("Queued: update deploy branch for {name}."),
    );
    Ok(see_other(&format!("/apps/{name}/deploy")))
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
    Ok(see_other(&format!("/apps/{name}/deploy")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(port: u16) -> Settings {
        Settings::from_map(&std::collections::HashMap::from([(
            "DOKKU_SSH_PORT".to_owned(),
            port.to_string(),
        )]))
        .expect("settings")
    }

    #[test]
    fn push_url_uses_scp_style_on_the_default_port() {
        let settings = Settings::from_map(&std::collections::HashMap::from([
            ("DOKKU_HOST".to_owned(), "dokku.example.com".to_owned()),
            ("DOKKU_SSH_USER".to_owned(), "dokku".to_owned()),
        ]))
        .expect("settings");
        assert_eq!(
            push_url("alpha", &settings),
            "dokku@dokku.example.com:alpha"
        );
    }

    #[test]
    fn push_url_uses_ssh_scheme_on_a_custom_port() {
        let settings = settings(2222);
        assert_eq!(
            push_url("alpha", &settings),
            "ssh://dokku@host.docker.internal:2222/alpha"
        );
    }
}
