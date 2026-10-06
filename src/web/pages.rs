use actix_session::Session;
use actix_web::{HttpResponse, web};
use askama::Template;
use serde::Deserialize;

use crate::dokku::{AppRow, dashboard_from_snapshot_filtered, format_age};
use crate::domain::types::AppStats;
use crate::error::AppError;
use crate::storage::instance_settings::{InstanceSettingsRepo, SqliteInstanceSettingsRepo};
use crate::storage::users::{SqliteUsersRepo, UsersRepo};
use crate::web::auth_middleware::SESSION_USER_ID;
use crate::web::csrf_form::{CsrfForm, ensure_csrf};
use crate::web::flash::{FlashLevel, FlashMessage, set_flash, take_flash};
use crate::web::render::see_other;
use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardPage {
    email: String,
    csrf_token: String,
    flash: Option<FlashMessage>,
    stats: AppStats,
    rows: Vec<AppRow>,
    updated: String,
    can_manage: bool,
}

pub async fn dashboard(
    state: web::Data<AppState>,
    session: Session,
) -> Result<HttpResponse, AppError> {
    let user_id = session
        .get::<i64>(SESSION_USER_ID)
        .map_err(|err| AppError::Internal(err.to_string()))?
        .ok_or_else(|| AppError::Internal("session has no user id".into()))?;
    let repo = SqliteUsersRepo::new(state.db.clone());
    let user = repo
        .find_by_id(user_id)
        .await?
        .ok_or_else(|| AppError::Internal("session user no longer exists".into()))?;

    let snapshot = state.snapshot.ensure_loaded().await?;
    let instance = SqliteInstanceSettingsRepo::new(state.db.clone())
        .load()
        .await?;
    let mut data = dashboard_from_snapshot_filtered(&snapshot, |name| instance.app_is_hidden(name));
    let destroying = state.action_runs.active_destruction_subjects().await?;
    for row in &mut data.rows {
        row.deleting = destroying.iter().any(|subject| subject == &row.name);
    }
    let updated = format_age(snapshot.age());

    let page = DashboardPage {
        email: user.email,
        csrf_token: ensure_csrf(&session).await?,
        flash: take_flash(&session),
        stats: data.stats,
        rows: data.rows,
        updated,
        can_manage: user.role.can_manage_apps(),
    };
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(page.render()?))
}

#[derive(Deserialize)]
pub struct RefreshForm {}

/// Manual full refresh from the dashboard. Runs the cheap ps/apps pass across
/// every app, so it can take a few seconds on a busy host.
pub async fn refresh_now(
    state: web::Data<AppState>,
    session: Session,
    _form: CsrfForm<RefreshForm>,
) -> Result<HttpResponse, AppError> {
    match state.snapshot.refresh().await {
        Ok(_) => set_flash(&session, FlashLevel::Success, "Data refreshed."),
        Err(err) => set_flash(
            &session,
            FlashLevel::Error,
            format!("Failed to refresh: {err}"),
        ),
    }
    Ok(see_other("/"))
}

#[cfg(test)]
mod tests {
    use crate::dokku::AppRow;
    use crate::domain::types::{AppHealth, PsReport};

    fn report(running: bool, deployed: bool) -> PsReport {
        PsReport {
            deployed,
            running,
            process_count: 1,
            processes: Vec::new(),
            can_scale: None,
        }
    }

    #[test]
    fn health_labels_are_distinct() {
        let mut labels = vec![
            AppHealth::Running.label(),
            AppHealth::Stopped.label(),
            AppHealth::NotDeployed.label(),
            AppHealth::Unknown.label(),
        ];
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), 4);
    }

    #[test]
    fn health_badge_classes_are_tailwind_utilities() {
        for health in [
            AppHealth::Running,
            AppHealth::Stopped,
            AppHealth::NotDeployed,
            AppHealth::Unknown,
        ] {
            let css = health.badge_css();
            assert!(css.contains("bg-"), "{health:?}: {css}");
            assert!(css.contains("text-"), "{health:?}: {css}");
        }
        assert!(AppHealth::Running.badge_css().contains("emerald"));
        assert!(AppHealth::Stopped.badge_css().contains("red"));
    }

    #[test]
    fn process_label_formats_counts() {
        let row = |count| AppRow {
            name: "a".into(),
            health: AppHealth::from_report(Some(&report(true, true))),
            process_count: count,
            deleting: false,
        };
        assert_eq!(row(-1).process_label(), "—");
        assert_eq!(row(0).process_label(), "0");
        assert_eq!(row(4).process_label(), "4");
    }
}
