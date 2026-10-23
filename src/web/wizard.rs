use actix_session::Session;
use actix_web::{HttpResponse, web};
use askama::Template;
use futures_util::future::Either;
use serde::Deserialize;

use crate::dokku::service_names;
use crate::domain::AppName;
use crate::domain::app_wizard::{
    AppWizardDraft, MAX_BUILDPACKS, MAX_SERVICES, MAX_VOLUMES, VolumeMode, WizardService,
    WizardVolume,
};
use crate::domain::build::{BUILDERS, is_valid_buildpack};
use crate::domain::capabilities::Support;
use crate::domain::command::DokkuCommand;
use crate::domain::mount_spec::MountSpec;
use crate::domain::parse::parse_storage_entries;
use crate::domain::service_name::ServiceName;
use crate::domain::service_plugin::ServicePlugin;
use crate::domain::storage_entry::{StorageEntryName, is_valid_volume_container_path};
use crate::domain::types::StorageEntry;
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

/// One option of the attach select: a service that already exists on the host.
struct AvailableService {
    plugin: ServicePlugin,
    service: String,
}

/// One row of the current-selections table.
struct SelectedServiceRow {
    plugin_label: String,
    service: String,
    mode_label: &'static str,
}

#[derive(Template)]
#[template(path = "apps/wizard/services.html")]
struct ServicesPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    active_step: &'static str,
    error: Option<String>,
    services_error: Option<String>,
    list_errors: Vec<(String, String)>,
    available_services: Vec<AvailableService>,
    plugins: Vec<ServicePlugin>,
    selected_rows: Vec<SelectedServiceRow>,
}

/// What the services step learned from the host: the installed datastore
/// plugins, their existing service names, and where the reads failed.
struct ServiceCatalog {
    plugins: Vec<ServicePlugin>,
    available_services: Vec<AvailableService>,
    list_errors: Vec<(String, String)>,
    services_error: Option<String>,
}

/// Capability-gated plugin inventory plus one `<plugin>:list` per installed
/// plugin — names only, never the per-service `<plugin>:info` fetches the
/// service pages use. A failed probe blanks the plugin/service lists (the
/// notice never blocks "Next"); a failing single plugin list only degrades
/// that plugin.
async fn load_service_catalog(state: &AppState) -> ServiceCatalog {
    let plugins: Vec<ServicePlugin> = match state.capabilities.ensure_loaded().await {
        Ok(capabilities) => ServicePlugin::all()
            .filter(|plugin| {
                !matches!(
                    capabilities.supports_plugin(plugin.as_str()),
                    Support::PluginMissing { .. }
                )
            })
            .collect(),
        Err(err) => {
            tracing::warn!(error = %err, "wizard services step: capability probe failed");
            return ServiceCatalog {
                plugins: Vec::new(),
                available_services: Vec::new(),
                list_errors: Vec::new(),
                services_error: Some(err.to_string()),
            };
        }
    };

    let mut available_services = Vec::new();
    let mut list_errors = Vec::new();
    for plugin in &plugins {
        match service_names(&*state.dokku, *plugin).await {
            Ok(names) => {
                available_services.extend(names.into_iter().map(|service| AvailableService {
                    plugin: *plugin,
                    service,
                }))
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    plugin = plugin.as_str(),
                    "wizard services step: plugin list failed"
                );
                list_errors.push((plugin.as_str().to_owned(), err.to_string()));
            }
        }
    }

    ServiceCatalog {
        plugins,
        available_services,
        list_errors,
        services_error: None,
    }
}

/// Renders the draft's accumulated services. A hand-edited draft can carry a
/// plugin name outside the catalog; its raw string is shown rather than
/// dropped, and `to_plan` rejects it later.
fn selected_rows(draft: &AppWizardDraft) -> Vec<SelectedServiceRow> {
    draft
        .services
        .iter()
        .map(|entry| SelectedServiceRow {
            plugin_label: ServicePlugin::try_from(entry.plugin.as_str())
                .map(|plugin| plugin.display_name().to_owned())
                .unwrap_or_else(|_| entry.plugin.clone()),
            service: entry.service.clone(),
            mode_label: if entry.create {
                "create + link"
            } else {
                "attach"
            },
        })
        .collect()
}

async fn render_services(
    state: &AppState,
    session: &Session,
    user: &User,
    csrf_token: &str,
    draft: &AppWizardDraft,
    error: Option<String>,
) -> Result<HttpResponse, AppError> {
    let catalog = load_service_catalog(state).await;
    let flash = take_flash(session);
    let page = ServicesPage {
        email: &user.email,
        csrf_token,
        flash: flash.as_ref(),
        active_step: "services",
        error,
        services_error: catalog.services_error,
        list_errors: catalog.list_errors,
        available_services: catalog.available_services,
        plugins: catalog.plugins,
        selected_rows: selected_rows(draft),
    };
    render(&page)
}

