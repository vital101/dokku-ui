use std::collections::HashMap;

use actix_session::Session;
use actix_web::{HttpResponse, web};
use askama::Template;

use crate::domain::service_plugin::ServicePlugin;
use crate::error::AppError;
use crate::storage::runs::{RunSummary, TargetKind};
use crate::web::csrf_form::ensure_csrf;
use crate::web::flash::{FlashMessage, take_flash};
use crate::web::fragments::current_user;
use crate::web::render::render;
use crate::web::services::detail_context;
use crate::web::state::AppState;

const ACTIVITY_LIMIT: usize = 100;

#[derive(Template)]
#[template(path = "activity.html")]
struct ActivityPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    rows: Vec<RunSummary>,
    user_filter: Option<&'a str>,
}

#[derive(Template)]
#[template(path = "apps/activity.html")]
struct AppActivityPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    name: &'a str,
    active_tab: &'static str,
    rows: Vec<RunSummary>,
}

#[derive(Template)]
#[template(path = "services/activity.html")]
struct ServiceActivityPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    plugin: ServicePlugin,
    service: &'a str,
    active_tab: &'static str,
    rows: Vec<RunSummary>,
}

/// Global audit trail with an optional per-user filter. Reads only the shared
/// `action_runs` table, so any container answers identically.
pub async fn index(
    state: web::Data<AppState>,
    session: Session,
    query: web::Query<HashMap<String, String>>,
) -> Result<HttpResponse, AppError> {
    let user = current_user(&state, &session).await?;
    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);

    let filter: Option<String> = query
        .get("user")
        .map(String::as_str)
        .map(|email| email.trim().to_owned())
        .filter(|email| !email.is_empty());
    let rows = match &filter {
        Some(email) => {
            state
                .action_runs
                .list_for_actor_email(email, ACTIVITY_LIMIT)
                .await?
        }
        None => state.action_runs.list_recent(ACTIVITY_LIMIT).await?,
    };

    let page = ActivityPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        rows,
        user_filter: filter.as_deref(),
    };
    render(&page)
}

/// The audit trail for one app (its own tab).
pub async fn app_activity(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let user = current_user(&state, &session).await?;
    state.snapshot.resolve_app(&name).await?;

    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);
    let rows = state
        .action_runs
        .list_for_target(TargetKind::App, &name, ACTIVITY_LIMIT)
        .await?;
    let page = AppActivityPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        name: &name,
        active_tab: "activity",
        rows,
    };
    render(&page)
}

/// The audit trail for one service (its own tab).
pub async fn service_activity(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<(String, String)>,
) -> Result<HttpResponse, AppError> {
    let (raw_plugin, raw_service) = path.into_inner();
    let ctx = detail_context(&state, &session, &raw_plugin, &raw_service).await?;
    let rows = state
        .action_runs
        .list_for_target(TargetKind::Service, ctx.service.as_str(), ACTIVITY_LIMIT)
        .await?;
    let page = ServiceActivityPage {
        email: &ctx.email,
        csrf_token: &ctx.csrf_token,
        flash: ctx.flash.as_ref(),
        plugin: ctx.plugin,
        service: ctx.service.as_str(),
        active_tab: "activity",
        rows,
    };
    render(&page)
}
