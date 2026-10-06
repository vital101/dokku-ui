use actix_session::Session;
use actix_web::{HttpResponse, web};
use serde::Serialize;

use crate::error::AppError;
use crate::storage::instance_settings::{InstanceSettingsRepo, SqliteInstanceSettingsRepo};
use crate::web::fragments::current_user;
use crate::web::state::AppState;

#[derive(Serialize)]
struct PaletteItem {
    label: String,
    href: String,
    group: &'static str,
}

#[derive(Serialize)]
struct Palette {
    items: Vec<PaletteItem>,
}

fn item(label: impl Into<String>, href: impl Into<String>, group: &'static str) -> PaletteItem {
    PaletteItem {
        label: label.into(),
        href: href.into(),
        group,
    }
}

/// Data source for the Cmd-K palette. Apps come from the shared snapshot
/// (minus the instance app filter); the Users/Settings entries are role-gated
/// here so a viewer never sees them.
pub async fn palette(
    state: web::Data<AppState>,
    session: Session,
) -> Result<HttpResponse, AppError> {
    let user = current_user(&state, &session).await?;
    let snapshot = state.snapshot.ensure_loaded().await?;
    let instance = SqliteInstanceSettingsRepo::new(state.db.clone())
        .load()
        .await
        .unwrap_or_default();

    let mut items = vec![
        item("Dashboard", "/", "Navigate"),
        item("Activity", "/activity", "Navigate"),
        item("Volumes", "/volumes", "Navigate"),
        item("New app", "/apps/new", "Actions"),
        item("Change password", "/password", "Navigate"),
    ];
    if user.role.is_admin() {
        items.push(item("Users", "/users", "Navigate"));
        items.push(item("Settings", "/settings", "Navigate"));
    }
    for app in &snapshot.apps {
        if instance.app_is_hidden(app) {
            continue;
        }
        items.push(item(app.clone(), format!("/apps/{app}"), "Apps"));
    }

    Ok(HttpResponse::Ok()
        .content_type("application/json; charset=utf-8")
        .json(Palette { items }))
}
