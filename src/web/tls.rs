use actix_session::Session;
use actix_web::{HttpRequest, HttpResponse, web};
use askama::Template;
use serde::Deserialize;

use crate::dokku::format_age;
use crate::domain::AppName;
use crate::domain::capabilities::Support;
use crate::domain::command::DokkuCommand;
use crate::domain::job::JobSpec;
use crate::domain::parse::{parse_certs_report, parse_letsencrypt_active, parse_letsencrypt_list};
use crate::domain::tls::{LetsencryptAction, is_valid_letsencrypt_email};
use crate::domain::types::{LetsencryptEntry, SslReport};
use crate::error::AppError;
use crate::storage::runs::TargetKind;
use crate::web::csrf_form::{CsrfForm, ensure_csrf};
use crate::web::flash::{FlashLevel, FlashMessage, set_flash, take_flash};
use crate::web::fragments::{
    RunCompletion, RunRefresh, RunRequest, current_user, enqueue_action_run, error_fragment,
    is_htmx, modal_error, start_action_run,
};
use crate::web::render::{render, see_other};
use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "apps/tls.html")]
struct TlsPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    name: &'a str,
    active_tab: &'static str,
}

#[derive(Template)]
#[template(path = "apps/partials/tls.html")]
struct TlsPartial<'a> {
    name: &'a str,
    csrf_token: &'a str,
    letsencrypt_available: bool,
    letsencrypt_label: String,
    active: bool,
    entry: Option<LetsencryptEntry>,
    ssl: Option<SslReport>,
    updated: String,
    can_manage: bool,
}

fn partial_url(name: &str) -> String {
    format!("/apps/{name}/partials/tls")
}

async fn letsencrypt_support(state: &AppState) -> Support {
    state
        .capabilities
        .current()
        .await
        .map(|caps| caps.supports_plugin("letsencrypt"))
        .unwrap_or(Support::Unknown)
}

pub async fn page(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let user = current_user(&state, &session).await?;
    state.snapshot.resolve_app(&name).await?;

    let csrf_token = ensure_csrf(&session).await?;
    let flash = take_flash(&session);
    let page = TlsPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        name: &name,
        active_tab: "tls",
    };
    render(&page)
}

pub async fn partial(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    let user = current_user(&state, &session).await?;

    let retry_url = partial_url(&name);
    let (snapshot, app) = match state.snapshot.resolve_app(&name).await {
        Ok(resolved) => resolved,
        Err(err) => {
            let message = if err.is_not_found() {
                format!("App '{name}' was not found — it may have been deleted.")
            } else {
                err.to_string()
            };
            return error_fragment(&retry_url, &message);
        }
    };
    // Certificate status is core dokku and always available.
    let ssl = match state
        .dokku
        .exec(&DokkuCommand::CertsReport {
            app: Some(app.clone()),
        })
        .await
    {
        Ok(output) => parse_certs_report(&output.stdout),
        Err(err) => return error_fragment(&retry_url, &err.to_string()),
    };

    let support = letsencrypt_support(&state).await;
    let available = support == Support::Supported;
    let (active, entry) = if available {
        let active = state
            .dokku
            .exec(&DokkuCommand::LetsencryptActive { app: app.clone() })
            .await
            .map(|output| parse_letsencrypt_active(&output.stdout))
            .unwrap_or(false);
        let entry = state
            .dokku
            .exec(&DokkuCommand::LetsencryptList)
            .await
            .ok()
            .and_then(|output| {
                parse_letsencrypt_list(&output.stdout)
                    .into_iter()
                    .find(|entry| entry.app == name)
            });
        (active, entry)
    } else {
        (false, None)
    };

    let csrf_token = ensure_csrf(&session).await?;
    render(&TlsPartial {
        name: &name,
        csrf_token: &csrf_token,
        letsencrypt_available: available,
        letsencrypt_label: support.label(),
        active,
        entry,
        ssl,
        updated: format_age(snapshot.age()),
        can_manage: user.role.can_manage_apps(),
    })
}

pub async fn enable(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    _form: CsrfForm<super::apps::ActionForm>,
) -> Result<HttpResponse, AppError> {
    action(
        state,
        session,
        req,
        path.into_inner(),
        LetsencryptAction::Enable,
    )
    .await
}

