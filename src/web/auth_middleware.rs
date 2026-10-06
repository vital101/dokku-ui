use actix_session::Session;
use actix_web::body::BoxBody;
use actix_web::dev::{Payload, ServiceRequest, ServiceResponse};
use actix_web::http::header::LOCATION;
use actix_web::middleware::Next;
use actix_web::{Error, FromRequest, HttpResponse, ResponseError, web};

use crate::auth::rbac::{authorize, permission_for};
use crate::error::AppError;
use crate::storage::users::{SqliteUsersRepo, UsersRepo};
use crate::web::state::AppState;

pub const SESSION_USER_ID: &str = "user_id";

pub async fn auth_middleware(
    req: ServiceRequest,
    next: Next<BoxBody>,
) -> Result<ServiceResponse<BoxBody>, Error> {
    if is_public_path(req.path()) {
        return next.call(req).await;
    }

    let session = Session::from_request(req.request(), &mut Payload::None).await?;
    let user_id = session
        .get::<i64>(SESSION_USER_ID)
        .map_err(actix_web::error::ErrorInternalServerError)?;

    let Some(user_id) = user_id else {
        return redirect_to_login(req).await;
    };

    let repo = {
        let state = req
            .app_data::<web::Data<AppState>>()
            .expect("app state registered");
        SqliteUsersRepo::new(state.db.clone())
    };
    let user = match repo.find_by_id(user_id).await {
        Ok(Some(user)) => user,
        // A session for a user that no longer exists is dropped rather than
        // rendering a 500 from every handler's `current_user`.
        Ok(None) => {
            session.purge();
            return redirect_to_login(req).await;
        }
        Err(err) => return Err(actix_web::error::ErrorInternalServerError(err)),
    };

    let permission = permission_for(req.method().as_str(), req.path());
    if !authorize(user.role, permission) {
        let path = req.path().to_owned();
        let is_htmx = req.headers().contains_key("HX-Request");
        tracing::warn!(
            path = %path,
            email = %user.email,
            role = user.role.as_str(),
            "permission denied"
        );
        // htmx does not swap 4xx/5xx responses, so the clicked button would
        // spin forever; fragments stay HTTP 200 (see `error_fragment`).
        if is_htmx {
            return match super::fragments::error_fragment(
                &path,
                "You do not have permission to perform this action.",
            ) {
                Ok(response) => Ok(req.into_response(response)),
                Err(err) => Err(err.into()),
            };
        }
        return Ok(req.into_response(AppError::Forbidden.error_response()));
    }

    next.call(req).await
}

/// Sends unauthenticated users to the first-run wizard or the login page.
async fn redirect_to_login(req: ServiceRequest) -> Result<ServiceResponse<BoxBody>, Error> {
    let redirect = {
        let state = req
            .app_data::<web::Data<AppState>>()
            .expect("app state registered");
        let repo = SqliteUsersRepo::new(state.db.clone());
        match repo.count().await {
            Ok(0) => "/setup",
            Ok(_) => "/login",
            Err(err) => return Err(actix_web::error::ErrorInternalServerError(err)),
        }
    };
    let response = HttpResponse::TemporaryRedirect()
        .insert_header((LOCATION, redirect))
        .finish();
    Ok(req.into_response(response))
}

fn is_public_path(path: &str) -> bool {
    path == "/healthz"
        || path == "/login"
        || path == "/setup"
        || path == "/favicon.ico"
        || path.starts_with("/static/")
        || path.starts_with("/webhooks/")
        || path.starts_with("/reset/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_paths_are_identified() {
        for path in [
            "/healthz",
            "/login",
            "/setup",
            "/favicon.ico",
            "/static/css/app.css",
            "/webhooks/github/myapp",
            "/reset/aabbccdd",
        ] {
            assert!(is_public_path(path), "{path} should be public");
        }
    }

    #[test]
    fn private_paths_are_identified() {
        for path in [
            "/",
            "/apps",
            "/apps/myapp",
            "/logout",
            "/static.txt",
            "/webhooks",
            "/reset",
            "/xwebhooks/github/myapp",
        ] {
            assert!(!is_public_path(path), "{path} should be private");
        }
    }
}
