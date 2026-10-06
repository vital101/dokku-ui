use actix_session::Session;
use actix_web::http::header::{CACHE_CONTROL, CONTENT_LENGTH, HeaderValue};
use actix_web::{HttpRequest, HttpResponse, web};
use askama::Template;
use serde::Deserialize;

use crate::auth::csrf::generate_token;
use crate::domain::AppName;
use crate::domain::git::{
    GitBuildMode, is_valid_git_ref, is_valid_git_remote, remote_secret_fragments,
};
use crate::domain::job::JobSpec;
use crate::domain::webhook::{
    SIGNATURE_HEADER, push_branch_matches, repo_matches, verify_github_signature,
};
use crate::error::AppError;
use crate::storage::runs::{Actor, TargetKind};
use crate::storage::webhooks::Webhook;
use crate::web::csrf_form::CsrfForm;
use crate::web::flash::{FlashLevel, set_flash};
use crate::web::fragments::{
    RunCompletion, RunRefresh, current_user, enqueue_action_run_as, is_htmx, modal_error,
};
use crate::web::render::{render, see_other};
use crate::web::state::AppState;

/// Anonymous deliveries are attributed to this system identity in the audit
/// trail (no user id, so they never appear in a user's toast tray).
pub const WEBHOOK_ACTOR_EMAIL: &str = "github-webhook";

/// GitHub's own delivery limit; anything larger is not a real webhook.
const MAX_BODY_BYTES: usize = 256 * 1024;

/// The deploy-panel view of a stored webhook (never carries the secret).
#[derive(Debug, Clone)]
pub(super) struct WebhookView {
    pub repo: String,
    pub branch: String,
    pub build_mode: String,
    pub enabled: bool,
}

impl From<&Webhook> for WebhookView {
    fn from(webhook: &Webhook) -> Self {
        Self {
            repo: webhook.repo.clone(),
            branch: webhook.branch.clone(),
            build_mode: webhook.build_mode.clone(),
            enabled: webhook.enabled,
        }
    }
}

#[derive(Deserialize)]
pub struct WebhookForm {
    repo: String,
    #[serde(default)]
    branch: String,
    #[serde(default)]
    build_mode: String,
    #[serde(default)]
    enabled: String,
}

pub async fn save(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<WebhookForm>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    current_user(&state, &session).await?;
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
    let branch = form.branch.trim().to_owned();
    if !is_valid_git_ref(&branch) {
        return reject(
            &session,
            &req,
            &name,
            "Branch may only contain letters, digits, '.', '_', '/', and '-'.",
        );
    }
    let Some(build_mode) = GitBuildMode::parse(form.build_mode.trim()) else {
        return reject(&session, &req, &name, "Unknown build mode.");
    };
    if let Err(err) = AppName::try_from(name.clone()) {
        return reject(&session, &req, &name, &format!("Invalid app name: {err}"));
    }
    // The secret is generated once and survives edits so GitHub keeps working.
    let secret = match state.webhooks.get(&name).await? {
        Some(existing) => existing.secret,
        None => generate_token(),
    };
    let enabled = matches!(form.enabled.as_str(), "on" | "true" | "1");
    state
        .webhooks
        .upsert(&Webhook {
            app: name.clone(),
            repo,
            branch,
            build_mode: build_mode.as_str().to_owned(),
            secret,
            enabled,
        })
        .await?;
    if is_htmx(&req) {
        let mut response = HttpResponse::Ok();
        response.insert_header(("HX-Redirect", format!("/apps/{name}/deploy")));
        return Ok(response.finish());
    }
    set_flash(
        &session,
        FlashLevel::Success,
        format!("Webhook saved for {name}."),
    );
    Ok(see_other(&format!("/apps/{name}/deploy")))
}

pub async fn remove(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    _form: CsrfForm<super::apps::ActionForm>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    current_user(&state, &session).await?;
    state.webhooks.delete(&name).await?;
    if is_htmx(&req) {
        let mut response = HttpResponse::Ok();
        response.insert_header(("HX-Redirect", format!("/apps/{name}/deploy")));
        return Ok(response.finish());
    }
    set_flash(
        &session,
        FlashLevel::Success,
        format!("Webhook removed for {name}."),
    );
    Ok(see_other(&format!("/apps/{name}/deploy")))
}

#[derive(Template)]
#[template(path = "apps/partials/webhook_secret.html")]
struct WebhookSecretPartial {
    secret: String,
}

