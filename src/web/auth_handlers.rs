use actix_session::Session;
use actix_web::{HttpResponse, web};
use askama::Template;
use serde::Deserialize;

use crate::auth::password::{hash_password, verify_password};
use crate::domain::{Email, Password};
use crate::error::AppError;
use crate::storage::users::{SqliteUsersRepo, UsersRepo};
use crate::web::auth_middleware::SESSION_USER_ID;
use crate::web::csrf_form::{CsrfForm, ensure_csrf};
use crate::web::flash::{FlashLevel, set_flash};
use crate::web::render::{redirect, render, see_other};
use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "auth/login.html")]
struct LoginPage<'a> {
    next: &'a str,
    error: Option<&'a str>,
    csrf_token: &'a str,
}

#[derive(Template)]
#[template(path = "auth/setup.html")]
struct SetupPage<'a> {
    error: Option<&'a str>,
    csrf_token: &'a str,
}

#[derive(Deserialize)]
pub struct LoginQuery {
    #[serde(default)]
    next: String,
}

#[derive(Deserialize)]
pub struct LoginForm {
    email: String,
    password: String,
    #[serde(default)]
    next: String,
}

#[derive(Deserialize)]
pub struct SetupForm {
    email: String,
    password: String,
    confirm: String,
}

fn safe_next(next: &str) -> String {
    if next.starts_with('/') && !next.starts_with("//") {
        next.to_owned()
    } else {
        "/".to_owned()
    }
}

async fn users_count(state: &AppState) -> Result<i64, AppError> {
    let repo = SqliteUsersRepo::new(state.db.clone());
    repo.count().await.map_err(AppError::Database)
}

pub async fn login_form(
    state: web::Data<AppState>,
    session: Session,
    query: web::Query<LoginQuery>,
) -> Result<HttpResponse, AppError> {
    if session
        .get::<i64>(SESSION_USER_ID)
        .map_err(|err| AppError::Internal(err.to_string()))?
        .is_some()
    {
        return Ok(redirect("/"));
    }
    if users_count(&state).await? == 0 {
        return Ok(redirect("/setup"));
    }
    let csrf_token = ensure_csrf(&session).await?;
    let next = safe_next(&query.next);
    let page = LoginPage {
        next: &next,
        error: None,
        csrf_token: &csrf_token,
    };
    render(&page)
}

pub async fn login_submit(
    state: web::Data<AppState>,
    session: Session,
    form: CsrfForm<LoginForm>,
) -> Result<HttpResponse, AppError> {
    let form = form.0;
    let repo = SqliteUsersRepo::new(state.db.clone());
    let user = repo.find_by_email(&form.email).await?;

    let password = Password::new(&form.password).ok();
    let valid = match (&user, password) {
        (Some(user), Some(password)) => verify_password(&password, &user.password_hash)?,
        _ => false,
    };

    if !valid {
        let csrf_token = ensure_csrf(&session).await?;
        let next = safe_next(&form.next);
        let page = LoginPage {
            next: &next,
            error: Some("Invalid email or password"),
            csrf_token: &csrf_token,
        };
        return render(&page);
    }

    let user = user.expect("user present when valid");
    session.renew();
    session
        .insert(SESSION_USER_ID, user.id)
        .map_err(|err| AppError::Internal(err.to_string()))?;
    let _ = ensure_csrf(&session).await?;
    set_flash(
        &session,
        FlashLevel::Success,
        format!("Welcome back, {}", user.email),
    );
    Ok(see_other(&safe_next(&form.next)))
}

pub async fn logout(session: Session) -> Result<HttpResponse, AppError> {
    session.purge();
    Ok(see_other("/login"))
}

pub async fn setup_form(
    state: web::Data<AppState>,
    session: Session,
) -> Result<HttpResponse, AppError> {
    if users_count(&state).await? > 0 {
        return Ok(redirect("/login"));
    }
    let csrf_token = ensure_csrf(&session).await?;
    let page = SetupPage {
        error: None,
        csrf_token: &csrf_token,
    };
    render(&page)
}

pub async fn setup_submit(
    state: web::Data<AppState>,
    session: Session,
    form: CsrfForm<SetupForm>,
) -> Result<HttpResponse, AppError> {
    if users_count(&state).await? > 0 {
        return Ok(redirect("/login"));
    }

    let form = form.0;
    let email_result = Email::try_from(form.email.as_str());
    let password_result = Password::new(&form.password);
    let mismatch = form.password != form.confirm;

    let message = if email_result.is_err() {
        Some("Enter a valid email address".to_owned())
    } else if let Err(err) = &password_result {
        Some(err.to_string())
    } else if mismatch {
        Some("Passwords do not match".to_owned())
    } else {
        None
    };

    if let Some(message) = message {
        let csrf_token = ensure_csrf(&session).await?;
        let page = SetupPage {
            error: Some(&message),
            csrf_token: &csrf_token,
        };
        return render(&page);
    }

    let email = email_result.expect("validated above");
    let password = password_result.expect("validated above");
    let hash = hash_password(&password)?;
    let repo = SqliteUsersRepo::new(state.db.clone());
    let user = repo.insert(email.as_str(), &hash).await?;

    session.renew();
    session
        .insert(SESSION_USER_ID, user.id)
        .map_err(|err| AppError::Internal(err.to_string()))?;
    let _ = ensure_csrf(&session).await?;
    set_flash(
        &session,
        FlashLevel::Success,
        "Setup complete. Welcome to Dokku UI!",
    );
    Ok(see_other("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_next_allows_relative_paths() {
        assert_eq!(safe_next("/apps"), "/apps");
        assert_eq!(safe_next("/"), "/");
    }

    #[test]
    fn safe_next_rejects_external_and_protocol_relative() {
        assert_eq!(safe_next("https://evil.example"), "/");
        assert_eq!(safe_next("//evil.example"), "/");
        assert_eq!(safe_next(""), "/");
    }
}
