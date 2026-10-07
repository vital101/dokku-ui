use actix_session::Session;
use actix_web::{HttpRequest, HttpResponse, web};
use askama::Template;
use serde::Deserialize;

use crate::dokku::{app_domains, format_age};
use crate::domain::AppName;
use crate::domain::domain_name::{DomainName, parse_domain_list};
use crate::domain::job::JobSpec;
use crate::domain::types::DomainsReport;
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
#[template(path = "apps/domains.html")]
struct DomainsPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    name: &'a str,
    active_tab: &'static str,
}

/// One vhost row with its DNS pre-check result (`None` for wildcard vhosts,
/// which cannot be resolved literally).
struct DomainRow {
    domain: String,
    resolves: Option<bool>,
}

#[derive(Template)]
#[template(path = "apps/partials/domains.html")]
struct DomainsPartial<'a> {
    name: &'a str,
    csrf_token: &'a str,
    report: DomainsReport,
    rows: Vec<DomainRow>,
    current_list: String,
    updated: String,
    can_manage: bool,
}

fn partial_url(name: &str) -> String {
    format!("/apps/{name}/partials/domains")
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
    let page = DomainsPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        name: &name,
        active_tab: "domains",
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
    let report = match app_domains(&*state.dokku, app).await {
        Ok(Some(report)) => report,
        Ok(None) => {
            return error_fragment(&retry_url, "Could not read this app's domains.");
        }
        Err(err) => return error_fragment(&retry_url, &err.to_string()),
    };
    let statuses = state.snapshot.domain_dns_statuses(&report.vhosts).await;
    let rows = statuses
        .into_iter()
        .map(|(domain, resolves)| DomainRow { domain, resolves })
        .collect::<Vec<_>>();
    let current_list = report.vhosts.join(" ");
    let csrf_token = ensure_csrf(&session).await?;
    render(&DomainsPartial {
        name: &name,
        csrf_token: &csrf_token,
        report,
        rows,
        current_list,
        updated: format_age(snapshot.age()),
        can_manage: user.role.can_manage_apps(),
    })
}

#[derive(Deserialize)]
pub struct DomainsForm {
    /// Whitespace/comma separated hostnames.
    domains: String,
}

#[derive(Deserialize)]
pub struct DomainForm {
    domain: String,
}

pub async fn add(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<DomainsForm>,
) -> Result<HttpResponse, AppError> {
    mutate(
        &state,
        &session,
        &req,
        path.into_inner(),
        "add",
        &form.0.domains,
    )
    .await
}

pub async fn remove(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<DomainForm>,
) -> Result<HttpResponse, AppError> {
    mutate(
        &state,
        &session,
        &req,
        path.into_inner(),
        "remove",
        &form.0.domain,
    )
    .await
}

pub async fn set(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<DomainsForm>,
) -> Result<HttpResponse, AppError> {
    mutate(
        &state,
        &session,
        &req,
        path.into_inner(),
        "set",
        &form.0.domains,
    )
    .await
}

async fn mutate(
    state: &AppState,
    session: &Session,
    req: &HttpRequest,
    name: String,
    action: &str,
    raw_domains: &str,
) -> Result<HttpResponse, AppError> {
    if let Err(err) = AppName::try_from(name.clone()) {
        if is_htmx(req) {
            return modal_error(format!("Invalid app name: {err}"));
        }
        set_flash(
            session,
            FlashLevel::Error,
            format!("Invalid app name: {err}"),
        );
        return Ok(see_other("/"));
    }
    let domains = match parse_domain_list(raw_domains) {
        Ok(domains) if !domains.is_empty() => domains,
        Ok(_) => {
            let message = "Enter at least one domain.".to_owned();
            if is_htmx(req) {
                return modal_error(message);
            }
            set_flash(session, FlashLevel::Error, message);
            return Ok(see_other(&format!("/apps/{name}/domains")));
        }
        Err(err) => {
            let message = format!("Invalid domain: {err}");
            if is_htmx(req) {
                return modal_error(message);
            }
            set_flash(session, FlashLevel::Error, message);
            return Ok(see_other(&format!("/apps/{name}/domains")));
        }
    };

    let spec = |domains: Vec<DomainName>| match action {
        "add" => JobSpec::DomainsAdd {
            app: name.clone(),
            domains: domains.iter().map(|d| d.as_str().to_owned()).collect(),
        },
        "remove" => JobSpec::DomainsRemove {
            app: name.clone(),
            domains: domains.iter().map(|d| d.as_str().to_owned()).collect(),
        },
        _ => JobSpec::DomainsSet {
            app: name.clone(),
            domains: domains.iter().map(|d| d.as_str().to_owned()).collect(),
        },
    };
    let verb = match action {
        "add" => "add",
        "remove" => "remove",
        _ => "set",
    };
    let completion = RunCompletion {
        success_message: format!("Domains updated for '{name}'."),
        redirect: None,
        refresh: RunRefresh::None,
    };
    let plan = vec![spec(domains)];
    if is_htmx(req) {
        return start_action_run(
            state,
            session,
            &RunRequest {
                subject: name.clone(),
                operation: format!("domains.{verb}"),
                target_kind: TargetKind::App,
                title: format!("Updating domains for {name}…"),
                plan,
                completion,
                redactions: Vec::new(),
                refresh_url: Some(partial_url(&name)),
            },
        )
        .await;
    }
    let _ = enqueue_action_run(
        state,
        session,
        &name,
        &format!("domains.{verb}"),
        TargetKind::App,
        &plan,
        &completion,
        &[],
    )
    .await?;
    set_flash(
        session,
        FlashLevel::Success,
        format!("Queued: {verb} domains for {name}."),
    );
    Ok(see_other(&format!("/apps/{name}/domains")))
}