/// Reveals the HMAC secret under the re-auth window, `no-store` (mirrors the
/// config reveal). The section renders inline into the webhook card.
pub async fn reveal(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    _form: CsrfForm<super::apps::ActionForm>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    current_user(&state, &session).await?;
    if !reauth_valid(&session) {
        let next = format!("/apps/{name}/deploy");
        if is_htmx(&req) {
            let mut response = HttpResponse::Ok();
            response.insert_header(("HX-Redirect", format!("/reauth?next={next}")));
            return Ok(response.finish());
        }
        return Ok(see_other(&format!("/reauth?next={next}")));
    }
    let Some(webhook) = state.webhooks.get(&name).await? else {
        return Ok(
            modal_error("No webhook is configured for this app.").unwrap_or_else(|_| {
                HttpResponse::Ok().body("No webhook is configured for this app.")
            }),
        );
    };
    record_reveal(&state, &session, &name).await;
    let page = WebhookSecretPartial {
        secret: webhook.secret,
    };
    let mut response = render(&page)?;
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

fn reauth_valid(session: &Session) -> bool {
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    match session.get::<i64>(crate::auth::reauth::REAUTH_UNTIL) {
        Ok(Some(until)) => crate::auth::reauth::reauth_valid(now, until),
        _ => false,
    }
}

async fn record_reveal(state: &AppState, session: &Session, name: &str) {
    let actor = crate::web::fragments::current_actor(state, session).await;
    if let Ok(run_id) = state
        .action_runs
        .insert_with(&crate::storage::runs::NewRun {
            subject: name.to_owned(),
            operation: "webhook.reveal".to_owned(),
            target_kind: TargetKind::App,
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
                    message: "Webhook secret revealed.".to_owned(),
                    redirect: None,
                },
            )
            .await;
    }
}

/// The public GitHub receiver. Unauthenticated by design: authentication is
/// the HMAC signature over the raw body. Unknown or disabled apps 404 so the
/// endpoint never reveals which apps exist.
pub async fn receive(
    state: web::Data<AppState>,
    path: web::Path<String>,
    req: HttpRequest,
    body: web::Bytes,
) -> HttpResponse {
    let too_large = req
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_BODY_BYTES)
        || body.len() > MAX_BODY_BYTES;
    if too_large {
        return HttpResponse::PayloadTooLarge().finish();
    }

    let name = path.into_inner();
    if AppName::try_from(name.as_str()).is_err() {
        return HttpResponse::NotFound().finish();
    }
    let webhook = match state.webhooks.get(&name).await {
        Ok(Some(webhook)) if webhook.enabled => webhook,
        Ok(_) => return HttpResponse::NotFound().finish(),
        Err(err) => {
            tracing::error!(error = %err, "failed to load webhook config");
            return HttpResponse::InternalServerError().finish();
        }
    };

    let signature = req
        .headers()
        .get(SIGNATURE_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !verify_github_signature(&webhook.secret, &body, signature) {
        return HttpResponse::Unauthorized().finish();
    }

    let event = req
        .headers()
        .get("X-GitHub-Event")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    match event {
        "ping" => HttpResponse::Ok().body("pong"),
        "push" => handle_push(&state, &name, &webhook, &body).await,
        _ => HttpResponse::Ok().body("ignored"),
    }
}

async fn handle_push(state: &AppState, name: &str, webhook: &Webhook, body: &[u8]) -> HttpResponse {
    let payload: serde_json::Value = match serde_json::from_slice(body) {
        Ok(payload) => payload,
        Err(_) => return HttpResponse::BadRequest().finish(),
    };
    if !repo_matches(&webhook.repo, &payload) || !push_branch_matches(&webhook.branch, &payload) {
        return HttpResponse::Ok().body("ignored");
    }
    let build_mode =
        GitBuildMode::parse(&webhook.build_mode).unwrap_or(GitBuildMode::BuildIfChanges);
    let mut redactions = remote_secret_fragments(&webhook.repo);
    redactions.push(webhook.secret.clone());
    let completion = RunCompletion {
        success_message: format!("Deployed '{name}' from a GitHub push."),
        redirect: None,
        refresh: RunRefresh::Reports,
    };
    let plan = vec![JobSpec::GitSync {
        app: name.to_owned(),
        repo: webhook.repo.clone(),
        git_ref: Some(webhook.branch.clone()),
        build_mode,
    }];
    let actor = Actor {
        user_id: None,
        email: Some(WEBHOOK_ACTOR_EMAIL.to_owned()),
    };
    match enqueue_action_run_as(
        state,
        actor,
        name,
        "git.sync",
        TargetKind::App,
        &plan,
        &completion,
        &redactions,
    )
    .await
    {
        Ok(_) => HttpResponse::Accepted().body("queued"),
        Err(err) => {
            tracing::error!(error = %err, "failed to enqueue webhook deploy");
            HttpResponse::InternalServerError().finish()
        }
    }
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
