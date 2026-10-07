use actix_session::Session;
use actix_web::{HttpResponse, web};
use askama::Template;
use serde::Deserialize;

use crate::domain::command::DokkuCommand;
use crate::domain::parse::parse_ssh_keys;
use crate::domain::ssh_key::{is_valid_public_key, is_valid_ssh_key_name};
use crate::domain::types::SshKey;
use crate::error::AppError;
use crate::storage::runs::{NewRun, RunOutcome, TargetKind};
use crate::web::csrf_form::CsrfForm;
use crate::web::flash::{FlashLevel, FlashMessage, set_flash, take_flash};
use crate::web::fragments::{current_actor, current_user};
use crate::web::render::{render, see_other};
use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "keys/list.html")]
struct KeysPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    keys: Vec<SshKey>,
    list_error: Option<String>,
}

/// Admin screen for the host's authorized deploy keys (`ssh-keys:*`). These
/// are identity operations, not app mutations: they run synchronously and are
/// recorded in the audit trail rather than through the job queue (which has
/// no stdin channel for `ssh-keys:add`).
pub async fn index(state: web::Data<AppState>, session: Session) -> Result<HttpResponse, AppError> {
    let user = current_user(&state, &session).await?;
    let csrf_token = crate::web::csrf_form::ensure_csrf(&session).await?;
    let flash = take_flash(&session);
    let (keys, list_error) = match state.dokku.exec(&DokkuCommand::SshKeysList).await {
        Ok(output) => (parse_ssh_keys(&output.stdout), None),
        Err(err) => (Vec::new(), Some(err.to_string())),
    };

    let page = KeysPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        keys,
        list_error,
    };
    render(&page)
}

#[derive(Deserialize)]
pub struct AddKeyForm {
    name: String,
    key: String,
}

#[derive(Deserialize)]
pub struct RemoveKeyForm {
    name: String,
}

pub async fn add(
    state: web::Data<AppState>,
    session: Session,
    form: CsrfForm<AddKeyForm>,
) -> Result<HttpResponse, AppError> {
    let name = form.0.name.trim().to_owned();
    if !is_valid_ssh_key_name(&name) {
        set_flash(
            &session,
            FlashLevel::Error,
            "Key names may contain letters, digits, '.', '_', '-', '+', and '@'.",
        );
        return Ok(see_other("/keys"));
    }
    let key = form.0.key.trim().to_owned();
    if !is_valid_public_key(&key) {
        set_flash(
            &session,
            FlashLevel::Error,
            "That does not look like an OpenSSH public key.",
        );
        return Ok(see_other("/keys"));
    }

    // The key is written to stdin (never argv); the trailing newline matches
    // the single-line `read` in `dokku ssh-keys:add`.
    let payload = format!("{key}\n");
    let result = state
        .dokku
        .exec_with_stdin(&DokkuCommand::SshKeysAdd { name: name.clone() }, &payload)
        .await;
    match &result {
        Ok(_) => set_flash(
            &session,
            FlashLevel::Success,
            format!("Key '{name}' added."),
        ),
        Err(err) => set_flash(
            &session,
            FlashLevel::Error,
            format!("Failed to add key '{name}': {err}"),
        ),
    }
    record(&state, &session, "ssh-key.add", &name, result.is_ok()).await;
    Ok(see_other("/keys"))
}

pub async fn remove(
    state: web::Data<AppState>,
    session: Session,
    form: CsrfForm<RemoveKeyForm>,
) -> Result<HttpResponse, AppError> {
    let name = form.0.name.trim().to_owned();
    if !is_valid_ssh_key_name(&name) {
        set_flash(&session, FlashLevel::Error, "Invalid key name.");
        return Ok(see_other("/keys"));
    }
    let result = state
        .dokku
        .exec(&DokkuCommand::SshKeysRemove { name: name.clone() })
        .await;
    match &result {
        Ok(_) => set_flash(
            &session,
            FlashLevel::Success,
            format!("Key '{name}' removed."),
        ),
        Err(err) => set_flash(
            &session,
            FlashLevel::Error,
            format!("Failed to remove key '{name}': {err}"),
        ),
    }
    record(&state, &session, "ssh-key.remove", &name, result.is_ok()).await;
    Ok(see_other("/keys"))
}

/// Best-effort audit entry for an identity operation (no run lines; the key
/// material never reaches the audit trail).
async fn record(state: &AppState, session: &Session, operation: &str, subject: &str, ok: bool) {
    let actor = current_actor(state, session).await;
    let new_run = NewRun {
        subject: subject.to_owned(),
        operation: operation.to_owned(),
        target_kind: TargetKind::System,
        actor,
        parent_run_id: None,
    };
    if let Ok(run_id) = state.action_runs.insert_with(&new_run).await {
        let _ = state
            .action_runs
            .finish(
                &run_id,
                &RunOutcome {
                    ok,
                    message: format!("{operation} {subject}"),
                    redirect: None,
                },
            )
            .await;
    }
}
