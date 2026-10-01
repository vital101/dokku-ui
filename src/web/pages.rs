use actix_session::Session;
use actix_web::{HttpResponse, web};
use askama::Template;

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

    let page = DashboardPage {
        email: user.email,
        csrf_token: session
            .get::<String>("csrf_token")
            .map_err(|err| AppError::Internal(err.to_string()))?
            .unwrap_or_default(),
        flash: take_flash(&session),
    };
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(page.render()?))
}
