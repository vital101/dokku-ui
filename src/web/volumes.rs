use actix_session::Session;
use actix_web::{HttpRequest, HttpResponse, web};
use askama::Template;
use serde::Deserialize;

use crate::dokku::app_mounts;
use crate::domain::AppName;
use crate::domain::command::DokkuCommand;
use crate::domain::mount_spec::MountSpec;
use crate::domain::types::AppMounts;
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

#[derive(Template)]
#[template(path = "volumes/partials/list.html")]
struct MountsPartial<'a> {
    csrf_token: &'a str,
    mounted_apps: Vec<AppMounts>,
    mountable_apps: Vec<String>,
    total_mounts: usize,
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
    let mounted_apps: Vec<AppMounts> = apps.into_iter().filter(|app| !app.is_empty()).collect();

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
