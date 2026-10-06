mod activity;
mod apps;
mod auth_handlers;
mod auth_middleware;
mod build;
mod cron;
mod csrf_form;
mod deploy;
mod domains;
mod favicon;
mod flash;
mod fragments;
mod instance_settings;
mod logs;
mod pages;
mod palette;
mod reauth;
mod render;
mod runs;
mod security_headers;
mod services;
mod state;
mod tls;
mod toasts;
mod users;
mod volumes;
mod webhooks;

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
        .service(
            web::resource("/reauth")
                .route(web::get().to(reauth::reauth_form))
                .route(web::post().to(reauth::reauth_submit)),
        )
        .route("/", web::get().to(pages::dashboard))
        .route("/refresh", web::post().to(pages::refresh_now))
        .route("/activity", web::get().to(activity::index))
        .route("/palette.json", web::get().to(palette::palette))
        .route("/actions/toasts", web::get().to(toasts::tray))
        .route("/actions/runs/{id}/ack", web::post().to(toasts::ack))
        .service(
            web::resource("/users")
                .route(web::get().to(users::index))
                .route(web::post().to(users::create)),
        )
        .route("/users/{id}/role", web::post().to(users::set_role))
        .route("/users/{id}/delete", web::post().to(users::delete))
        .route("/partials/admin-nav", web::get().to(users::admin_nav))
        .service(
            web::resource("/settings")
                .route(web::get().to(instance_settings::form))
                .route(web::post().to(instance_settings::save)),
        )
        .route(
            "/partials/service-nav",
            web::get().to(services::service_nav),
        )
        .service(
            web::resource("/password")
                .route(web::get().to(users::password_form))
                .route(web::post().to(users::password_submit)),
        )
        .route("/apps/new", web::get().to(apps::new_form))
        .route("/apps", web::post().to(apps::create))
        .route("/apps/{name}", web::get().to(apps::show))
        .route("/apps/{name}/processes", web::get().to(apps::processes))
        .route("/apps/{name}/scale", web::post().to(apps::scale))
        .route(
            "/apps/{name}/resources",
            web::post().to(apps::update_resources),
        )
        .route("/apps/{name}/services", web::get().to(apps::services))
        .route(
            "/apps/{name}/maintenance/enable",
            web::post().to(apps::maintenance_enable),
        )
        .route(
            "/apps/{name}/maintenance/disable",
            web::post().to(apps::maintenance_disable),
        )
        .route(
            "/apps/{name}/http-auth/enable",
            web::post().to(apps::http_auth_enable),
        )
        .route(
            "/apps/{name}/http-auth/disable",
            web::post().to(apps::http_auth_disable),
        )
        .route(
            "/apps/{name}/http-auth/add-user",
            web::post().to(apps::http_auth_add_user),
        )
        .route(
            "/apps/{name}/http-auth/remove-user",
            web::post().to(apps::http_auth_remove_user),
        )
        .route("/apps/{name}/build", web::get().to(build::page))
        .route("/apps/{name}/partials/build", web::get().to(build::partial))
        .route(
            "/apps/{name}/build/buildpacks/add",
            web::post().to(build::buildpacks_add),
        )
        .route(
            "/apps/{name}/build/buildpacks/set",
            web::post().to(build::buildpacks_set),
        )
        .route(
            "/apps/{name}/build/buildpacks/remove",
            web::post().to(build::buildpacks_remove),
        )
        .route(
            "/apps/{name}/build/buildpacks/clear",
            web::post().to(build::buildpacks_clear),
        )
        .route(
            "/apps/{name}/build/builder",
            web::post().to(build::builder_set),
        )
        .route("/apps/{name}/deploy", web::get().to(deploy::page))
        .route(
            "/apps/{name}/partials/deploy",
            web::get().to(deploy::partial),
        )
        .route(
            "/apps/{name}/partials/failed-logs",
            web::get().to(deploy::failed_logs_partial),
        )
        .route(
            "/apps/{name}/deploy/branch",
            web::post().to(deploy::set_branch),
        )
        .route("/apps/{name}/deploy/sync", web::post().to(deploy::sync))
        .route(
            "/apps/{name}/deploy/from-image",
            web::post().to(deploy::from_image),
        )
        .route(
            "/apps/{name}/deploy/from-archive",
            web::post().to(deploy::from_archive),
        )
        .route("/apps/{name}/webhooks", web::post().to(webhooks::save))
        .route(
            "/apps/{name}/webhooks/remove",
            web::post().to(webhooks::remove),
        )
        .route(
            "/apps/{name}/webhooks/reveal",
            web::post().to(webhooks::reveal),
        )
        .route("/webhooks/github/{app}", web::post().to(webhooks::receive))
        .route("/apps/{name}/tls", web::get().to(tls::page))
        .route("/apps/{name}/partials/tls", web::get().to(tls::partial))
        .route("/apps/{name}/tls/enable", web::post().to(tls::enable))
        .route("/apps/{name}/tls/disable", web::post().to(tls::disable))
        .route("/apps/{name}/tls/revoke", web::post().to(tls::revoke))
        .route("/apps/{name}/tls/cleanup", web::post().to(tls::cleanup))
        .route(
            "/apps/{name}/tls/cron-job/add",
            web::post().to(tls::cron_job_add),
        )
        .route(
            "/apps/{name}/tls/cron-job/remove",
            web::post().to(tls::cron_job_remove),
        )
        .route("/apps/{name}/cron", web::get().to(cron::page))
        .route("/apps/{name}/partials/cron", web::get().to(cron::partial))
        .route("/apps/{name}/cron/run", web::post().to(cron::run))
        .route("/apps/{name}/cron/suspend", web::post().to(cron::suspend))
        .route("/apps/{name}/cron/resume", web::post().to(cron::resume))
        .route("/apps/{name}/domains", web::get().to(domains::page))
        .route(
            "/apps/{name}/partials/domains",
            web::get().to(domains::partial),
        )
        .route("/apps/{name}/domains/add", web::post().to(domains::add))
        .route(
            "/apps/{name}/domains/remove",
            web::post().to(domains::remove),
        )
        .route("/apps/{name}/domains/set", web::post().to(domains::set))
        .route("/apps/{name}/settings", web::get().to(apps::settings))
        .route(
            "/apps/{name}/partials/settings",
            web::get().to(apps::settings_partial),
        )
        .route("/apps/{name}/lock", web::post().to(apps::lock))
        .route("/apps/{name}/unlock", web::post().to(apps::unlock))
        .route("/apps/{name}/rename", web::post().to(apps::rename))
        .route("/apps/{name}/config", web::get().to(apps::config))
        .route("/apps/{name}/config/edit", web::get().to(apps::config_edit))
        .route(
            "/apps/{name}/config/reveal",
            web::post().to(apps::config_reveal),
        )
        .route("/apps/{name}/config", web::post().to(apps::config_update))
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
        .route("/apps/{name}/logs/stream", web::get().to(logs::log_stream))
        .route(
            "/apps/{name}/activity",
            web::get().to(activity::app_activity),
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
            "/services/{plugin}/{service}/activity",
            web::get().to(activity::service_activity),
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
