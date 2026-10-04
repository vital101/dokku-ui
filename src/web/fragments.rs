use std::sync::Arc;

use actix_session::Session;
use actix_web::{HttpRequest, HttpResponse};
use askama::Template;

use crate::dokku::{ActionRun, RunOutcome};
use crate::domain::command::DokkuCommand;
use crate::error::AppError;
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

pub(super) enum RunRefresh {
    Reports,
    All,
    /// No server-side refresh: the run fragment's `data-refresh` re-pulls the
    /// affected partial (used by service and volume actions, which are not in
    /// the app snapshot).
    None,
}

pub(super) struct RunCompletion {
    pub(super) success_message: String,
    pub(super) redirect: Option<String>,
    pub(super) refresh: RunRefresh,
}

/// Starts `command`, streaming its output into `run` line by line. On completion
/// refreshes the snapshot and records the outcome the SSE stream delivers.
pub(super) fn spawn_run(
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
                    RunRefresh::None => {}
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

/// Registers a run, spawns the command, and returns the streaming modal fragment.
pub(super) async fn start_action_run(
    state: &AppState,
    subject: &str,
    title: String,
    command: DokkuCommand,
    completion: RunCompletion,
    refresh_url: Option<String>,
) -> Result<HttpResponse, AppError> {
    let run = state.action_runs.insert(subject).await;
    spawn_run(state.clone(), run.clone(), command, completion);
    render(&RunPartial {
        run_url: format!("/actions/runs/{}/events", run.id),
        title,
        refresh_url,
    })
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
