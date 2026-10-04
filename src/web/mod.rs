mod apps;
mod auth_handlers;
mod auth_middleware;
mod csrf_form;
mod favicon;
mod flash;
mod fragments;
mod pages;
mod render;
mod runs;
mod security_headers;
mod services;
mod state;
mod volumes;

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
        .wrap(actix_web::middleware::from_fn(
            security_headers::security_headers_middleware,
        ))
        .service(actix_files::Files::new("/static", "./static"))
        .route("/healthz", web::get().to(healthz))
        .route("/favicon.ico", web::get().to(favicon::favicon))
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
        .route("/refresh", web::post().to(pages::refresh_now))
        .route("/apps/new", web::get().to(apps::new_form))
        .route("/apps", web::post().to(apps::create))
        .route("/apps/{name}", web::get().to(apps::show))
        .route("/apps/{name}/processes", web::get().to(apps::processes))
        .route("/apps/{name}/scale", web::post().to(apps::scale))
        .route("/apps/{name}/services", web::get().to(apps::services))
        .route("/apps/{name}/config", web::get().to(apps::config))
        .route("/apps/{name}/logs", web::get().to(apps::logs))
        .route(
            "/apps/{name}/partials/overview",
            web::get().to(apps::overview_partial),
        )
        .route(
            "/apps/{name}/partials/processes",
            web::get().to(apps::processes_partial),
        )
        .route(
            "/apps/{name}/partials/services",
            web::get().to(apps::services_partial),
        )
        .route(
            "/apps/{name}/partials/config",
            web::get().to(apps::config_partial),
        )
        .route(
            "/apps/{name}/partials/logs",
            web::get().to(apps::logs_partial),
        )
        .route(
            "/apps/{name}/partials/delete-confirm",
            web::get().to(apps::delete_confirm_modal),
        )
        .route(
            "/actions/runs/{id}/events",
            web::get().to(runs::action_events),
        )
        .service(
            web::resource("/apps/{name}/delete")
                .route(web::get().to(apps::delete_confirm))
                .route(web::post().to(apps::destroy)),
        )
        .route("/apps/{name}/start", web::post().to(apps::start))
        .route("/apps/{name}/stop", web::post().to(apps::stop))
        .route("/apps/{name}/restart", web::post().to(apps::restart))
        .route("/apps/{name}/rebuild", web::post().to(apps::rebuild))
        .service(
            web::resource("/services/{plugin}")
                .route(web::get().to(services::index))
                .route(web::post().to(services::create)),
        )
        .route(
            "/services/{plugin}/partials/list",
            web::get().to(services::list_partial),
        )
        .route("/services/{plugin}/new", web::get().to(services::new_form))
        .route(
            "/services/{plugin}/{service}",
            web::get().to(services::show),
        )
        .route(
            "/services/{plugin}/{service}/links",
            web::get().to(services::links_page),
        )
        .route(
            "/services/{plugin}/{service}/logs",
            web::get().to(services::logs_page),
        )
        .route(
            "/services/{plugin}/{service}/delete",
            web::get().to(services::delete_confirm),
        )
        .route(
            "/services/{plugin}/{service}/partials/overview",
            web::get().to(services::overview_partial),
        )
        .route(
            "/services/{plugin}/{service}/partials/links",
            web::get().to(services::links_partial),
        )
        .route(
            "/services/{plugin}/{service}/partials/logs",
            web::get().to(services::logs_partial),
        )
        .route(
            "/services/{plugin}/{service}/partials/stats",
            web::get().to(services::stats_partial),
        )
        .route(
            "/services/{plugin}/{service}/partials/delete-confirm",
            web::get().to(services::delete_confirm_modal),
        )
        .route(
            "/services/{plugin}/{service}/start",
            web::post().to(services::start),
        )
        .route(
            "/services/{plugin}/{service}/stop",
            web::post().to(services::stop),
        )
        .route(
            "/services/{plugin}/{service}/restart",
            web::post().to(services::restart),
        )
        .route(
            "/services/{plugin}/{service}/destroy",
            web::post().to(services::destroy),
        )
        .route(
            "/services/{plugin}/{service}/expose",
            web::post().to(services::expose),
        )
        .route(
            "/services/{plugin}/{service}/unexpose",
            web::post().to(services::unexpose),
        )
        .route(
            "/services/{plugin}/{service}/link",
            web::post().to(services::link),
        )
        .route(
            "/services/{plugin}/{service}/unlink",
            web::post().to(services::unlink),
        )
        .route("/volumes", web::get().to(volumes::index))
        .route(
            "/volumes/partials/list",
            web::get().to(volumes::list_partial),
        )
        .route(
            "/volumes/partials/usage",
            web::get().to(volumes::usage_partial),
        )
        .route(
            "/volumes/partials/disk-summary",
            web::get().to(volumes::disk_summary_partial),
        )
        .route("/volumes/mount", web::post().to(volumes::mount))
        .route("/volumes/unmount", web::post().to(volumes::unmount))
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
