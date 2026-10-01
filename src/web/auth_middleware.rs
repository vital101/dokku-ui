use actix_session::Session;
use actix_web::body::BoxBody;
use actix_web::dev::{Payload, ServiceRequest, ServiceResponse};
use actix_web::http::header::LOCATION;
use actix_web::middleware::Next;
use actix_web::{Error, FromRequest, HttpResponse, web};

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
    let authenticated = session.get::<i64>(SESSION_USER_ID).ok().flatten().is_some();
    if authenticated {
        return next.call(req).await;
    }

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
        ] {
            assert!(is_public_path(path), "{path} should be public");
        }
    }

    #[test]
    fn private_paths_are_identified() {
        for path in ["/", "/apps", "/apps/myapp", "/logout", "/static.txt"] {
            assert!(!is_public_path(path), "{path} should be private");
        }
    }
}
