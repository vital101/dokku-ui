use actix_session::Session;
use actix_web::{HttpResponse, web};
use askama::Template;
use futures_util::future::Either;
use serde::Deserialize;

use crate::domain::AppName;
use crate::domain::app_wizard::{AppWizardDraft, MAX_BUILDPACKS};
use crate::domain::build::{BUILDERS, is_valid_buildpack};
use crate::error::AppError;
use crate::storage::users::User;
use crate::web::csrf_form::{CsrfForm, ensure_csrf};
use crate::web::flash::{FlashLevel, FlashMessage, set_flash, take_flash};
use crate::web::fragments::current_user;
use crate::web::render::{render, see_other};
use crate::web::state::AppState;

const DRAFT_KEY: &str = "app_wizard_draft";

/// Session helpers for the app-creation wizard draft. The session store is
/// shared SQLite, so the draft survives across containers. Corrupt or stale
/// drafts (e.g. written by an older build) fall back to the default.
fn load_draft(session: &Session) -> Result<AppWizardDraft, AppError> {
    match session.get::<AppWizardDraft>(DRAFT_KEY) {
        Ok(Some(draft)) => Ok(draft),
        Ok(None) => Ok(AppWizardDraft::default()),
        Err(err) => {
            tracing::warn!(error = %err, "wizard draft was corrupt; starting over");
            clear_draft(session);
            Ok(AppWizardDraft::default())
        }
    }
}

fn save_draft(session: &Session, draft: &AppWizardDraft) -> Result<(), AppError> {
    session
        .insert(DRAFT_KEY, draft)
        .map_err(|err| AppError::Internal(err.to_string()))
}

fn clear_draft(session: &Session) {
    if session.remove(DRAFT_KEY).is_some() {
        tracing::debug!("wizard draft cleared");
    }
}

/// Shared preamble for every wizard step: resolve the user, gate on
/// `can_manage_apps`, load the draft, and make sure the session carries a
/// CSRF token. The auth middleware remains authoritative; this mirrors the
/// old create form's handler-side check. Left error = ready response to
/// return (redirect), right error = internal failure.
async fn step_preamble(
    state: &AppState,
    session: &Session,
) -> Result<(User, AppWizardDraft, String), Either<HttpResponse, AppError>> {
    let user = current_user(state, session).await.map_err(Either::Right)?;
    if !user.role.can_manage_apps() {
        set_flash(
            session,
            FlashLevel::Error,
            "Your role does not allow creating apps.",
        );
        return Err(Either::Left(see_other("/")));
    }
    let draft = load_draft(session).map_err(Either::Right)?;
    let csrf_token = ensure_csrf(session).await.map_err(Either::Right)?;
    Ok((user, draft, csrf_token))
}

#[derive(Template)]
#[template(path = "apps/wizard/name.html")]
struct NamePage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    active_step: &'static str,
    error: Option<String>,
    name: String,
}

#[derive(Deserialize)]
pub struct NameForm {
    name: String,
}

pub async fn name_step(
    state: web::Data<AppState>,
    session: Session,
) -> Result<HttpResponse, AppError> {
    let (user, draft, csrf_token) = match step_preamble(&state, &session).await {
        Ok(preamble) => preamble,
        Err(Either::Left(response)) => return Ok(response),
        Err(Either::Right(err)) => return Err(err),
    };
    let flash = take_flash(&session);

    let page = NamePage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        active_step: "name",
        error: None,
        name: draft.name.unwrap_or_default(),
    };
    render(&page)
}

pub async fn save_name(
    state: web::Data<AppState>,
    session: Session,
    form: CsrfForm<NameForm>,
) -> Result<HttpResponse, AppError> {
    let (user, mut draft, csrf_token) = match step_preamble(&state, &session).await {
        Ok(preamble) => preamble,
        Err(Either::Left(response)) => return Ok(response),
        Err(Either::Right(err)) => return Err(err),
    };

    let name = form.0.name.trim().to_owned();
    let error = match AppName::try_from(name.clone()) {
        Ok(app) => {
            // Best-effort duplicate check: if the snapshot cannot load we
            // cannot know, so we continue and let dokku reject the create.
            match state.snapshot.ensure_loaded().await {
                Ok(snapshot)
                    if snapshot
                        .apps
                        .iter()
                        .any(|existing| existing == app.as_str()) =>
                {
                    Some(format!("An app named '{app}' already exists."))
                }
                Ok(_) => None,
                Err(err) => {
                    tracing::warn!(error = %err, "duplicate-check snapshot load failed; continuing");
                    None
                }
            }
        }
        Err(err) => Some(format!("Invalid app name: {err}")),
    };

    if let Some(error) = error {
        // Re-render (no redirect) so the typed name survives.
        let flash = take_flash(&session);
        let page = NamePage {
            email: &user.email,
            csrf_token: &csrf_token,
            flash: flash.as_ref(),
            active_step: "name",
            error: Some(error),
            name,
        };
        return render(&page);
    }

    draft.name = Some(name);
    save_draft(&session, &draft)?;
    Ok(see_other("/apps/new/build"))
}