pub async fn disable(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    _form: CsrfForm<super::apps::ActionForm>,
) -> Result<HttpResponse, AppError> {
    action(
        state,
        session,
        req,
        path.into_inner(),
        LetsencryptAction::Disable,
    )
    .await
}

pub async fn revoke(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    _form: CsrfForm<super::apps::ActionForm>,
) -> Result<HttpResponse, AppError> {
    action(
        state,
        session,
        req,
        path.into_inner(),
        LetsencryptAction::Revoke,
    )
    .await
}

pub async fn cleanup(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    _form: CsrfForm<super::apps::ActionForm>,
) -> Result<HttpResponse, AppError> {
    action(
        state,
        session,
        req,
        path.into_inner(),
        LetsencryptAction::Cleanup,
    )
    .await
}

async fn action(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    name: String,
    action: LetsencryptAction,
) -> Result<HttpResponse, AppError> {
    if letsencrypt_support(&state).await != Support::Supported {
        return reject(
            &session,
            &req,
            &name,
            "The letsencrypt plugin is not available on this host.",
        );
    }
    if let Err(err) = AppName::try_from(name.clone()) {
        return reject(&session, &req, &name, &format!("Invalid app name: {err}"));
    }
    let (title, success, queued) = match action {
        LetsencryptAction::Enable => (
            format!("Enabling Let's Encrypt for {name}…"),
            format!("Enabled Let's Encrypt for '{name}'."),
            format!("Queued: enable Let's Encrypt for {name}."),
        ),
        LetsencryptAction::Disable => (
            format!("Disabling Let's Encrypt for {name}…"),
            format!("Disabled Let's Encrypt for '{name}'."),
            format!("Queued: disable Let's Encrypt for {name}."),
        ),
        LetsencryptAction::Revoke => (
            format!("Revoking the certificate for {name}…"),
            format!("Revoked the certificate for '{name}'."),
            format!("Queued: revoke the certificate for {name}."),
        ),
        LetsencryptAction::Cleanup => (
            format!("Cleaning up certificates for {name}…"),
            format!("Cleaned up certificates for '{name}'."),
            format!("Queued: clean up certificates for {name}."),
        ),
    };
    let operation = format!("letsencrypt.{}", action.as_str());
    let plan = vec![JobSpec::LetsencryptAction {
        app: name.clone(),
        action,
    }];
    let completion = RunCompletion {
        success_message: success,
        redirect: None,
        refresh: RunRefresh::None,
    };
    if is_htmx(&req) {
        return start_action_run(
            &state,
            &session,
            &RunRequest {
                subject: name.clone(),
                operation,
                target_kind: TargetKind::App,
                title,
                plan,
                completion,
                redactions: Vec::new(),
                refresh_url: Some(partial_url(&name)),
            },
        )
        .await;
    }
    enqueue_action_run(
        &state,
        &session,
        &name,
        &operation,
        TargetKind::App,
        &plan,
        &completion,
        &[],
    )
    .await?;
    set_flash(&session, FlashLevel::Success, queued);
    Ok(see_other(&format!("/apps/{name}/tls")))
}

pub async fn cron_job_add(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    _form: CsrfForm<super::apps::ActionForm>,
) -> Result<HttpResponse, AppError> {
    cron_job(state, session, req, path.into_inner(), true).await
}

pub async fn cron_job_remove(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    _form: CsrfForm<super::apps::ActionForm>,
) -> Result<HttpResponse, AppError> {
    cron_job(state, session, req, path.into_inner(), false).await
}

