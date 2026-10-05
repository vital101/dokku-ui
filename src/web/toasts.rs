use actix_session::Session;
use actix_web::{HttpResponse, web};
use askama::Template;

use crate::error::AppError;
use crate::storage::runs::RunSummary;
use crate::web::csrf_form::{CsrfForm, ensure_csrf};
use crate::web::fragments::current_user;
use crate::web::render::render;
use crate::web::state::AppState;

/// How many of the actor's runs the tray considers (queued/running + recent
/// completions).
const TOAST_LIMIT: usize = 20;

/// One tray row: the run's state, its audit label, and whether the actor has
/// already dismissed the completion toast.
#[derive(Debug, Clone)]
pub(super) struct ToastRow {
    run_id: String,
    label: String,
    state_label: String,
    acked: bool,
    csrf_token: String,
}

impl ToastRow {
    fn from_summary(
        summary: &RunSummary,
        acked: bool,
        csrf_token: &str,
        active_runs: &[String],
    ) -> Self {
        let state_label = match summary.ok {
            Some(true) => "succeeded",
            Some(false) => "failed",
            None if active_runs.contains(&summary.id) => "running",
            None => "queued",
        }
        .to_owned();
        Self {
            run_id: summary.id.clone(),
            label: format!("{} {}", summary.operation, summary.subject),
            state_label,
            acked,
            csrf_token: csrf_token.to_owned(),
        }
    }
}

#[derive(Template)]
#[template(path = "partials/toasts.html")]
pub(super) struct ToastsPartial {
    rows: Vec<ToastRow>,
}

/// The global toast tray: the session user's queued/running jobs and recent
/// completions (unacked), served from the shared SQLite row so any container
/// answers identically. Polled from `base.html` every few seconds.
pub async fn tray(state: web::Data<AppState>, session: Session) -> Result<HttpResponse, AppError> {
    let user = current_user(&state, &session).await?;
    let csrf_token = ensure_csrf(&session).await?;
    let rows = tray_rows(&state, user.id, &csrf_token).await?;
    render(&ToastsPartial { rows })
}

/// Dismisses a completion toast (CSRF-protected), returning the re-rendered
/// tray so htmx can swap it in place.
pub async fn ack(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
    _form: CsrfForm<AckForm>,
) -> Result<HttpResponse, AppError> {
    let run_id = path.into_inner();
    if !is_run_id(&run_id) {
        return Err(AppError::NotFound);
    }
    let user = current_user(&state, &session).await?;
    if state.action_runs.get(&run_id).await?.is_none() {
        return Err(AppError::NotFound);
    }
    state.action_runs.ack(&run_id, user.id).await?;
    let csrf_token = ensure_csrf(&session).await?;
    let rows = tray_rows(&state, user.id, &csrf_token).await?;
    render(&ToastsPartial { rows })
}

#[derive(serde::Deserialize)]
pub(super) struct AckForm {}

async fn tray_rows(
    state: &AppState,
    user_id: i64,
    csrf_token: &str,
) -> Result<Vec<ToastRow>, AppError> {
    let summaries = state
        .action_runs
        .list_for_actor(user_id, TOAST_LIMIT)
        .await?;
    let acked = state.action_runs.acked_run_ids(user_id).await?;
    let active: Vec<String> = state
        .jobs
        .list_active()
        .await?
        .into_iter()
        .map(|job| job.run_id)
        .collect();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let rows = summaries
        .into_iter()
        .filter(|summary| {
            // Running/queued runs always show; finished runs show only for a
            // few minutes and until the actor dismisses them.
            summary.ok.is_none() || (now - summary.created_at < 300 && !acked.contains(&summary.id))
        })
        .map(|summary| {
            ToastRow::from_summary(&summary, acked.contains(&summary.id), csrf_token, &active)
        })
        .collect();
    Ok(rows)
}

/// Run ids are 64 lowercase hex characters (`auth::csrf::generate_token`).
fn is_run_id(id: &str) -> bool {
    id.len() == 64 && id.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_ids_are_64_hex_characters() {
        assert!(is_run_id(&"a".repeat(64)));
        assert!(!is_run_id(&"g".repeat(64)));
        assert!(!is_run_id(&"a".repeat(63)));
        assert!(!is_run_id(""));
    }
}
