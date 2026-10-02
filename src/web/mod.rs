mod apps;
mod auth_handlers;
mod auth_middleware;
mod csrf_form;
mod flash;
mod pages;
mod render;
mod state;

use actix_session::SessionMiddleware;
use actix_session::config::{PersistentSession, TtlExtensionPolicy};
use actix_web::body::MessageBody;
use actix_web::cookie::{Key, SameSite};
use actix_web::dev::{ServiceFactory, ServiceRequest, ServiceResponse};
use actix_web::{App, Error, HttpResponse, web};

use crate::error::AppError;
use crate::storage::sessions::SqliteSessionStore;

pub use state::AppState;

const SESSION_COOKIE_NAME: &str = "dokku-ui-session";

const HEALTHZ_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Dokku UI</title>
  <link rel="stylesheet" href="/static/css/app.css">
</head>
<body class="flex min-h-screen items-center justify-center bg-slate-950">
  <div class="text-center">
    <h1 class="text-2xl font-semibold text-white">Dokku UI</h1>
    <p class="mt-4 inline-flex items-center gap-2 rounded-full bg-emerald-500/10 px-4 py-1.5 text-sm font-medium text-emerald-400">
      <span class="size-2 rounded-full bg-emerald-400"></span>
      healthy
    </p>
  </div>
</body>
</html>"#;

pub fn build_app(
    state: AppState,
) -> App<
    impl ServiceFactory<
        ServiceRequest,
        Config = (),
        Response = ServiceResponse<impl MessageBody>,
        Error = Error,
        InitError = (),
    >,
> {
    let key = Key::derive_from(state.settings.secret_key.as_bytes());
    let session_store = SqliteSessionStore::new(state.db.clone());
    let session_ttl =
        time::Duration::seconds(i64::try_from(state.settings.session_ttl_secs).unwrap_or(i64::MAX));
    let cookie_secure = state.settings.cookie_secure;

    App::new()
        .app_data(web::Data::new(state))
        .wrap(actix_web::middleware::from_fn(
            auth_middleware::auth_middleware,
        ))
        .wrap(
            SessionMiddleware::builder(session_store, key)
                .cookie_name(SESSION_COOKIE_NAME.to_owned())
                .cookie_http_only(true)
                .cookie_same_site(SameSite::Lax)
                .cookie_secure(cookie_secure)
                .session_lifecycle(
                    PersistentSession::default()
                        .session_ttl(session_ttl)
                        .session_ttl_extension_policy(TtlExtensionPolicy::OnEveryRequest),
                )
                .build(),
        )
        .service(actix_files::Files::new("/static", "./static"))
        .route("/healthz", web::get().to(healthz))
        .service(
            web::resource("/setup")
                .route(web::get().to(auth_handlers::setup_form))
                .route(web::post().to(auth_handlers::setup_submit)),
        )
        .service(
            web::resource("/login")
                .route(web::get().to(auth_handlers::login_form))
                .route(web::post().to(auth_handlers::login_submit)),
        )
        .route("/logout", web::post().to(auth_handlers::logout))
        .route("/", web::get().to(pages::dashboard))
        .route("/apps/new", web::get().to(apps::new_form))
        .route("/apps", web::post().to(apps::create))
        .route("/apps/{name}", web::get().to(apps::show))
        .service(
            web::resource("/apps/{name}/delete")
                .route(web::get().to(apps::delete_confirm))
                .route(web::post().to(apps::destroy)),
        )
        .default_service(web::to(not_found))
}

async fn healthz(state: web::Data<AppState>) -> HttpResponse {
    match sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(&state.db)
        .await
    {
        Ok(_) => HttpResponse::Ok()
            .content_type("text/html; charset=utf-8")
            .body(HEALTHZ_HTML),
        Err(err) => {
            tracing::error!(error = %err, "healthz database check failed");
            HttpResponse::ServiceUnavailable().finish()
        }
    }
}

async fn not_found() -> Result<HttpResponse, AppError> {
    Err(AppError::NotFound)
}