/// Parses a `"<plugin>:<service>"` select value. Splitting once on the colon
/// keeps a smuggled second colon inside the service name, where the
/// `ServiceName` validator rejects it.
fn parse_service_key(raw: &str) -> Result<(ServicePlugin, ServiceName), String> {
    let Some((plugin, service)) = raw.split_once(':') else {
        return Err("Choose a service to attach.".to_owned());
    };
    let plugin = ServicePlugin::try_from(plugin.trim())
        .map_err(|_| format!("Unknown service plugin '{plugin}'."))?;
    let service = ServiceName::try_from(service.trim())
        .map_err(|err| format!("Invalid service name: {err}"))?;
    Ok((plugin, service))
}

/// Duplicate and size checks shared by the attach and create POSTs. The draft
/// is the accumulated state, so the same plugin/service pair can only be
/// selected once and the step is capped at `MAX_SERVICES`.
fn check_selection(
    draft: &AppWizardDraft,
    plugin: ServicePlugin,
    service: &str,
) -> Result<(), String> {
    if draft
        .services
        .iter()
        .any(|entry| entry.plugin == plugin.as_str() && entry.service == service)
    {
        return Err(format!(
            "The {} service '{service}' is already selected.",
            plugin.display_name()
        ));
    }
    if draft.services.len() >= MAX_SERVICES {
        return Err(format!(
            "Too many services; at most {MAX_SERVICES} are allowed."
        ));
    }
    Ok(())
}

/// Validates an attach selection against the host: the plugin's list must be
/// readable and must contain the service. No command is executed — the link
/// happens in the final combined run.
async fn plan_attach(
    state: &AppState,
    draft: &AppWizardDraft,
    service_key: &str,
) -> Result<WizardService, String> {
    let (plugin, service) = parse_service_key(service_key)?;
    let names = service_names(&*state.dokku, plugin).await.map_err(|err| {
        tracing::warn!(
            error = %err,
            plugin = plugin.as_str(),
            "wizard attach: plugin list failed"
        );
        format!("Could not read {} services; retry.", plugin.display_name())
    })?;
    if !names.iter().any(|name| name == service.as_str()) {
        return Err(format!(
            "No {} service named '{service}' exists.",
            plugin.display_name()
        ));
    }
    check_selection(draft, plugin, service.as_str())?;
    Ok(WizardService {
        plugin: plugin.as_str().to_owned(),
        service: service.as_str().to_owned(),
        create: false,
    })
}

/// Validates a create selection: known plugin, valid name, the plugin must be
/// installed, its list must be readable, and the name must be free. Service
/// creation itself is deferred to the final combined run.
async fn plan_create(
    state: &AppState,
    draft: &AppWizardDraft,
    plugin: &str,
    service: &str,
) -> Result<WizardService, String> {
    let plugin = ServicePlugin::try_from(plugin.trim())
        .map_err(|_| format!("Unknown service plugin '{plugin}'."))?;
    let service = ServiceName::try_from(service.trim())
        .map_err(|err| format!("Invalid service name: {err}"))?;
    let capabilities = state.capabilities.ensure_loaded().await.map_err(|err| {
        tracing::warn!(error = %err, "wizard create: capability probe failed");
        "Could not read the host's installed plugins; retry.".to_owned()
    })?;
    if matches!(
        capabilities.supports_plugin(plugin.as_str()),
        Support::PluginMissing { .. }
    ) {
        return Err(format!(
            "The {} plugin is not installed on this host.",
            plugin.display_name()
        ));
    }
    let names = service_names(&*state.dokku, plugin).await.map_err(|err| {
        tracing::warn!(
            error = %err,
            plugin = plugin.as_str(),
            "wizard create: plugin list failed"
        );
        format!("Could not read {} services; retry.", plugin.display_name())
    })?;
    if names.iter().any(|name| name == service.as_str()) {
        return Err(format!(
            "A {} service named '{service}' already exists; attach it instead.",
            plugin.display_name()
        ));
    }
    check_selection(draft, plugin, service.as_str())?;
    Ok(WizardService {
        plugin: plugin.as_str().to_owned(),
        service: service.as_str().to_owned(),
        create: true,
    })
}

pub async fn services_step(
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
    render_services(&state, &session, &user, &csrf_token, &draft, None).await
}

