use actix_session::Session;
use actix_web::{HttpResponse, web};
use askama::Template;
use serde::Deserialize;

use crate::auth::csrf::generate_token;
use crate::auth::password::{hash_password, verify_password};
use crate::auth::rbac::{Role, can_delete_user, can_set_role};
use crate::domain::{Email, Password, RESET_TTL_SECS, hash_token, reset_url};
use crate::error::AppError;
use crate::storage::instance_settings::{InstanceSettingsRepo, SqliteInstanceSettingsRepo};
use crate::storage::password_resets::{PasswordResetsRepo, SqlitePasswordResetsRepo};
use crate::storage::sessions::SqliteSessionStore;
use crate::storage::users::{SqliteUsersRepo, UsersRepo};
use crate::web::csrf_form::{CsrfForm, ensure_csrf};
use crate::web::flash::{FlashLevel, FlashMessage, set_flash, take_flash};
use crate::web::fragments::current_user;
use crate::web::render::{render, see_other};
use crate::web::state::AppState;

/// One row of the user table, with the hash kept out of the template context.
struct UserRow {
    id: i64,
    email: String,
    role: Role,
    created_label: String,
}

#[derive(Template)]
#[template(path = "users/list.html")]
struct UsersPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    users: Vec<UserRow>,
    current_id: i64,
    roles: [Role; 3],
    error: Option<&'a str>,
}

#[derive(Template)]
#[template(path = "partials/admin_nav.html")]
struct AdminNavPartial {
    is_admin: bool,
}

#[derive(Template)]
#[template(path = "users/reset_link.html")]
struct ResetLinkPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    target_email: &'a str,
    link: &'a str,
}

#[derive(Template)]
#[template(path = "auth/password.html")]
struct PasswordPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    error: Option<&'a str>,
}

#[derive(Deserialize)]
pub struct CreateUserForm {
    email: String,
    role: String,
    password: String,
    confirm: String,
}

#[derive(Deserialize)]
pub struct SetRoleForm {
    role: String,
}

#[derive(Deserialize)]
pub struct DeleteUserForm {}

#[derive(Deserialize)]
pub struct ResetLinkForm {}

#[derive(Deserialize)]
pub struct ChangePasswordForm {
    current_password: String,
    password: String,
    confirm: String,
}

fn format_date(unix: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(unix)
        .map(|dt| dt.date().to_string())
        .unwrap_or_else(|_| "—".to_owned())
}

/// Renders the sidebar Users link only for admins. Served as an htmx partial
/// so the shared page chrome does not need every page context to carry the
/// current user's role.
pub async fn admin_nav(
    state: web::Data<AppState>,
    session: Session,
) -> Result<HttpResponse, AppError> {
    let user = current_user(&state, &session).await?;
    render(&AdminNavPartial {
        is_admin: user.role.is_admin(),
    })
}

async fn load_rows(state: &AppState) -> Result<Vec<UserRow>, AppError> {
    let repo = SqliteUsersRepo::new(state.db.clone());
    Ok(repo
        .list()
        .await?
        .into_iter()
        .map(|user| UserRow {
            id: user.id,
            email: user.email,
            role: user.role,
            created_label: format_date(user.created_at),
        })
        .collect())
}

async fn render_users_page(
    state: &AppState,
    session: &Session,
    email: &str,
    error: Option<&str>,
) -> Result<HttpResponse, AppError> {
    let csrf_token = ensure_csrf(session).await?;
    let flash = take_flash(session);
    let page = UsersPage {
        email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        users: load_rows(state).await?,
        current_id: current_user(state, session).await?.id,
        roles: Role::all(),
        error,
    };
    render(&page)
}

pub async fn index(state: web::Data<AppState>, session: Session) -> Result<HttpResponse, AppError> {
    let user = current_user(&state, &session).await?;
    render_users_page(&state, &session, &user.email, None).await
}

