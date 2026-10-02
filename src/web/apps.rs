use actix_session::Session;
use actix_web::{HttpResponse, web};
use askama::Template;
use serde::Deserialize;

use crate::dokku::{OverviewError, app_overview};
use crate::domain::AppName;
use crate::domain::command::DokkuCommand;
use crate::domain::types::AppInfo;
use crate::error::AppError;
use crate::storage::users::{SqliteUsersRepo, UsersRepo};
use crate::web::auth_middleware::SESSION_USER_ID;
use crate::web::csrf_form::{CsrfForm, ensure_csrf};
use crate::web::flash::{FlashLevel, FlashMessage, set_flash, take_flash};
use crate::web::render::{render, see_other};
use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "apps/new.html")]
struct NewFormPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
}

#[derive(Deserialize)]
pub struct CreateForm {
    name: String,
}

#[derive(Template)]
#[template(path = "apps/show.html")]
struct ShowPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    name: &'a str,
    health_label: &'static str,
    health_css: &'static str,
    process_label: String,
    deployed: bool,
    created_at: &'a str,
    locked_label: &'static str,
    image_status_label: &'static str,
    link_exists_label: &'static str,
    dns_record_exists_label: &'static str,
}

#[derive(Template)]
#[template(path = "apps/delete_confirm.html")]
struct DeleteConfirmPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    name: &'a str,
}

async fn current_user(
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

pub async fn new_form(
    state: web::Data<AppState>,
    session: Session,
) -> Result<HttpResponse, AppError> {
    let user = current_user(&state, &session).await?;
    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);

    let page = NewFormPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
    };
    render(&page)
}

pub async fn create(
    state: web::Data<AppState>,
    session: Session,
    form: CsrfForm<CreateForm>,
) -> Result<HttpResponse, AppError> {
    let name = form.0.name.trim().to_owned();

    let app = match AppName::try_from(name.clone()) {
        Ok(app) => app,
        Err(err) => {
            set_flash(
                &session,
                FlashLevel::Error,
                format!("Invalid app name: {err}"),
            );
            return Ok(see_other("/apps/new"));
        }
    };

    match state.dokku.exec(&DokkuCommand::AppsCreate { app }).await {
        Ok(_) => {
            set_flash(
                &session,
                FlashLevel::Success,
                format!("App '{}' created.", name),
            );
            Ok(see_other(&format!("/apps/{}", name)))
        }
        Err(err) => {
            set_flash(
                &session,
                FlashLevel::Error,
                format!("Failed to create app: {err}"),
            );
            Ok(see_other("/apps/new"))
        }
    }
}

pub async fn show(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let user = current_user(&state, &session).await?;

    let overview = app_overview(&*state.dokku, &name)
        .await
        .map_err(|err| match err {
            OverviewError::List(dokku_err) => AppError::Dokku(dokku_err),
            OverviewError::ParseList(parse_err) => AppError::Internal(parse_err.to_string()),
            OverviewError::AppNotFound(_) => AppError::NotFound,
            OverviewError::Report(dokku_err) => AppError::Dokku(dokku_err),
        })?;

    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);
    let process_label = match overview.process_count {
        -1 => "—".to_owned(),
        count => count.to_string(),
    };
    let app_info = overview.app_info.as_ref();
    let page = ShowPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        name: &name,
        health_label: overview.health.label(),
        health_css: overview.health.badge_css(),
        process_label,
        deployed: overview
            .ps_report
            .as_ref()
            .map(|r| r.deployed)
            .unwrap_or(false),
        created_at: app_info.map(|a| a.created_at.as_str()).unwrap_or("unknown"),
        locked_label: app_info.map(AppInfo::locked_label).unwrap_or("unknown"),
        image_status_label: app_info
            .map(AppInfo::image_status_label)
            .unwrap_or("unknown"),
        link_exists_label: app_info
            .map(AppInfo::link_exists_label)
            .unwrap_or("unknown"),
        dns_record_exists_label: app_info
            .map(AppInfo::dns_record_exists_label)
            .unwrap_or("unknown"),
    };

    render(&page)
}

pub async fn delete_confirm(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let user = current_user(&state, &session).await?;
    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);

    let page = DeleteConfirmPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        name: &name,
    };

    render(&page)
}

pub async fn destroy(
    state: web::Data<AppState>,
    session: Session,
    form: CsrfForm<DestroyForm>,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();

    let app = match AppName::try_from(name.clone()) {
        Ok(app) => app,
        Err(err) => {
            set_flash(
                &session,
                FlashLevel::Error,
                format!("Invalid app name: {err}"),
            );
            return Ok(see_other("/"));
        }
    };

    if form.0.name.trim() != name {
        set_flash(
            &session,
            FlashLevel::Error,
            format!("Type `{name}` to confirm deletion."),
        );
        return Ok(see_other(&format!("/apps/{}/delete", name)));
    }

    match state
        .dokku
        .exec(&DokkuCommand::AppsDestroy { app, force: true })
        .await
    {
        Ok(_) => {
            set_flash(
                &session,
                FlashLevel::Success,
                format!("App '{}' destroyed.", name),
            );
            Ok(see_other("/"))
        }
        Err(err) => {
            set_flash(
                &session,
                FlashLevel::Error,
                format!("Failed to destroy app: {err}"),
            );
            Ok(see_other(&format!("/apps/{}/delete", name)))
        }
    }
}

#[derive(Deserialize)]
pub struct DestroyForm {
    name: String,
}
