use std::collections::HashMap;

use actix_session::Session;
use actix_web::{HttpRequest, HttpResponse, web};
use askama::Template;
use serde::Deserialize;

use crate::dokku::{app_mounts, storage_entries, volume_usage};
use crate::domain::AppName;
use crate::domain::command::DokkuCommand;
use crate::domain::mount_spec::MountSpec;
use crate::domain::types::{AppMounts, StorageEntry};
use crate::error::AppError;
use crate::web::csrf_form::{CsrfForm, ensure_csrf};
use crate::web::flash::{FlashLevel, FlashMessage, set_flash, take_flash};
use crate::web::fragments::{
    RunCompletion, RunRefresh, current_user, error_fragment, is_htmx, modal_error, start_action_run,
};
use crate::web::render::{render, see_other};
use crate::web::state::AppState;

const LIST_PARTIAL_URL: &str = "/volumes/partials/list";

#[derive(Template)]
#[template(path = "volumes/list.html")]
struct VolumesPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
}

/// One mount row with its storage-entry mapping resolved (the entry name is
/// what the lazy usage fragment runs `storage:exec` against).
struct MountView {
    host: String,
    container: String,
    options: String,
    phases_label: String,
    locator: String,
    entry: Option<String>,
}

struct AppMountsView {
    app: String,
    mounts: Vec<MountView>,
}

impl AppMountsView {
    fn from_mounts(app: &AppMounts, entries: &[StorageEntry]) -> Self {
        Self {
            app: app.app.clone(),
            mounts: app
                .mounts
                .iter()
                .map(|mount| MountView {
                    host: mount.host.clone(),
                    container: mount.container.clone(),
                    options: mount.options.clone(),
                    phases_label: mount.phases_label(),
                    locator: mount.locator(),
                    entry: entries
                        .iter()
                        .find(|entry| entry.host_path == mount.host)
                        .map(|entry| entry.name.clone()),
                })
                .collect(),
        }
    }

    fn mount_count(&self) -> usize {
        self.mounts.len()
    }
}

#[derive(Template)]
#[template(path = "volumes/partials/list.html")]
struct MountsPartial<'a> {
    csrf_token: &'a str,
    mounted_apps: Vec<AppMountsView>,
    mountable_apps: Vec<String>,
    total_mounts: usize,
}

#[derive(Template)]
#[template(path = "volumes/partials/usage.html")]
struct UsagePartial {
    available: bool,
    used_label: String,
    fs_label: String,
}

#[derive(Template)]
#[template(path = "volumes/partials/disk-summary.html")]
struct DiskSummaryPartial {
    available: bool,
    used_label: String,
    total_label: String,
    avail_label: String,
    percent: Option<u32>,
}

pub async fn index(state: web::Data<AppState>, session: Session) -> Result<HttpResponse, AppError> {
    let user = current_user(&state, &session).await?;
    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);

    render(&VolumesPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
    })
}

pub async fn list_partial(
    state: web::Data<AppState>,
    session: Session,
) -> Result<HttpResponse, AppError> {
    current_user(&state, &session).await?;

    let apps = match app_mounts(&*state.dokku).await {
        Ok(apps) => apps,
        Err(err) => return error_fragment(LIST_PARTIAL_URL, &err.to_string()),
    };
    let total_mounts = apps.iter().map(AppMounts::mount_count).sum();

    // Entry names are needed for the per-row usage fragments; a failure just
    // leaves those cells as em dashes.
    let entries = storage_entries(&*state.dokku).await;
    let mounted_apps: Vec<AppMountsView> = apps
        .iter()
        .filter(|app| !app.is_empty())
        .map(|app| AppMountsView::from_mounts(app, &entries))
        .collect();

    // The mount form picks from the app snapshot; a snapshot failure degrades
    // to "no apps available" rather than taking the whole panel down.
    let mountable_apps = state
        .snapshot
        .ensure_loaded()
        .await
        .map(|snapshot| snapshot.apps.clone())
        .unwrap_or_default();

    let csrf_token = ensure_csrf(&session).await?;
    render(&MountsPartial {
        csrf_token: &csrf_token,
        mounted_apps,
        mountable_apps,
        total_mounts,
    })
}

/// Lazy per-row disk usage: `storage:exec` on the entry's throwaway container.
pub async fn usage_partial(
    state: web::Data<AppState>,
    session: Session,
    query: web::Query<HashMap<String, String>>,
) -> Result<HttpResponse, AppError> {
    current_user(&state, &session).await?;

    let entry = query.get("entry").map(String::as_str).unwrap_or_default();
    if !valid_entry_name(entry) {
        return error_fragment(LIST_PARTIAL_URL, "Unknown storage entry.");
    }

    match volume_usage(&*state.dokku, entry).await {
        Ok(Some(usage)) => render(&UsagePartial {
            available: true,
            used_label: usage.used_label(),
            fs_label: usage.fs_label(),
        }),
        // Per-row failures degrade to an em dash rather than a retry card:
        // the figure is minor and the page is already usable.
        Ok(None) | Err(_) => render(&UsagePartial {
            available: false,
            used_label: "—".into(),
            fs_label: String::new(),
        }),
    }
}