pub async fn create(
    state: web::Data<AppState>,
    session: Session,
    form: CsrfForm<CreateUserForm>,
) -> Result<HttpResponse, AppError> {
    let user = current_user(&state, &session).await?;
    let form = form.0;

    let email = match Email::try_from(form.email.trim()) {
        Ok(email) => email,
        Err(_) => {
            return render_users_page(
                &state,
                &session,
                &user.email,
                Some("Enter a valid email address."),
            )
            .await;
        }
    };
    let password = match Password::new(&form.password) {
        Ok(password) => password,
        Err(err) => {
            let message = err.to_string();
            return render_users_page(&state, &session, &user.email, Some(&message)).await;
        }
    };
    if form.password != form.confirm {
        return render_users_page(
            &state,
            &session,
            &user.email,
            Some("Passwords do not match."),
        )
        .await;
    }
    let Some(role) = Role::try_from(form.role.as_str()) else {
        return render_users_page(&state, &session, &user.email, Some("Pick a valid role.")).await;
    };

    let repo = SqliteUsersRepo::new(state.db.clone());
    if repo.find_by_email(email.as_str()).await?.is_some() {
        return render_users_page(
            &state,
            &session,
            &user.email,
            Some("A user with that email already exists."),
        )
        .await;
    }

    let hash = hash_password(&password)?;
    if let Err(err) = repo.insert(email.as_str(), &hash, role).await {
        // A concurrent create with the same email surfaces here as a unique
        // violation; degrade to the friendly duplicate message.
        if repo.find_by_email(email.as_str()).await?.is_some() {
            return render_users_page(
                &state,
                &session,
                &user.email,
                Some("A user with that email already exists."),
            )
            .await;
        }
        return Err(err.into());
    }
    set_flash(
        &session,
        FlashLevel::Success,
        format!("User '{}' created.", email.as_str()),
    );
    Ok(see_other("/users"))
}

pub async fn set_role(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<i64>,
    form: CsrfForm<SetRoleForm>,
) -> Result<HttpResponse, AppError> {
    let id = path.into_inner();
    let repo = SqliteUsersRepo::new(state.db.clone());
    let target = repo.find_by_id(id).await?.ok_or(AppError::NotFound)?;
    let Some(new_role) = Role::try_from(form.0.role.as_str()) else {
        set_flash(&session, FlashLevel::Error, "Pick a valid role.");
        return Ok(see_other("/users"));
    };

    let admins = repo.count_admins().await?;
    if target.role == new_role {
        set_flash(
            &session,
            FlashLevel::Success,
            format!("{} is already {}.", target.email, new_role.label()),
        );
        return Ok(see_other("/users"));
    }
    if !can_set_role(target.role, new_role, admins) {
        set_flash(
            &session,
            FlashLevel::Error,
            "The last admin cannot be demoted. Promote another admin first.",
        );
        return Ok(see_other("/users"));
    }

    // The guard is re-checked atomically inside the statement, so a concurrent
    // demote/delete can never leave zero admins.
    if !repo.update_role_guarded(id, new_role).await? {
        set_flash(
            &session,
            FlashLevel::Error,
            "The last admin cannot be demoted. Promote another admin first.",
        );
        return Ok(see_other("/users"));
    }
    set_flash(
        &session,
        FlashLevel::Success,
        format!("{} is now {}.", target.email, new_role.label()),
    );
    Ok(see_other("/users"))
}

pub async fn delete(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<i64>,
    _form: CsrfForm<DeleteUserForm>,
) -> Result<HttpResponse, AppError> {
    let id = path.into_inner();
    let user = current_user(&state, &session).await?;
    if user.id == id {
        set_flash(
            &session,
            FlashLevel::Error,
            "You cannot delete your own account.",
        );
        return Ok(see_other("/users"));
    }

    let repo = SqliteUsersRepo::new(state.db.clone());
    let target = repo.find_by_id(id).await?.ok_or(AppError::NotFound)?;
    let admins = repo.count_admins().await?;
    if !can_delete_user(target.role, admins) {
        set_flash(
            &session,
            FlashLevel::Error,
            "The last admin cannot be deleted. Promote another admin first.",
        );
        return Ok(see_other("/users"));
    }

    // The guard is re-checked atomically inside the statement.
    if !repo.delete_guarded(id).await? {
        set_flash(
            &session,
            FlashLevel::Error,
            "The last admin cannot be deleted. Promote another admin first.",
        );
        return Ok(see_other("/users"));
    }
    set_flash(
        &session,
        FlashLevel::Success,
        format!("User '{}' deleted.", target.email),
    );
    Ok(see_other("/users"))
}

