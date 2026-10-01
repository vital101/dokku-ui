use actix_session::Session;
use actix_web::{HttpResponse, web};
use askama::Template;

use crate::dokku::{AppRow, DashboardError, dashboard_data};
use crate::domain::types::{AppHealth, AppStats};
use crate::error::AppError;
use crate::storage::users::{SqliteUsersRepo, UsersRepo};
use crate::web::auth_middleware::SESSION_USER_ID;
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
}

impl AppHealth {
    pub fn label(self) -> &'static str {
        match self {
            AppHealth::Running => "Running",
            AppHealth::Stopped => "Stopped",
            AppHealth::NotDeployed => "Not deployed",
            AppHealth::Unknown => "Unknown",
        }
    }

    pub fn badge_css(self) -> &'static str {
        match self {
            AppHealth::Running => "bg-emerald-500/10 text-emerald-400",
            AppHealth::Stopped => "bg-red-500/10 text-red-400",
            AppHealth::NotDeployed => "bg-slate-500/10 text-slate-400",
            AppHealth::Unknown => "bg-slate-500/10 text-slate-400",
        }
    }
}

impl AppRow {
    pub fn process_label(&self) -> String {
        match self.process_count {
            -1 => "—".to_owned(),
            count => count.to_string(),
        }
    }
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

    let data = dashboard_data(state.dokku.as_ref())
        .await
        .map_err(|err| match err {
            DashboardError::List(dokku_err) => AppError::Dokku(dokku_err),
            DashboardError::Parse(parse_err) => AppError::Internal(parse_err.to_string()),
        })?;

    let page = DashboardPage {
        email: user.email,
        csrf_token: session
            .get::<String>("csrf_token")
            .map_err(|err| AppError::Internal(err.to_string()))?
            .unwrap_or_default(),
        flash: take_flash(&session),
        stats: data.stats,
        rows: data.rows,
    };
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(page.render()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dokku::AppRow;
    use crate::domain::types::PsReport;

    fn report(running: bool, deployed: bool) -> PsReport {
        PsReport {
            deployed,
            running,
            process_count: 1,
            processes: Vec::new(),
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
