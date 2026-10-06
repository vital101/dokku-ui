use actix_session::Session;
use actix_web::{HttpResponse, web};
use askama::Template;
use serde::Deserialize;

use crate::domain::InstanceSettings;
use crate::error::AppError;
use crate::storage::instance_settings::{InstanceSettingsRepo, SqliteInstanceSettingsRepo};
use crate::web::csrf_form::{CsrfForm, ensure_csrf};
use crate::web::flash::{FlashLevel, FlashMessage, set_flash, take_flash};
use crate::web::fragments::current_user;
use crate::web::render::{render, see_other};
use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "settings.html")]
struct SettingsPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    error: Option<&'a str>,
    public_url: String,
    login_banner: String,
    app_filter: String,
    activity_ttl: String,
    run_log_ttl: String,
    activity_ttl_default: u64,
    run_log_ttl_default: u64,
}

#[derive(Deserialize)]
pub struct SettingsForm {
    #[serde(default)]
    public_url: String,
    #[serde(default)]
    login_banner: String,
    #[serde(default)]
    app_filter: String,
    #[serde(default)]
    activity_ttl: String,
    #[serde(default)]
    run_log_ttl: String,
}

async fn load_settings(state: &AppState) -> Result<InstanceSettings, AppError> {
    // A read failure propagates: rendering a blank form would invite a save
    // that wipes the stored settings.
    SqliteInstanceSettingsRepo::new(state.db.clone())
        .load()
        .await
        .map_err(AppError::Database)
}

struct SettingsValues {
    public_url: String,
    login_banner: String,
    app_filter: String,
    activity_ttl: String,
    run_log_ttl: String,
}

impl SettingsValues {
    fn from_settings(settings: &InstanceSettings) -> Self {
        Self {
            public_url: settings.public_url.clone().unwrap_or_default(),
            login_banner: settings.login_banner.clone().unwrap_or_default(),
            app_filter: settings.app_filter.join("\n"),
            activity_ttl: settings
                .activity_ttl_secs
                .map(|ttl| ttl.to_string())
                .unwrap_or_default(),
            run_log_ttl: settings
                .run_log_ttl_secs
                .map(|ttl| ttl.to_string())
                .unwrap_or_default(),
        }
    }
}

fn page_from_values<'a>(
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    error: Option<&'a str>,
    values: SettingsValues,
    state: &AppState,
) -> SettingsPage<'a> {
    SettingsPage {
        email,
        csrf_token,
        flash,
        error,
        public_url: values.public_url,
        login_banner: values.login_banner,
        app_filter: values.app_filter,
        activity_ttl: values.activity_ttl,
        run_log_ttl: values.run_log_ttl,
        activity_ttl_default: state.settings.activity_ttl_secs,
        run_log_ttl_default: state.settings.run_log_ttl_secs,
    }
}

pub async fn form(state: web::Data<AppState>, session: Session) -> Result<HttpResponse, AppError> {
    let user = current_user(&state, &session).await?;
    let settings = load_settings(&state).await?;
    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);
    let page = page_from_values(
        &user.email,
        &csrf_token,
        flash.as_ref(),
        None,
        SettingsValues::from_settings(&settings),
        &state,
    );
    render(&page)
}

pub async fn save(
    state: web::Data<AppState>,
    session: Session,
    form: CsrfForm<SettingsForm>,
) -> Result<HttpResponse, AppError> {
    let user = current_user(&state, &session).await?;
    let form = form.0;

    match InstanceSettings::parse(
        Some(&form.public_url),
        Some(&form.login_banner),
        Some(&form.app_filter),
        Some(&form.activity_ttl),
        Some(&form.run_log_ttl),
    ) {
        Ok(settings) => {
            SqliteInstanceSettingsRepo::new(state.db.clone())
                .save(&settings)
                .await?;
            set_flash(&session, FlashLevel::Success, "Instance settings saved.");
            Ok(see_other("/settings"))
        }
        Err(err) => {
            let csrf_token = ensure_csrf(&session).await?;
            let flash = take_flash(&session);
            let message = err.to_string();
            let page = page_from_values(
                &user.email,
                &csrf_token,
                flash.as_ref(),
                Some(&message),
                SettingsValues {
                    public_url: form.public_url,
                    login_banner: form.login_banner,
                    app_filter: form.app_filter,
                    activity_ttl: form.activity_ttl,
                    run_log_ttl: form.run_log_ttl,
                },
                &state,
            );
            render(&page)
        }
    }
}