#[derive(Template)]
#[template(path = "apps/wizard/build.html")]
struct BuildPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    active_step: &'static str,
    error: Option<String>,
    builder: String,
    buildpacks: String,
    builders: Vec<String>,
}

#[derive(Deserialize)]
pub struct BuildForm {
    #[serde(default)]
    builder: String,
    #[serde(default)]
    buildpacks: String,
}

fn builder_names() -> Vec<String> {
    BUILDERS.iter().map(|name| (*name).to_owned()).collect()
}

pub async fn build_step(
    state: web::Data<AppState>,
    session: Session,
) -> Result<HttpResponse, AppError> {
    let (user, draft, csrf_token) = match step_preamble(&state, &session).await {
        Ok(preamble) => preamble,
        Err(Either::Left(response)) => return Ok(response),
        Err(Either::Right(err)) => return Err(err),
    };
    if draft.name.is_none() {
        return Ok(see_other("/apps/new"));
    }
    let flash = take_flash(&session);

    let page = BuildPage {
        email: &user.email,
        csrf_token: &csrf_token,
        flash: flash.as_ref(),
        active_step: "build",
        error: None,
        builder: draft.builder.unwrap_or_default(),
        buildpacks: draft.buildpacks.join("\n"),
        builders: builder_names(),
    };
    render(&page)
}

pub async fn save_build(
    state: web::Data<AppState>,
    session: Session,
    form: CsrfForm<BuildForm>,
) -> Result<HttpResponse, AppError> {
    let (user, mut draft, csrf_token) = match step_preamble(&state, &session).await {
        Ok(preamble) => preamble,
        Err(Either::Left(response)) => return Ok(response),
        Err(Either::Right(err)) => return Err(err),
    };
    if draft.name.is_none() {
        return Ok(see_other("/apps/new"));
    }

    let builder = form.0.builder.trim().to_owned();
    let buildpacks: Vec<String> = form
        .0
        .buildpacks
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();

    let error = if !builder.is_empty() && !BUILDERS.contains(&builder.as_str()) {
        Some("Unknown builder.".to_owned())
    } else if buildpacks.len() > MAX_BUILDPACKS {
        Some(format!(
            "Too many buildpacks; at most {MAX_BUILDPACKS} are allowed."
        ))
    } else if let Some(bad) = buildpacks.iter().find(|line| !is_valid_buildpack(line)) {
        Some(format!("Invalid buildpack '{bad}'."))
    } else if !buildpacks.is_empty() && !builder.is_empty() && builder != "herokuish" {
        Some("Buildpacks require the herokuish builder.".to_owned())
    } else {
        None
    };

    if let Some(error) = error {
        // Re-render (no redirect) so the typed values survive.
        let flash = take_flash(&session);
        let page = BuildPage {
            email: &user.email,
            csrf_token: &csrf_token,
            flash: flash.as_ref(),
            active_step: "build",
            error: Some(error),
            builder,
            buildpacks: buildpacks.join("\n"),
            builders: builder_names(),
        };
        return render(&page);
    }

    // Auto stays `None`: `to_plan` inserts `herokuish` when buildpacks follow.
    draft.builder = if builder.is_empty() {
        None
    } else {
        Some(builder)
    };
    draft.buildpacks = buildpacks;
    save_draft(&session, &draft)?;
    Ok(see_other("/apps/new/services"))
}

/// Body for the reset POST: nothing but the CSRF token. A unit `()` target
/// would reject the extra `csrf_token` field, so this empty struct lets the
/// CSRF extractor do its job and drops the rest.
#[derive(Deserialize)]
pub struct ResetForm {}

pub async fn reset(
    state: web::Data<AppState>,
    session: Session,
    form: CsrfForm<ResetForm>,
) -> Result<HttpResponse, AppError> {
    let CsrfForm(ResetForm {}) = form;
    match step_preamble(&state, &session).await {
        Ok((_user, _draft, _csrf_token)) => {}
        Err(Either::Left(response)) => return Ok(response),
        Err(Either::Right(err)) => return Err(err),
    }
    clear_draft(&session);
    set_flash(&session, FlashLevel::Success, "Wizard reset.");
    Ok(see_other("/apps/new"))
}
