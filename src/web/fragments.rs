use actix_session::Session;
use actix_web::{HttpRequest, HttpResponse};
use askama::Template;

use crate::dokku::workers::{DEFAULT_MAX_ATTEMPTS, spawn_job_executor};
use crate::domain::command::DokkuCommand;
use crate::domain::job::{CompletionRefresh, CompletionSpec, JobPayload, JobSpec};
use crate::error::AppError;
use crate::storage::runs::{Actor, NewRun, RunOutcome, TargetKind};
use crate::storage::users::{SqliteUsersRepo, UsersRepo};
use crate::web::auth_middleware::SESSION_USER_ID;
use crate::web::render::render;
use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "partials/error.html")]
pub(super) struct ErrorPartial<'a> {
    message: &'a str,
    retry_url: String,
}

#[derive(Template)]
#[template(path = "partials/run.html")]
pub(super) struct RunPartial {
    run_url: String,
    title: String,
    refresh_url: Option<String>,
}

#[derive(Template)]
#[template(path = "partials/modal_error.html")]
pub(super) struct ModalErrorPartial<'a> {
    message: &'a str,
}

/// Renders the small retry card that htmx swaps in when a partial fetch fails.
/// Always 200: htmx does not swap 4xx/5xx responses, so an error status would
/// leave the loading skeleton spinning forever.
pub(super) fn error_fragment(retry_url: &str, message: &str) -> Result<HttpResponse, AppError> {
    let page = ErrorPartial {
        message,
        retry_url: retry_url.to_owned(),
    };
    render(&page)
}

pub(super) fn modal_error(message: impl Into<String>) -> Result<HttpResponse, AppError> {
    let message = message.into();
    let page = ModalErrorPartial { message: &message };
    render(&page)
}

