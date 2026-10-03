use actix_session::Session;
use actix_web::{HttpResponse, web};
use askama::Template;

use crate::dokku::{AppRow, dashboard_from_snapshot, format_age};
use crate::domain::types::AppStats;
use crate::error::AppError;
use crate::storage::users::{SqliteUsersRepo, UsersRepo};
use crate::web::auth_middleware::SESSION_USER_ID;
use crate::web::csrf_form::ensure_csrf;
use crate::web::flash::{FlashMessage, take_flash};
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
    let data = dashboard_from_snapshot(&snapshot);
    let updated = format_age(snapshot.age());

    let page = DashboardPage {
        email: user.email,
        csrf_token: ensure_csrf(&session).await?,
        flash: take_flash(&session),
        stats: data.stats,
        rows: data.rows,
        updated,
    };
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(page.render()?))
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
        };
        assert_eq!(row(-1).process_label(), "—");
        assert_eq!(row(0).process_label(), "0");
        assert_eq!(row(4).process_label(), "4");
    }
}
