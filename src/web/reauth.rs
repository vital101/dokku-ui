use actix_session::Session;
use actix_web::{HttpResponse, web};
use askama::Template;
use serde::Deserialize;

use crate::auth::password::verify_password;
use crate::auth::reauth::REAUTH_UNTIL;
use crate::domain::Password;
use crate::error::AppError;
use crate::web::auth_handlers::safe_next;
use crate::web::csrf_form::{CsrfForm, ensure_csrf};
use crate::web::flash::{FlashLevel, FlashMessage, set_flash, take_flash};
use crate::web::fragments::{current_user, is_htmx};
use crate::web::render::{render, see_other};
use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "auth/reauth.html")]
struct ReauthPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    next: &'a str,
    error: Option<&'a str>,
}

#[derive(Template)]
#[template(path = "partials/reauth.html")]
struct ReauthPartial<'a> {
    csrf_token: &'a str,
    next: &'a str,
    error: Option<&'a str>,
}

#[derive(Deserialize)]
pub struct ReauthQuery {
    #[serde(default)]
    next: String,
}

#[derive(Deserialize)]
pub struct ReauthForm {
    password: String,
    #[serde(default)]
    next: String,
}

/// Prompts for the password again before sensitive actions (config reveal).
/// Renders a full page, or an htmx modal fragment when requested.
pub async fn reauth_form(
    state: web::Data<AppState>,
    session: Session,
    req: actix_web::HttpRequest,
    query: web::Query<ReauthQuery>,
) -> Result<HttpResponse, AppError> {
    let user = current_user(&state, &session).await?;
    let csrf_token = ensure_csrf(&session).await?;
    let next = safe_next(&query.next);
    let flash = take_flash(&session);
    if is_htmx(&req) {
        return render(&ReauthPartial {
            csrf_token: &csrf_token,
            next: &next,
            error: None,
        });
    }
    let page = ReauthPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        next: &next,
        error: None,
    };
    render(&page)
}

/// Verifies the password and stamps `reauth_until` on the session, then
/// redirects (or, for htmx, renders a success fragment the caller swaps in).
pub async fn reauth_submit(
    state: web::Data<AppState>,
    session: Session,
    req: actix_web::HttpRequest,
    form: CsrfForm<ReauthForm>,
) -> Result<HttpResponse, AppError> {
    let form = form.0;
    let user = current_user(&state, &session).await?;
    let next = safe_next(&form.next);

    let password = Password::new(&form.password).ok();
    let valid = match password {
        Some(password) => verify_password(&password, &user.password_hash)?,
        None => false,
    };
    if !valid {
        let csrf_token = ensure_csrf(&session).await?;
        let error = "Invalid password.";
        if is_htmx(&req) {
            return render(&ReauthPartial {
                csrf_token: &csrf_token,
                next: &next,
                error: Some(error),
            });
        }
        let page = ReauthPage {
            email: &user.email,
            csrf_token: &csrf_token,
            flash: None,
            next: &next,
            error: Some(error),
        };
        return render(&page);
    }

    let until =
        time::OffsetDateTime::now_utc().unix_timestamp() + state.settings.reauth_ttl_secs as i64;
    session
        .insert(REAUTH_UNTIL, until)
        .map_err(|err| AppError::Internal(err.to_string()))?;
    set_flash(&session, FlashLevel::Success, "Verified — you can proceed.");
    Ok(see_other(&next))
}