pub(super) fn is_htmx(req: &HttpRequest) -> bool {
    req.headers().contains_key("HX-Request")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RunRefresh {
    Reports,
    All,
    /// No server-side refresh: the run fragment's `data-refresh` re-pulls the
    /// affected partial (used by service and volume actions, which are not in
    /// the app snapshot).
    None,
}

#[derive(Debug, Clone)]
pub(super) struct RunCompletion {
    pub(super) success_message: String,
    pub(super) redirect: Option<String>,
    pub(super) refresh: RunRefresh,
}

/// The audit dimensions a handler declares when starting a run.
#[derive(Debug, Clone)]
pub(super) struct RunRequest {
    /// Target name: app name, service name, or volume entry.
    pub(super) subject: String,
    /// Stable machine key, e.g. `app.restart`, `service.create`.
    pub(super) operation: String,
    pub(super) target_kind: TargetKind,
    pub(super) title: String,
    /// The commands to run, as a validated, serializable job plan.
    pub(super) plan: Vec<JobSpec>,
    pub(super) completion: RunCompletion,
    pub(super) refresh_url: Option<String>,
    /// Literal secrets redacted from persisted run lines.
    pub(super) redactions: Vec<String>,
}

/// Attributes the run to the session's user when possible; a missing or stale
/// session degrades to a system run so the action itself never blocks on audit.
pub(super) async fn current_actor(state: &AppState, session: &Session) -> Actor {
    match current_user(state, session).await {
        Ok(user) => Actor {
            user_id: Some(user.id),
            email: Some(user.email),
        },
        Err(_) => Actor {
            user_id: None,
            email: None,
        },
    }
}

fn completion_spec(completion: &RunCompletion) -> CompletionSpec {
    let refresh = match completion.refresh {
        RunRefresh::Reports => CompletionRefresh::Reports,
        RunRefresh::All => CompletionRefresh::All,
        RunRefresh::None => CompletionRefresh::None,
    };
    CompletionSpec {
        success_message: completion.success_message.clone(),
        redirect: completion.redirect.clone(),
        refresh,
    }
}

fn job_payload(plan: &[JobSpec], completion: &RunCompletion, redactions: &[String]) -> JobPayload {
    JobPayload {
        plan: plan.to_vec(),
        completion: completion_spec(completion),
        redactions: redactions.to_vec(),
    }
}

fn max_attempts(plan: &[JobSpec]) -> usize {
    if plan.iter().any(JobSpec::is_destructive) {
        1
    } else {
        DEFAULT_MAX_ATTEMPTS
    }
}

/// Registers a run + its job, spawns an executor for prompt execution, and
/// returns the streaming modal fragment. The job is persisted in SQLite, so a
/// dead process never loses it — another container's worker reclaims it.
pub(super) async fn start_action_run(
    state: &AppState,
    session: &Session,
    request: &RunRequest,
) -> Result<HttpResponse, AppError> {
    // A SQLite failure here must not return 4xx/5xx: htmx does not swap
    // those, so the modal would spin forever. Render a 200 error card instead.
    let run_id = match enqueue_action_run(
        state,
        session,
        &request.subject,
        &request.operation,
        request.target_kind,
        &request.plan,
        &request.completion,
        &request.redactions,
    )
    .await
    {
        Ok(run_id) => run_id,
        Err(err) => return modal_error(err.to_string()),
    };
    render(&RunPartial {
        run_url: format!("/actions/runs/{run_id}/events"),
        title: request.title.clone(),
        refresh_url: request.refresh_url.clone(),
    })
}

/// Persists a run + its job and spawns an executor. Returns the run id. The
/// no-JS paths call this directly and redirect with a "queued" flash.
#[allow(clippy::too_many_arguments)]
pub(super) async fn enqueue_action_run(
    state: &AppState,
    session: &Session,
    subject: &str,
    operation: &str,
    target_kind: TargetKind,
    plan: &[JobSpec],
    completion: &RunCompletion,
    redactions: &[String],
) -> Result<String, AppError> {
    let actor = current_actor(state, session).await;
    enqueue_action_run_as(
        state,
        actor,
        subject,
        operation,
        target_kind,
        plan,
        completion,
        redactions,
    )
    .await
}

/// Enqueues with an explicit actor, for paths without a session (the public
/// GitHub webhook route attributes to a system identity).
#[allow(clippy::too_many_arguments)]
pub(super) async fn enqueue_action_run_as(
    state: &AppState,
    actor: Actor,
    subject: &str,
    operation: &str,
    target_kind: TargetKind,
    plan: &[JobSpec],
    completion: &RunCompletion,
    redactions: &[String],
) -> Result<String, AppError> {
    let run_id = state
        .action_runs
        .insert_with(&NewRun {
            subject: subject.to_owned(),
            operation: operation.to_owned(),
            target_kind,
            actor,
            parent_run_id: None,
        })
        .await?;
    let job_id = state
        .jobs
        .enqueue(
            &run_id,
            &job_payload(plan, completion, redactions),
            max_attempts(plan),
        )
        .await?;
    spawn_job_executor(state.clone(), job_id.clone(), job_id);
    Ok(run_id)
}

/// Runs a mutating command synchronously (the no-JS create paths, whose
/// redirect target must already exist), recording a completed run for the
/// audit trail with the command's stdout as its lines. Audit writes are
/// best-effort: a failure to record never blocks the action.
pub(super) async fn run_synchronously(
    state: &AppState,
    session: &Session,
    subject: &str,
    operation: &str,
    target_kind: TargetKind,
    command: DokkuCommand,
    success_message: &str,
) -> Result<crate::dokku::DokkuOutput, crate::dokku::DokkuError> {
    let actor = current_actor(state, session).await;
    let mut run_id: Option<String> = None;
    if let Ok(id) = state
        .action_runs
        .insert_with(&NewRun {
            subject: subject.to_owned(),
            operation: operation.to_owned(),
            target_kind,
            actor,
            parent_run_id: None,
        })
        .await
    {
        run_id = Some(id);
    } else {
        tracing::warn!("failed to record audit run for {operation}");
    }

    let result = state.dokku.exec(&command).await;
    if let Some(run_id) = run_id {
        if let Ok(output) = &result {
            for (seq, line) in output.stdout.lines().enumerate() {
                if let Err(err) = state.action_runs.append_line(&run_id, seq, line, &[]).await {
                    tracing::warn!(error = %err, "failed to persist audit line");
                }
            }
        }
        let outcome = match &result {
            Ok(_) => RunOutcome {
                ok: true,
                message: success_message.to_owned(),
                redirect: None,
            },
            Err(err) => RunOutcome {
                ok: false,
                message: err.to_string(),
                redirect: None,
            },
        };
        if let Err(err) = state.action_runs.finish(&run_id, &outcome).await {
            tracing::warn!(error = %err, "failed to persist audit outcome");
        }
    }
    result
}

pub(super) async fn current_user(
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