#[derive(Deserialize)]
pub struct AttachServiceForm {
    /// `"<plugin>:<service>"`
    service_key: String,
}

pub async fn attach_service(
    state: web::Data<AppState>,
    session: Session,
    form: CsrfForm<AttachServiceForm>,
) -> Result<HttpResponse, AppError> {
    let (user, draft, csrf_token) = match step_preamble(&state, &session).await {
        Ok(preamble) => preamble,
        Err(Either::Left(response)) => return Ok(response),
        Err(Either::Right(err)) => return Err(err),
    };
    if draft.name.is_none() {
        return Ok(see_other("/apps/new"));
    }

    match plan_attach(&state, &draft, &form.0.service_key).await {
        Err(error) => {
            render_services(&state, &session, &user, &csrf_token, &draft, Some(error)).await
        }
        Ok(entry) => {
            let mut draft = draft;
            draft.services.push(entry);
            save_draft(&session, &draft)?;
            Ok(see_other("/apps/new/services"))
        }
    }
}

#[derive(Deserialize)]
pub struct CreateServiceForm {
    plugin: String,
    service: String,
}

pub async fn create_service(
    state: web::Data<AppState>,
    session: Session,
    form: CsrfForm<CreateServiceForm>,
) -> Result<HttpResponse, AppError> {
    let (user, draft, csrf_token) = match step_preamble(&state, &session).await {
        Ok(preamble) => preamble,
        Err(Either::Left(response)) => return Ok(response),
        Err(Either::Right(err)) => return Err(err),
    };
    if draft.name.is_none() {
        return Ok(see_other("/apps/new"));
    }

    let CsrfForm(CreateServiceForm { plugin, service }) = form;
    match plan_create(&state, &draft, &plugin, &service).await {
        Err(error) => {
            render_services(&state, &session, &user, &csrf_token, &draft, Some(error)).await
        }
        Ok(entry) => {
            let mut draft = draft;
            draft.services.push(entry);
            save_draft(&session, &draft)?;
            Ok(see_other("/apps/new/services"))
        }
    }
}

#[derive(Deserialize)]
pub struct RemoveServiceForm {
    index: usize,
}

pub async fn remove_service(
    state: web::Data<AppState>,
    session: Session,
    form: CsrfForm<RemoveServiceForm>,
) -> Result<HttpResponse, AppError> {
    let (_user, mut draft, _csrf_token) = match step_preamble(&state, &session).await {
        Ok(preamble) => preamble,
        Err(Either::Left(response)) => return Ok(response),
        Err(Either::Right(err)) => return Err(err),
    };
    if draft.name.is_none() {
        return Ok(see_other("/apps/new"));
    }

    // An out-of-range index is a no-op: the row was already gone (another tab,
    // or a stale page), and a redirect shows the current state.
    if form.0.index < draft.services.len() {
        draft.services.remove(form.0.index);
        save_draft(&session, &draft)?;
    }
    Ok(see_other("/apps/new/services"))
}

/// One row of the volumes step's current-selections table. The fixed mode
/// labels keep askama from having to match the draft's `VolumeMode` enum.
struct SelectedVolumeRow {
    mode_label: &'static str,
    source: String,
    container: String,
}

#[derive(Template)]
#[template(path = "apps/wizard/volumes.html")]
struct VolumesPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    active_step: &'static str,
    error: Option<String>,
    entries_error: Option<String>,
    entries: Vec<StorageEntry>,
    selected_rows: Vec<SelectedVolumeRow>,
}

/// The host's registered storage entries (`storage:list-entries --format
/// json`). The error string only gates the attach-entry select: create and
/// attach-path never need the list to be readable.
async fn load_entry_list(state: &AppState) -> Result<Vec<StorageEntry>, String> {
    match state.dokku.exec(&DokkuCommand::StorageListEntries).await {
        Ok(output) => Ok(parse_storage_entries(&output.stdout)),
        Err(err) => {
            tracing::warn!(error = %err, "wizard volumes step: storage entry list failed");
            Err(err.to_string())
        }
    }
}

fn volume_rows(draft: &AppWizardDraft) -> Vec<SelectedVolumeRow> {
    draft
        .volumes
        .iter()
        .map(|volume| SelectedVolumeRow {
            mode_label: match volume.mode {
                VolumeMode::Create => "create named entry",
                VolumeMode::AttachEntry => "attach entry",
                VolumeMode::AttachPath => "attach host path",
            },
            source: volume.source.clone(),
            container: volume.container.clone(),
        })
        .collect()
}