async fn cron_job(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    name: String,
    add: bool,
) -> Result<HttpResponse, AppError> {
    if letsencrypt_support(&state).await != Support::Supported {
        return reject(
            &session,
            &req,
            &name,
            "The letsencrypt plugin is not available on this host.",
        );
    }
    // The cron job is server-wide; every container shares the host's crontab.
    let (title, success, queued) = if add {
        (
            "Adding the Let's Encrypt auto-renew cron job…".to_owned(),
            "Added the auto-renew cron job.".to_owned(),
            "Queued: add the auto-renew cron job.".to_owned(),
        )
    } else {
        (
            "Removing the Let's Encrypt auto-renew cron job…".to_owned(),
            "Removed the auto-renew cron job.".to_owned(),
            "Queued: remove the auto-renew cron job.".to_owned(),
        )
    };
    let operation = "letsencrypt.cron-job".to_owned();
    let plan = vec![JobSpec::LetsencryptCronJob { add }];
    let completion = RunCompletion {
        success_message: success,
        redirect: None,
        refresh: RunRefresh::None,
    };
    if is_htmx(&req) {
        return start_action_run(
            &state,
            &session,
            &RunRequest {
                subject: "letsencrypt".to_owned(),
                operation,
                target_kind: TargetKind::System,
                title,
                plan,
                completion,
                redactions: Vec::new(),
                refresh_url: Some(partial_url(&name)),
            },
        )
        .await;
    }
    enqueue_action_run(
        &state,
        &session,
        "letsencrypt",
        &operation,
        TargetKind::System,
        &plan,
        &completion,
        &[],
    )
    .await?;
    set_flash(&session, FlashLevel::Success, queued);
    Ok(see_other(&format!("/apps/{name}/tls")))
}

fn reject(
    session: &Session,
    req: &HttpRequest,
    name: &str,
    message: &str,
) -> Result<HttpResponse, AppError> {
    if is_htmx(req) {
        return modal_error(message);
    }
    set_flash(session, FlashLevel::Error, message.to_owned());
    Ok(see_other(&format!("/apps/{name}/tls")))
}

#[derive(Deserialize)]
pub struct SetForm {
    #[serde(default)]
    email: String,
    #[serde(default)]
    staging: String,
}

/// Applies `letsencrypt:set` for the registration email and/or the staging
/// flag. Fields left blank are not touched; clearing staging is not exposed
/// because the UI only offers explicit true/false.
pub async fn set(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<SetForm>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    if letsencrypt_support(&state).await != Support::Supported {
        return reject(
            &session,
            &req,
            &name,
            "The letsencrypt plugin is not available on this host.",
        );
    }
    if let Err(err) = AppName::try_from(name.clone()) {
        return reject(&session, &req, &name, &format!("Invalid app name: {err}"));
    }
    let form = form.0;
    let email = form.email.trim().to_owned();

    let mut plan = Vec::new();
    if !email.is_empty() {
        if !is_valid_letsencrypt_email(&email) {
            return reject(&session, &req, &name, "Enter a valid email address.");
        }
        plan.push(JobSpec::LetsencryptSet {
            app: name.clone(),
            property: "email".to_owned(),
            value: Some(email),
        });
    }
    match form.staging.as_str() {
        "" => {}
        "true" | "false" => plan.push(JobSpec::LetsencryptSet {
            app: name.clone(),
            property: "staging".to_owned(),
            value: Some(form.staging.clone()),
        }),
        _ => return reject(&session, &req, &name, "Pick a valid staging value."),
    }
    if plan.is_empty() {
        return reject(
            &session,
            &req,
            &name,
            "Provide an email or a staging value to update.",
        );
    }

    let operation = "letsencrypt.set".to_owned();
    let completion = RunCompletion {
        success_message: "Let's Encrypt settings saved.".to_owned(),
        redirect: None,
        refresh: RunRefresh::None,
    };
    if is_htmx(&req) {
        return start_action_run(
            &state,
            &session,
            &RunRequest {
                subject: name.clone(),
                operation,
                target_kind: TargetKind::App,
                title: "Saving Let's Encrypt settings…".to_owned(),
                plan,
                completion,
                redactions: Vec::new(),
                refresh_url: Some(partial_url(&name)),
            },
        )
        .await;
    }
    enqueue_action_run(
        &state,
        &session,
        &name,
        &operation,
        TargetKind::App,
        &plan,
        &completion,
        &[],
    )
    .await?;
    set_flash(
        &session,
        FlashLevel::Success,
        "Queued: save Let's Encrypt settings.",
    );
    Ok(see_other(&format!("/apps/{name}/tls")))
}