/// Host-filesystem summary for the Volumes page, computed via the first
/// registered entry (all docker-local entries share the host's data disk).
pub async fn disk_summary_partial(
    state: web::Data<AppState>,
    session: Session,
) -> Result<HttpResponse, AppError> {
    current_user(&state, &session).await?;

    let entries = storage_entries(&*state.dokku).await;
    let usage = match entries.first() {
        Some(entry) => volume_usage(&*state.dokku, &entry.name)
            .await
            .ok()
            .flatten(),
        None => None,
    };

    match usage {
        Some(usage) => render(&DiskSummaryPartial {
            available: true,
            used_label: usage.fs_used_label(),
            total_label: usage.fs_total_label(),
            avail_label: usage.fs_avail_label(),
            percent: usage.fs_percent_rounded(),
        }),
        None => render(&DiskSummaryPartial {
            available: false,
            used_label: "—".into(),
            total_label: "—".into(),
            avail_label: "—".into(),
            percent: None,
        }),
    }
}

/// Storage entry names are DNS-1123-ish (`legacy-<hex>` on docker-local);
/// anything else is a tampered query string and never reaches dokku.
fn valid_entry_name(entry: &str) -> bool {
    !entry.is_empty()
        && entry.len() <= 64
        && entry
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[derive(Deserialize)]
pub struct MountForm {
    app: String,
    host: String,
    container: String,
    options: String,
}

pub async fn mount(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    form: CsrfForm<MountForm>,
) -> Result<HttpResponse, AppError> {
    let app = match AppName::try_from(form.0.app.trim()) {
        Ok(app) => app,
        Err(err) => return form_error(&session, &req, format!("Invalid app name: {err}")),
    };

    let host = form.0.host.trim();
    let container = form.0.container.trim();
    let options = form.0.options.trim();
    let spec = if options.is_empty() {
        format!("{host}:{container}")
    } else {
        format!("{host}:{container}:{options}")
    };
    let mount = match MountSpec::try_from(spec.as_str()) {
        Ok(mount) => mount,
        Err(err) => return form_error(&session, &req, err.to_string()),
    };

    if is_htmx(&req) {
        return start_action_run(
            &state,
            app.as_str(),
            format!("Mounting volume into {app}…"),
            DokkuCommand::StorageMount {
                app: app.clone(),
                mount: mount.clone(),
            },
            RunCompletion {
                success_message: format!(
                    "Mounted '{}' — restart {app} for the change to take effect.",
                    mount.arg()
                ),
                redirect: None,
                refresh: RunRefresh::None,
            },
            Some(LIST_PARTIAL_URL.to_owned()),
        )
        .await;
    }

    match state
        .dokku
        .exec(&DokkuCommand::StorageMount {
            app: app.clone(),
            mount: mount.clone(),
        })
        .await
    {
        Ok(_) => set_flash(
            &session,
            FlashLevel::Success,
            format!(
                "Mounted '{}' — restart {app} for the change to take effect.",
                mount.arg()
            ),
        ),
        Err(err) => set_flash(
            &session,
            FlashLevel::Error,
            format!("Failed to mount volume: {err}"),
        ),
    }
    Ok(see_other("/volumes"))
}

#[derive(Deserialize)]
pub struct UnmountForm {
    app: String,
    spec: String,
}

pub async fn unmount(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    form: CsrfForm<UnmountForm>,
) -> Result<HttpResponse, AppError> {
    let app = match AppName::try_from(form.0.app.trim()) {
        Ok(app) => app,
        Err(err) => return form_error(&session, &req, format!("Invalid app name: {err}")),
    };
    let mount = match MountSpec::try_from(form.0.spec.trim()) {
        Ok(mount) => mount,
        Err(err) => return form_error(&session, &req, err.to_string()),
    };

    if is_htmx(&req) {
        return start_action_run(
            &state,
            app.as_str(),
            format!("Unmounting volume from {app}…"),
            DokkuCommand::StorageUnmount {
                app: app.clone(),
                mount: mount.clone(),
            },
            RunCompletion {
                success_message: format!(
                    "Unmounted '{}' — restart {app} for the change to take effect.",
                    mount.locator()
                ),
                redirect: None,
                refresh: RunRefresh::None,
            },
            Some(LIST_PARTIAL_URL.to_owned()),
        )
        .await;
    }

    match state
        .dokku
        .exec(&DokkuCommand::StorageUnmount {
            app: app.clone(),
            mount: mount.clone(),
        })
        .await
    {
        Ok(_) => set_flash(
            &session,
            FlashLevel::Success,
            format!(
                "Unmounted '{}' — restart {app} for the change to take effect.",
                mount.locator()
            ),
        ),
        Err(err) => set_flash(
            &session,
            FlashLevel::Error,
            format!("Failed to unmount volume: {err}"),
        ),
    }
    Ok(see_other("/volumes"))
}

/// Validation errors surface in the modal for htmx requests, as a flash for
/// plain form posts. No dokku command runs for either.
fn form_error(
    session: &Session,
    req: &HttpRequest,
    message: String,
) -> Result<HttpResponse, AppError> {
    if is_htmx(req) {
        return modal_error(message);
    }
    set_flash(session, FlashLevel::Error, message);
    Ok(see_other("/volumes"))
}
