use actix_session::Session;
use actix_web::{HttpResponse, web};
use askama::Template;
use serde::Deserialize;

use crate::auth::password::hash_password;
use crate::domain::{Password, hash_token, is_plausible_token};
use crate::error::AppError;
use crate::storage::password_resets::{PasswordResetsRepo, SqlitePasswordResetsRepo};
use crate::storage::sessions::SqliteSessionStore;
use crate::storage::users::{SqliteUsersRepo, UsersRepo};
use crate::web::csrf_form::{CsrfForm, ensure_csrf};
use crate::web::flash::{FlashLevel, set_flash};
use crate::web::render::{render, see_other};
use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "auth/reset.html")]
struct ResetPage<'a> {
    token: &'a str,
    csrf_token: &'a str,
    error: Option<&'a str>,
}

#[derive(Deserialize)]
pub struct ResetForm {
    password: String,
    confirm: String,
}

pub async fn form(session: Session, path: web::Path<String>) -> Result<HttpResponse, AppError> {
    let token = path.into_inner();
    if !is_plausible_token(&token) {
        return Err(AppError::NotFound);
    }
    let csrf_token = ensure_csrf(&session).await?;
    render(&ResetPage {
        token: &token,
        csrf_token: &csrf_token,
        error: None,
    })
}

pub async fn submit(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
    form: CsrfForm<ResetForm>,
) -> Result<HttpResponse, AppError> {
    let token = path.into_inner();
    if !is_plausible_token(&token) {
        return Err(AppError::NotFound);
    }
    let csrf_token = ensure_csrf(&session).await?;
    let form = form.0;

    let password = match Password::new(&form.password) {
        Ok(password) => password,
        Err(err) => {
            let message = err.to_string();
            return render(&ResetPage {
                token: &token,
                csrf_token: &csrf_token,
                error: Some(&message),
            });
        }
    };
    if form.password != form.confirm {
        return render(&ResetPage {
            token: &token,
            csrf_token: &csrf_token,
            error: Some("Passwords do not match."),
        });
    }

    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let repo = SqlitePasswordResetsRepo::new(state.db.clone());
    let Some(user_id) = repo.consume(&hash_token(&token), now).await? else {
        return render(&ResetPage {
            token: &token,
            csrf_token: &csrf_token,
            error: Some("This reset link is invalid or has expired. Ask an admin for a new one."),
        });
    };

    let hash = hash_password(&password)?;
    SqliteUsersRepo::new(state.db.clone())
        .update_password_hash(user_id, &hash)
        .await?;
    SqliteSessionStore::new(state.db.clone())
        .delete_user_sessions(user_id)
        .await?;
    session.renew();
    set_flash(
        &session,
        FlashLevel::Success,
        "Password updated. Sign in with your new password.",
    );
    Ok(see_other("/login"))
}