/// Generates a one-time password reset link for a user. The plaintext token is
/// rendered once and never stored; only its hash lives in SQLite.
pub async fn reset_link(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<i64>,
    _form: CsrfForm<ResetLinkForm>,
) -> Result<HttpResponse, AppError> {
    let id = path.into_inner();
    let user = current_user(&state, &session).await?;
    let target = SqliteUsersRepo::new(state.db.clone())
        .find_by_id(id)
        .await?
        .ok_or(AppError::NotFound)?;

    let settings = SqliteInstanceSettingsRepo::new(state.db.clone())
        .load()
        .await?;
    let Some(public_url) = settings.public_url else {
        set_flash(
            &session,
            FlashLevel::Error,
            "Set a public URL in instance settings before generating reset links.",
        );
        return Ok(see_other("/users"));
    };

    let token = generate_token();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    SqlitePasswordResetsRepo::new(state.db.clone())
        .create(target.id, &hash_token(&token), now, now + RESET_TTL_SECS)
        .await?;
    // A reset is the recovery path for a compromised account: drop every
    // existing session for the target so a copied cookie stops working.
    SqliteSessionStore::new(state.db.clone())
        .delete_user_sessions(target.id)
        .await?;

    let link = reset_url(&public_url, &token);
    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);
    let page = ResetLinkPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        target_email: &target.email,
        link: &link,
    };
    // The link is a one-time bearer credential; keep it out of caches.
    let mut response = render(&page)?;
    response.headers_mut().insert(
        actix_web::http::header::CACHE_CONTROL,
        actix_web::http::header::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}

pub async fn password_form(
    state: web::Data<AppState>,
    session: Session,
) -> Result<HttpResponse, AppError> {
    let user = current_user(&state, &session).await?;
    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);
    let page = PasswordPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        error: None,
    };
    render(&page)
}

pub async fn password_submit(
    state: web::Data<AppState>,
    session: Session,
    form: CsrfForm<ChangePasswordForm>,
) -> Result<HttpResponse, AppError> {
    let user = current_user(&state, &session).await?;
    let form = form.0;
    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);

    let render_error = |error: &str| {
        let page = PasswordPage {
            email: &user.email,
            csrf_token: &csrf_token,
            flash: flash.as_ref(),
            error: Some(error),
        };
        render(&page)
    };

    let current = match Password::new(&form.current_password) {
        Ok(password) => password,
        Err(_) => return render_error("Enter your current password."),
    };
    if !verify_password(&current, &user.password_hash)? {
        return render_error("Current password is incorrect.");
    }
    let new_password = match Password::new(&form.password) {
        Ok(password) => password,
        Err(err) => {
            let message = err.to_string();
            return render_error(&message);
        }
    };
    if form.password != form.confirm {
        return render_error("Passwords do not match.");
    }

    let hash = hash_password(&new_password)?;
    let repo = SqliteUsersRepo::new(state.db.clone());
    repo.update_password_hash(user.id, &hash).await?;
    // Sign out everywhere: the current cookie is revoked with the rest, and a
    // fresh session carries the flash to the sign-in page.
    SqliteSessionStore::new(state.db.clone())
        .delete_user_sessions(user.id)
        .await?;
    session.renew();
    set_flash(
        &session,
        FlashLevel::Success,
        "Password updated. Sign in again with your new password.",
    );
    Ok(see_other("/login"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_render_as_utc_days() {
        assert_eq!(format_date(0), "1970-01-01");
        assert_eq!(format_date(1_700_000_000), "2023-11-14");
        assert_eq!(format_date(i64::MAX), "—");
    }

    #[test]
    fn role_helpers_drive_visibility() {
        assert!(Role::Admin.is_admin());
        assert!(!Role::Operator.is_admin());
        assert!(!Role::Viewer.is_admin());
    }
}
