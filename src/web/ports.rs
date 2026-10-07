use actix_session::Session;
use actix_web::{HttpRequest, HttpResponse, web};
use askama::Template;
use serde::Deserialize;

use crate::domain::AppName;
use crate::domain::command::DokkuCommand;
use crate::domain::job::JobSpec;
use crate::domain::parse::{parse_ports_report, parse_proxy_report};
use crate::domain::port::parse_port_mappings;
use crate::domain::types::{PortsReport, ProxyReport};
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
#[template(path = "apps/ports.html")]
struct PortsPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    name: &'a str,
    active_tab: &'static str,
}

#[derive(Template)]
#[template(path = "apps/partials/ports.html")]
struct PortsPartial<'a> {
    name: &'a str,
    csrf_token: &'a str,
    ports: PortsReport,
    proxy: ProxyReport,
    can_manage: bool,
}

fn partial_url(name: &str) -> String {
    format!("/apps/{name}/partials/ports")
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
    let page = PortsPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        name: &name,
        active_tab: "ports",
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
    let (_snapshot, app) = match state.snapshot.resolve_app(&name).await {
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
    let ports = match state
        .dokku
        .exec(&DokkuCommand::PortsReport { app: app.clone() })
        .await
    {
        Ok(output) => parse_ports_report(&output.stdout),
        Err(err) => return error_fragment(&retry_url, &err.to_string()),
    };
    // Proxy status is informational; a failure degrades to "unknown" rather
    // than taking the panel down.
    let proxy = state
        .dokku
        .exec(&DokkuCommand::ProxyReport { app })
        .await
        .map(|output| parse_proxy_report(&output.stdout))
        .unwrap_or_default();

    let csrf_token = ensure_csrf(&session).await?;
    render(&PortsPartial {
        name: &name,
        csrf_token: &csrf_token,
        ports,
        proxy,
        can_manage: user.role.can_manage_apps(),
    })
}

#[derive(Deserialize)]
pub struct PortsForm {
    /// Whitespace/comma separated mappings.
    mappings: String,
}

#[derive(Deserialize)]
pub struct PortForm {
    mapping: String,
}

pub async fn add(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<PortsForm>,
) -> Result<HttpResponse, AppError> {
    mutate(
        &state,
        &session,
        &req,
        path.into_inner(),
        PortsMutation::Add,
        &form.0.mappings,
    )
    .await
}

pub async fn set(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<PortsForm>,
) -> Result<HttpResponse, AppError> {
    mutate(
        &state,
        &session,
        &req,
        path.into_inner(),
        PortsMutation::Set,
        &form.0.mappings,
    )
    .await
}

pub async fn remove(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    form: CsrfForm<PortForm>,
) -> Result<HttpResponse, AppError> {
    mutate(
        &state,
        &session,
        &req,
        path.into_inner(),
        PortsMutation::Remove,
        &form.0.mapping,
    )
    .await
}

pub async fn clear(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    path: web::Path<String>,
    _form: CsrfForm<super::apps::ActionForm>,
) -> Result<HttpResponse, AppError> {
    mutate(
        &state,
        &session,
        &req,
        path.into_inner(),
        PortsMutation::Clear,
        "",
    )
    .await
}

#[derive(Clone, Copy)]
enum PortsMutation {
    Add,
    Set,
    Remove,
    Clear,
}

impl PortsMutation {
    fn verb(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Set => "set",
            Self::Remove => "remove",
            Self::Clear => "clear",
        }
    }
}

async fn mutate(
    state: &AppState,
    session: &Session,
    req: &HttpRequest,
    name: String,
    mutation: PortsMutation,
    raw_mappings: &str,
) -> Result<HttpResponse, AppError> {
    if let Err(err) = AppName::try_from(name.clone()) {
        let message = format!("Invalid app name: {err}");
        if is_htmx(req) {
            return modal_error(message);
        }
        set_flash(session, FlashLevel::Error, message);
        return Ok(see_other("/"));
    }
    let redirect_to = format!("/apps/{name}/ports");
    let mappings = if matches!(mutation, PortsMutation::Clear) {
        Vec::new()
    } else {
        match parse_port_mappings(raw_mappings) {
            Ok(mappings) => mappings,
            Err(err) => {
                let message = err.to_string();
                if is_htmx(req) {
                    return modal_error(message);
                }
                set_flash(session, FlashLevel::Error, message);
                return Ok(see_other(&redirect_to));
            }
        }
    };

    let spec = match mutation {
        PortsMutation::Add => JobSpec::PortsAdd {
            app: name.clone(),
            mappings: mappings.clone(),
        },
        PortsMutation::Set => JobSpec::PortsSet {
            app: name.clone(),
            mappings: mappings.clone(),
        },
        PortsMutation::Remove => JobSpec::PortsRemove {
            app: name.clone(),
            mappings: mappings.clone(),
        },
        PortsMutation::Clear => JobSpec::PortsClear { app: name.clone() },
    };
    let verb = mutation.verb();
    let completion = RunCompletion {
        success_message: format!("Port mappings updated for '{name}'."),
        redirect: None,
        refresh: RunRefresh::None,
    };
    let plan = vec![spec];
    if is_htmx(req) {
        return start_action_run(
            state,
            session,
            &RunRequest {
                subject: name.clone(),
                operation: format!("ports.{verb}"),
                target_kind: TargetKind::App,
                title: format!("Updating port mappings for {name}…"),
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
        &format!("ports.{verb}"),
        TargetKind::App,
        &plan,
        &completion,
        &[],
    )
    .await?;
    set_flash(
        session,
        FlashLevel::Success,
        format!("Queued: {verb} port mappings for {name}."),
    );
    Ok(see_other(&redirect_to))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_verbs_are_stable_audit_keys() {
        assert_eq!(PortsMutation::Add.verb(), "add");
        assert_eq!(PortsMutation::Set.verb(), "set");
        assert_eq!(PortsMutation::Remove.verb(), "remove");
        assert_eq!(PortsMutation::Clear.verb(), "clear");
    }
}