async fn render_volumes(
    state: &AppState,
    session: &Session,
    user: &User,
    csrf_token: &str,
    draft: &AppWizardDraft,
    error: Option<String>,
) -> Result<HttpResponse, AppError> {
    let (entries, entries_error) = match load_entry_list(state).await {
        Ok(entries) => (entries, None),
        Err(err) => (Vec::new(), Some(err)),
    };
    let flash = take_flash(session);
    let page = VolumesPage {
        email: &user.email,
        csrf_token,
        flash: flash.as_ref(),
        active_step: "volumes",
        error,
        entries_error,
        entries,
        selected_rows: volume_rows(draft),
    };
    render(&page)
}

/// Duplicate and size checks shared by the three add POSTs: the same
/// `(mode, source, container)` triple can only be selected once and the step
/// is capped at `MAX_VOLUMES`.
fn check_volume_selection(
    draft: &AppWizardDraft,
    mode: VolumeMode,
    source: &str,
    container: &str,
) -> Result<(), String> {
    if draft.volumes.iter().any(|volume| {
        volume.mode == mode && volume.source == source && volume.container == container
    }) {
        return Err(format!(
            "The volume '{source}' at '{container}' is already selected."
        ));
    }
    if draft.volumes.len() >= MAX_VOLUMES {
        return Err(format!(
            "Too many volumes; at most {MAX_VOLUMES} are allowed."
        ));
    }
    Ok(())
}

/// Validates a create selection: a fresh non-reserved entry name plus a valid
/// `--container-dir` path. If the entry list is readable and already holds
/// the name, the user means attach-entry instead. A failing list only skips
/// the exists check — `storage:create` would collide in the final run. The
/// entry is never created here; that happens in the combined plan run.
async fn plan_create_volume(
    state: &AppState,
    draft: &AppWizardDraft,
    entry: &str,
    container: &str,
) -> Result<WizardVolume, String> {
    let entry = StorageEntryName::try_from_create(entry)
        .map_err(|err| format!("Invalid storage entry name: {err}"))?;
    if !is_valid_volume_container_path(container) {
        return Err(format!("Invalid container path '{container}'."));
    }
    match load_entry_list(state).await {
        Ok(entries)
            if entries
                .iter()
                .any(|existing| existing.name == entry.as_str()) =>
        {
            return Err(format!(
                "Entry '{entry}' already exists; attach it instead."
            ));
        }
        Ok(_) => {}
        Err(err) => {
            tracing::warn!(
                error = %err,
                "wizard volume create: entry list failed; skipping the exists check"
            );
        }
    }
    check_volume_selection(draft, VolumeMode::Create, entry.as_str(), container)?;
    Ok(WizardVolume {
        mode: VolumeMode::Create,
        source: entry.to_string(),
        container: container.to_owned(),
    })
}

/// Validates an attach-entry selection against the host: the entry list must
/// be readable and must contain the name. The `StorageEntryName` parse
/// allows `legacy-*` names, which exist on hosts but cannot be created.
async fn plan_attach_entry(
    state: &AppState,
    draft: &AppWizardDraft,
    entry: &str,
    container: &str,
) -> Result<WizardVolume, String> {
    let entries = load_entry_list(state).await.map_err(|err| {
        tracing::warn!(error = %err, "wizard attach entry: entry list failed");
        "Could not read storage entries; retry.".to_owned()
    })?;
    if !entries.iter().any(|existing| existing.name == entry) {
        return Err(format!("No storage entry named '{entry}' exists."));
    }
    let entry = StorageEntryName::try_from(entry)
        .map_err(|err| format!("Invalid storage entry name: {err}"))?;
    if !is_valid_volume_container_path(container) {
        return Err(format!("Invalid container path '{container}'."));
    }
    check_volume_selection(draft, VolumeMode::AttachEntry, entry.as_str(), container)?;
    Ok(WizardVolume {
        mode: VolumeMode::AttachEntry,
        source: entry.to_string(),
        container: container.to_owned(),
    })
}

/// Validates an attach-path selection with the colon-form `MountSpec`, then
/// stores the two halves. No entry list is involved: any host path or docker
/// volume name is acceptable to `storage:mount`.
fn plan_attach_path(
    draft: &AppWizardDraft,
    host: &str,
    container: &str,
) -> Result<WizardVolume, String> {
    let mount = MountSpec::try_from(format!("{host}:{container}"))
        .map_err(|err| format!("Invalid host mount: {err}"))?;
    check_volume_selection(
        draft,
        VolumeMode::AttachPath,
        mount.host(),
        mount.container(),
    )?;
    Ok(WizardVolume {
        mode: VolumeMode::AttachPath,
        source: mount.host().to_owned(),
        container: mount.container().to_owned(),
    })
}

pub async fn volumes_step(
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
    render_volumes(&state, &session, &user, &csrf_token, &draft, None).await
}

#[derive(Deserialize)]
pub struct CreateVolumeForm {
    entry: String,
    container: String,
}

pub async fn create_volume(
    state: web::Data<AppState>,
    session: Session,
    form: CsrfForm<CreateVolumeForm>,
) -> Result<HttpResponse, AppError> {
    let (user, draft, csrf_token) = match step_preamble(&state, &session).await {
        Ok(preamble) => preamble,
        Err(Either::Left(response)) => return Ok(response),
        Err(Either::Right(err)) => return Err(err),
    };
    if draft.name.is_none() {
        return Ok(see_other("/apps/new"));
    }

    let CsrfForm(CreateVolumeForm { entry, container }) = form;
    let entry = entry.trim().to_owned();
    let container = container.trim().to_owned();
    match plan_create_volume(&state, &draft, &entry, &container).await {
        Err(error) => {
            render_volumes(&state, &session, &user, &csrf_token, &draft, Some(error)).await
        }
        Ok(volume) => {
            let mut draft = draft;
            draft.volumes.push(volume);
            save_draft(&session, &draft)?;
            Ok(see_other("/apps/new/volumes"))
        }
    }
}

#[derive(Deserialize)]
pub struct AttachVolumeEntryForm {
    entry: String,
    container: String,
}

pub async fn attach_volume_entry(
    state: web::Data<AppState>,
    session: Session,
    form: CsrfForm<AttachVolumeEntryForm>,
) -> Result<HttpResponse, AppError> {
    let (user, draft, csrf_token) = match step_preamble(&state, &session).await {
        Ok(preamble) => preamble,
        Err(Either::Left(response)) => return Ok(response),
        Err(Either::Right(err)) => return Err(err),
    };
    if draft.name.is_none() {
        return Ok(see_other("/apps/new"));
    }

    let CsrfForm(AttachVolumeEntryForm { entry, container }) = form;
    let entry = entry.trim().to_owned();
    let container = container.trim().to_owned();
    match plan_attach_entry(&state, &draft, &entry, &container).await {
        Err(error) => {
            render_volumes(&state, &session, &user, &csrf_token, &draft, Some(error)).await
        }
        Ok(volume) => {
            let mut draft = draft;
            draft.volumes.push(volume);
            save_draft(&session, &draft)?;
            Ok(see_other("/apps/new/volumes"))
        }
    }
}

#[derive(Deserialize)]
pub struct AttachVolumePathForm {
    host: String,
    container: String,
}

pub async fn attach_volume_path(
    state: web::Data<AppState>,
    session: Session,
    form: CsrfForm<AttachVolumePathForm>,
) -> Result<HttpResponse, AppError> {
    let (user, draft, csrf_token) = match step_preamble(&state, &session).await {
        Ok(preamble) => preamble,
        Err(Either::Left(response)) => return Ok(response),
        Err(Either::Right(err)) => return Err(err),
    };
    if draft.name.is_none() {
        return Ok(see_other("/apps/new"));
    }

    let CsrfForm(AttachVolumePathForm { host, container }) = form;
    let host = host.trim().to_owned();
    let container = container.trim().to_owned();
    match plan_attach_path(&draft, &host, &container) {
        Err(error) => {
            render_volumes(&state, &session, &user, &csrf_token, &draft, Some(error)).await
        }
        Ok(volume) => {
            let mut draft = draft;
            draft.volumes.push(volume);
            save_draft(&session, &draft)?;
            Ok(see_other("/apps/new/volumes"))
        }
    }
}

#[derive(Deserialize)]
pub struct RemoveVolumeForm {
    index: usize,
}

pub async fn remove_volume(
    state: web::Data<AppState>,
    session: Session,
    form: CsrfForm<RemoveVolumeForm>,
) -> Result<HttpResponse, AppError> {
    let (_user, mut draft, _csrf_token) = match step_preamble(&state, &session).await {
        Ok(preamble) => preamble,
        Err(Either::Left(response)) => return Ok(response),
        Err(Either::Right(err)) => return Err(err),
    };
    if draft.name.is_none() {
        return Ok(see_other("/apps/new"));
    }

    // Same no-op semantics as the services remove: a stale row index just
    // redirects to the current state.
    if form.0.index < draft.volumes.len() {
        draft.volumes.remove(form.0.index);
        save_draft(&session, &draft)?;
    }
    Ok(see_other("/apps/new/volumes"))
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
