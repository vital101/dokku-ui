use serde::{Deserialize, Serialize};

use crate::domain::AppName;
use crate::domain::build::{
    is_valid_builder_property, is_valid_builder_value, is_valid_buildpack, is_valid_buildpack_index,
};
use crate::domain::command::DokkuCommand;
use crate::domain::cron::is_valid_cron_id;
use crate::domain::domain_name::DomainName;
use crate::domain::env_file::{is_valid_config_key, is_valid_config_value};
use crate::domain::git::{
    DEPLOY_BRANCH_PROPERTY, GitBuildMode, is_valid_archive_url, is_valid_git_ref,
    is_valid_git_remote, is_valid_image_ref,
};
use crate::domain::http_auth::is_valid_username;
use crate::domain::mount_spec::MountSpec;
use crate::domain::resource::{is_valid_process_type, is_valid_resource_value};
use crate::domain::service_create::ServiceCreateOptions;
use crate::domain::service_name::ServiceName;
use crate::domain::service_plugin::ServicePlugin;
use crate::domain::tls::{LetsencryptAction, is_valid_letsencrypt_email};
use crate::domain::types::{EnvVar, ScaleEntry};

/// Serializable description of one queued mutation. Rehydrating through
/// `to_commands()` re-validates every name with the newtypes' constructors, so
/// a tampered database payload can never reach a command with an invalid name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobSpec {
    AppAction {
        app: String,
        action: AppAction,
    },
    AppScale {
        app: String,
        scales: Vec<ScaleEntry>,
    },
    AppDestroy {
        app: String,
    },
    AppLock {
        app: String,
    },
    AppUnlock {
        app: String,
    },
    AppRename {
        app: String,
        new_name: String,
    },
    DomainsAdd {
        app: String,
        domains: Vec<String>,
    },
    DomainsRemove {
        app: String,
        domains: Vec<String>,
    },
    DomainsSet {
        app: String,
        domains: Vec<String>,
    },
    ResourceLimit {
        app: String,
        process_type: String,
        cpu: Option<String>,
        memory: Option<String>,
        memory_swap: Option<String>,
    },
    ResourceReserve {
        app: String,
        process_type: String,
        cpu: Option<String>,
        memory: Option<String>,
    },
    ResourceLimitClear {
        app: String,
        process_type: String,
    },
    ResourceReserveClear {
        app: String,
        process_type: String,
    },
    CronRun {
        app: String,
        cron_id: String,
    },
    CronSuspend {
        app: String,
        cron_id: String,
    },
    CronResume {
        app: String,
        cron_id: String,
    },
    MaintenanceEnable {
        app: String,
    },
    MaintenanceDisable {
        app: String,
    },
    HttpAuthEnable {
        app: String,
    },
    HttpAuthDisable {
        app: String,
    },
    HttpAuthAddUser {
        app: String,
        username: String,
        password: String,
    },
    HttpAuthRemoveUser {
        app: String,
        username: String,
    },
    BuildpacksSet {
        app: String,
        buildpack: String,
        index: Option<u32>,
    },
    BuildpacksAdd {
        app: String,
        buildpack: String,
        index: Option<u32>,
    },
    BuildpacksRemove {
        app: String,
        buildpack: String,
    },
    BuildpacksClear {
        app: String,
    },
    BuilderSet {
        app: String,
        property: String,
        value: Option<String>,
    },
    ServiceAction {
        plugin: String,
        service: String,
        action: ServiceAction,
    },
    ServiceCreate {
        plugin: String,
        service: String,
        #[serde(default)]
        options: ServiceCreateOptions,
    },
    ServiceDestroy {
        plugin: String,
        service: String,
    },
    ServiceExpose {
        plugin: String,
        service: String,
        ports: String,
    },
    ServiceUnexpose {
        plugin: String,
        service: String,
    },
    ServiceLink {
        plugin: String,
        service: String,
        app: String,
    },
    ServiceUnlink {
        plugin: String,
        service: String,
        app: String,
    },
    ConfigSet {
        app: String,
        vars: Vec<EnvVar>,
    },
    ConfigUnset {
        app: String,
        keys: Vec<String>,
    },
    GitSet {
        app: String,
        property: String,
        value: Option<String>,
    },
    GitSync {
        app: String,
        repo: String,
        git_ref: Option<String>,
        build_mode: GitBuildMode,
    },
    GitFromImage {
        app: String,
        image: String,
    },
    GitFromArchive {
        app: String,
        archive_url: String,
    },
    LetsencryptAction {
        app: String,
        action: LetsencryptAction,
    },
    LetsencryptSet {
        app: String,
        property: String,
        value: Option<String>,
    },
    LetsencryptCronJob {
        add: bool,
    },
    VolumeMount {
        app: String,
        spec: String,
    },
    VolumeUnmount {
        app: String,
        locator: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppAction {
    Start,
    Stop,
    Restart,
    Rebuild,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceAction {
    Start,
    Stop,
    Restart,
}

impl JobSpec {
    /// True for operations that must never be auto-retried (destructive).
    pub fn is_destructive(&self) -> bool {
        matches!(self, JobSpec::AppDestroy { .. }) || matches!(self, JobSpec::ServiceDestroy { .. })
    }

    /// Rebuilds the `DokkuCommand`s for this job, validating every name.
    pub fn to_commands(&self) -> Result<Vec<DokkuCommand>, JobSpecError> {
        match self {
            JobSpec::AppAction { app, action } => {
                let app = parse_app(app)?;
                let command = match action {
                    AppAction::Start => DokkuCommand::PsStart { app },
                    AppAction::Stop => DokkuCommand::PsStop { app },
                    AppAction::Restart => DokkuCommand::PsRestart { app },
                    AppAction::Rebuild => DokkuCommand::PsRebuild { app },
                };
                Ok(vec![command])
            }
            JobSpec::AppScale { app, scales } => {
                let app = parse_app(app)?;
                Ok(vec![DokkuCommand::PsScaleSet {
                    app,
                    scales: scales.clone(),
                }])
            }
            JobSpec::AppDestroy { app } => {
                let app = parse_app(app)?;
                Ok(vec![DokkuCommand::AppsDestroy { app, force: true }])
            }
            JobSpec::AppLock { app } => {
                let app = parse_app(app)?;
                Ok(vec![DokkuCommand::AppsLock { app }])
            }
            JobSpec::AppUnlock { app } => {
                let app = parse_app(app)?;
                Ok(vec![DokkuCommand::AppsUnlock { app }])
            }
            JobSpec::AppRename { app, new_name } => {
                let app = parse_app(app)?;
                let new_name = parse_app(new_name)?;
                Ok(vec![DokkuCommand::AppsRename { app, new_name }])
            }
            JobSpec::DomainsAdd { app, domains } => {
                let app = parse_app(app)?;
                let domains = parse_domains(domains)?;
                Ok(vec![DokkuCommand::DomainsAdd { app, domains }])
            }
            JobSpec::DomainsRemove { app, domains } => {
                let app = parse_app(app)?;
                let domains = parse_domains(domains)?;
                Ok(vec![DokkuCommand::DomainsRemove { app, domains }])
            }
            JobSpec::DomainsSet { app, domains } => {
                let app = parse_app(app)?;
                let domains = parse_domains(domains)?;
                Ok(vec![DokkuCommand::DomainsSet { app, domains }])
            }
            JobSpec::ResourceLimit {
                app,
                process_type,
                cpu,
                memory,
                memory_swap,
            } => {
                let app = parse_app(app)?;
                let process_type = parse_process_type(process_type)?;
                let cpu = parse_optional_resource(cpu)?;
                let memory = parse_optional_resource(memory)?;
                let memory_swap = parse_optional_resource(memory_swap)?;
                if cpu.is_none() && memory.is_none() && memory_swap.is_none() {
                    return Err(JobSpecError::Invalid(
                        "no resource values provided".to_owned(),
                    ));
                }
                Ok(vec![DokkuCommand::ResourceLimit {
                    app,
                    process_type,
                    cpu,
                    memory,
                    memory_swap,
                }])
            }
            JobSpec::ResourceReserve {
                app,
                process_type,
                cpu,
                memory,
            } => {
                let app = parse_app(app)?;
                let process_type = parse_process_type(process_type)?;
                let cpu = parse_optional_resource(cpu)?;
                let memory = parse_optional_resource(memory)?;
                if cpu.is_none() && memory.is_none() {
                    return Err(JobSpecError::Invalid(
                        "no resource values provided".to_owned(),
                    ));
                }
                Ok(vec![DokkuCommand::ResourceReserve {
                    app,
                    process_type,
                    cpu,
                    memory,
                }])
            }
            JobSpec::ResourceLimitClear { app, process_type } => {
                let app = parse_app(app)?;
                let process_type = parse_process_type(process_type)?;
                Ok(vec![DokkuCommand::ResourceLimitClear { app, process_type }])
            }
            JobSpec::ResourceReserveClear { app, process_type } => {
                let app = parse_app(app)?;
                let process_type = parse_process_type(process_type)?;
                Ok(vec![DokkuCommand::ResourceReserveClear {
                    app,
                    process_type,
                }])
            }
            JobSpec::CronRun { app, cron_id } => {
                let app = parse_app(app)?;
                let cron_id = parse_cron_id(cron_id)?;
                Ok(vec![DokkuCommand::CronRun { app, cron_id }])
            }
            JobSpec::CronSuspend { app, cron_id } => {
                let app = parse_app(app)?;
                let cron_id = parse_cron_id(cron_id)?;
                Ok(vec![DokkuCommand::CronSuspend { app, cron_id }])
            }
            JobSpec::CronResume { app, cron_id } => {
                let app = parse_app(app)?;
                let cron_id = parse_cron_id(cron_id)?;
                Ok(vec![DokkuCommand::CronResume { app, cron_id }])
            }
            JobSpec::MaintenanceEnable { app } => {
                let app = parse_app(app)?;
                Ok(vec![DokkuCommand::MaintenanceEnable { app }])
            }
            JobSpec::MaintenanceDisable { app } => {
                let app = parse_app(app)?;
                Ok(vec![DokkuCommand::MaintenanceDisable { app }])
            }
            JobSpec::HttpAuthEnable { app } => {
                let app = parse_app(app)?;
                Ok(vec![DokkuCommand::HttpAuthEnable { app }])
            }
            JobSpec::HttpAuthDisable { app } => {
                let app = parse_app(app)?;
                Ok(vec![DokkuCommand::HttpAuthDisable { app }])
            }
            JobSpec::HttpAuthAddUser {
                app,
                username,
                password,
            } => {
                let app = parse_app(app)?;
                let username = parse_username(username)?;
                if !is_valid_config_value(password) {
                    return Err(JobSpecError::Invalid(
                        "invalid password (single-line and quote-free required)".to_owned(),
                    ));
                }
                Ok(vec![DokkuCommand::HttpAuthAddUser {
                    app,
                    username,
                    password: password.clone(),
                }])
            }
            JobSpec::HttpAuthRemoveUser { app, username } => {
                let app = parse_app(app)?;
                let username = parse_username(username)?;
                Ok(vec![DokkuCommand::HttpAuthRemoveUser { app, username }])
            }
            JobSpec::BuildpacksSet {
                app,
                buildpack,
                index,
            } => {
                let app = parse_app(app)?;
                let buildpack = parse_buildpack(buildpack)?;
                let index = parse_buildpack_index(*index)?;
                Ok(vec![DokkuCommand::BuildpacksSet {
                    app,
                    buildpack,
                    index,
                }])
            }
            JobSpec::BuildpacksAdd {
                app,
                buildpack,
                index,
            } => {
                let app = parse_app(app)?;
                let buildpack = parse_buildpack(buildpack)?;
                let index = parse_buildpack_index(*index)?;
                Ok(vec![DokkuCommand::BuildpacksAdd {
                    app,
                    buildpack,
                    index,
                }])
            }
            JobSpec::BuildpacksRemove { app, buildpack } => {
                let app = parse_app(app)?;
                let buildpack = parse_buildpack(buildpack)?;
                Ok(vec![DokkuCommand::BuildpacksRemove { app, buildpack }])
            }
            JobSpec::BuildpacksClear { app } => {
                let app = parse_app(app)?;
                Ok(vec![DokkuCommand::BuildpacksClear { app }])
            }
            JobSpec::BuilderSet {
                app,
                property,
                value,
            } => {
                let app = parse_app(app)?;
                if !is_valid_builder_property(property) {
                    return Err(JobSpecError::Invalid(format!(
                        "invalid builder property `{property}`"
                    )));
                }
                let value = match value {
                    None => None,
                    Some(value) if value.is_empty() => None,
                    Some(value) => Some(value.clone()),
                };
                if let Some(value) = &value {
                    if !is_valid_builder_value(property, value) {
                        return Err(JobSpecError::Invalid(format!(
                            "invalid builder value `{value}`"
                        )));
                    }
                }
                Ok(vec![DokkuCommand::BuilderSet {
                    app,
                    property: property.clone(),
                    value,
                }])
            }
            JobSpec::ServiceAction {
                plugin,
                service,
                action,
            } => {
                let (plugin, service) = parse_service(plugin, service)?;
                let command = match action {
                    ServiceAction::Start => DokkuCommand::ServiceStart { plugin, service },
                    ServiceAction::Stop => DokkuCommand::ServiceStop { plugin, service },
                    ServiceAction::Restart => DokkuCommand::ServiceRestart { plugin, service },
                };
                Ok(vec![command])
            }
            JobSpec::ServiceCreate {
                plugin,
                service,
                options,
            } => {
                let (plugin, service) = parse_service(plugin, service)?;
                options
                    .validate()
                    .map_err(|err| JobSpecError::Invalid(err.to_string()))?;
                Ok(vec![DokkuCommand::ServiceCreate {
                    plugin,
                    service,
                    options: options.clone(),
                }])
            }
            JobSpec::ServiceDestroy { plugin, service } => {
                let (plugin, service) = parse_service(plugin, service)?;
                Ok(vec![DokkuCommand::ServiceDestroy {
                    plugin,
                    service,
                    force: true,
                }])
            }
            JobSpec::ServiceExpose {
                plugin,
                service,
                ports,
            } => {
                let (plugin, service) = parse_service(plugin, service)?;
                Ok(vec![DokkuCommand::ServiceExpose {
                    plugin,
                    service,
                    ports: ports.clone(),
                }])
            }
            JobSpec::ServiceUnexpose { plugin, service } => {
                let (plugin, service) = parse_service(plugin, service)?;
                Ok(vec![DokkuCommand::ServiceUnexpose { plugin, service }])
            }
            JobSpec::ServiceLink {
                plugin,
                service,
                app,
            } => {
                let (plugin, service) = parse_service(plugin, service)?;
                let app = parse_app(app)?;
                Ok(vec![DokkuCommand::ServiceLink {
                    plugin,
                    service,
                    app,
                }])
            }
            JobSpec::ServiceUnlink {
                plugin,
                service,
                app,
            } => {
                let (plugin, service) = parse_service(plugin, service)?;
                let app = parse_app(app)?;
                Ok(vec![DokkuCommand::ServiceUnlink {
                    plugin,
                    service,
                    app,
                }])
            }
            JobSpec::ConfigSet { app, vars } => {
                let app = parse_app(app)?;
                for var in vars {
                    if !is_valid_config_key(&var.key) || !is_valid_config_value(&var.value) {
                        return Err(JobSpecError::Invalid(format!(
                            "config key/value rejected: {}",
                            var.key
                        )));
                    }
                }
                Ok(vec![DokkuCommand::ConfigSet {
                    app,
                    vars: vars.clone(),
                }])
            }
            JobSpec::ConfigUnset { app, keys } => {
                let app = parse_app(app)?;
                for key in keys {
                    if !is_valid_config_key(key) {
                        return Err(JobSpecError::Invalid(format!("config key rejected: {key}")));
                    }
                }
                Ok(vec![DokkuCommand::ConfigUnset {
                    app,
                    keys: keys.clone(),
                }])
            }
            JobSpec::GitSet {
                app,
                property,
                value,
            } => {
                let app = parse_app(app)?;
                if property != DEPLOY_BRANCH_PROPERTY {
                    return Err(JobSpecError::Invalid(format!(
                        "invalid git property `{property}`"
                    )));
                }
                let value = match value {
                    None => None,
                    Some(value) if value.trim().is_empty() => None,
                    Some(value) => {
                        if !is_valid_git_ref(value) {
                            return Err(JobSpecError::Invalid(format!(
                                "invalid deploy branch `{value}`"
                            )));
                        }
                        Some(value.clone())
                    }
                };
                Ok(vec![DokkuCommand::GitSet {
                    app,
                    property: property.clone(),
                    value,
                }])
            }
            JobSpec::GitSync {
                app,
                repo,
                git_ref,
                build_mode,
            } => {
                let app = parse_app(app)?;
                if !is_valid_git_remote(repo) {
                    return Err(JobSpecError::Invalid(format!(
                        "invalid git remote `{repo}`"
                    )));
                }
                let git_ref = match git_ref {
                    None => None,
                    Some(git_ref) if git_ref.trim().is_empty() => None,
                    Some(git_ref) => {
                        if !is_valid_git_ref(git_ref) {
                            return Err(JobSpecError::Invalid(format!(
                                "invalid git ref `{git_ref}`"
                            )));
                        }
                        Some(git_ref.clone())
                    }
                };
                Ok(vec![DokkuCommand::GitSync {
                    app,
                    repo: repo.clone(),
                    git_ref,
                    build_mode: *build_mode,
                }])
            }
            JobSpec::GitFromImage { app, image } => {
                let app = parse_app(app)?;
                if !is_valid_image_ref(image) {
                    return Err(JobSpecError::Invalid(format!(
                        "invalid image ref `{image}`"
                    )));
                }
                Ok(vec![DokkuCommand::GitFromImage {
                    app,
                    image: image.clone(),
                }])
            }
            JobSpec::GitFromArchive { app, archive_url } => {
                let app = parse_app(app)?;
                if !is_valid_archive_url(archive_url) {
                    return Err(JobSpecError::Invalid(format!(
                        "invalid archive url `{archive_url}`"
                    )));
                }
                Ok(vec![DokkuCommand::GitFromArchive {
                    app,
                    archive_url: archive_url.clone(),
                }])
            }
            JobSpec::LetsencryptAction { app, action } => {
                let app = parse_app(app)?;
                let command = match action {
                    LetsencryptAction::Enable => DokkuCommand::LetsencryptEnable { app },
                    LetsencryptAction::Disable => DokkuCommand::LetsencryptDisable { app },
                    LetsencryptAction::Revoke => DokkuCommand::LetsencryptRevoke { app },
                    LetsencryptAction::Cleanup => DokkuCommand::LetsencryptCleanup { app },
                };
                Ok(vec![command])
            }
            JobSpec::LetsencryptCronJob { add } => {
                Ok(vec![DokkuCommand::LetsencryptCronJob { add: *add }])
            }
            JobSpec::LetsencryptSet {
                app,
                property,
                value,
            } => {
                let app = parse_app(app)?;
                let value = value.clone().filter(|value| !value.trim().is_empty());
                let property = match property.as_str() {
                    "email" => {
                        let Some(email) = value.as_deref() else {
                            return Err(JobSpecError::Invalid(
                                "letsencrypt email is required".to_owned(),
                            ));
                        };
                        if !is_valid_letsencrypt_email(email) {
                            return Err(JobSpecError::Invalid(format!(
                                "invalid letsencrypt email `{email}`"
                            )));
                        }
                        "email"
                    }
                    "staging" => {
                        match value.as_deref() {
                            Some("true") | Some("false") => {}
                            Some(other) => {
                                return Err(JobSpecError::Invalid(format!(
                                    "invalid letsencrypt staging value `{other}`"
                                )));
                            }
                            None => {
                                return Err(JobSpecError::Invalid(
                                    "letsencrypt staging requires a value".to_owned(),
                                ));
                            }
                        }
                        "staging"
                    }
                    other => {
                        return Err(JobSpecError::Invalid(format!(
                            "invalid letsencrypt property `{other}`"
                        )));
                    }
                };
                Ok(vec![DokkuCommand::LetsencryptSet {
                    app,
                    property: property.to_owned(),
                    value,
                }])
            }
            JobSpec::VolumeMount { app, spec } => {
                let app = parse_app(app)?;
                let mount = MountSpec::try_from(spec.as_str())
                    .map_err(|err| JobSpecError::Invalid(err.to_string()))?;
                Ok(vec![DokkuCommand::StorageMount { app, mount }])
            }
            JobSpec::VolumeUnmount { app, locator } => {
                let app = parse_app(app)?;
                let mount = MountSpec::try_from(locator.as_str())
                    .map_err(|err| JobSpecError::Invalid(err.to_string()))?;
                Ok(vec![DokkuCommand::StorageUnmount { app, mount }])
            }
        }
    }
}

fn parse_domains(raw: &[String]) -> Result<Vec<DomainName>, JobSpecError> {
    if raw.is_empty() {
        return Err(JobSpecError::Invalid("no domains provided".to_owned()));
    }
    raw.iter()
        .map(|domain| {
            DomainName::try_from(domain.as_str())
                .map_err(|err| JobSpecError::Invalid(err.to_string()))
        })
        .collect()
}

fn parse_buildpack(raw: &str) -> Result<String, JobSpecError> {
    if !is_valid_buildpack(raw) {
        return Err(JobSpecError::Invalid(format!("invalid buildpack `{raw}`")));
    }
    Ok(raw.to_owned())
}

fn parse_buildpack_index(index: Option<u32>) -> Result<Option<u32>, JobSpecError> {
    match index {
        None => Ok(None),
        Some(index) if is_valid_buildpack_index(index) => Ok(Some(index)),
        Some(index) => Err(JobSpecError::Invalid(format!(
            "invalid buildpack index `{index}`"
        ))),
    }
}

fn parse_username(raw: &str) -> Result<String, JobSpecError> {
    if !is_valid_username(raw) {
        return Err(JobSpecError::Invalid(format!("invalid username `{raw}`")));
    }
    Ok(raw.to_owned())
}

fn parse_cron_id(raw: &str) -> Result<String, JobSpecError> {
    if !is_valid_cron_id(raw) {
        return Err(JobSpecError::Invalid(format!("invalid cron id `{raw}`")));
    }
    Ok(raw.to_owned())
}

fn parse_process_type(raw: &str) -> Result<String, JobSpecError> {
    if !is_valid_process_type(raw) {
        return Err(JobSpecError::Invalid(format!(
            "invalid process type `{raw}`"
        )));
    }
    Ok(raw.to_owned())
}

fn parse_optional_resource(raw: &Option<String>) -> Result<Option<String>, JobSpecError> {
    match raw {
        None => Ok(None),
        Some(value) if value.is_empty() => Ok(None),
        Some(value) if is_valid_resource_value(value) => Ok(Some(value.clone())),
        Some(value) => Err(JobSpecError::Invalid(format!(
            "invalid resource value `{value}`"
        ))),
    }
}

fn parse_app(raw: &str) -> Result<AppName, JobSpecError> {
    AppName::try_from(raw).map_err(|err| JobSpecError::Invalid(err.to_string()))
}

fn parse_service(
    plugin: &str,
    service: &str,
) -> Result<(ServicePlugin, ServiceName), JobSpecError> {
    let plugin =
        ServicePlugin::try_from(plugin).map_err(|err| JobSpecError::Invalid(err.to_string()))?;
    let service =
        ServiceName::try_from(service).map_err(|err| JobSpecError::Invalid(err.to_string()))?;
    Ok((plugin, service))
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JobSpecError {
    #[error("invalid job payload: {0}")]
    Invalid(String),
}

/// What the run's `done` event should carry and what to refresh afterwards.
/// Serialized into the job so any container's worker can finish the run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionSpec {
    pub success_message: String,
    pub redirect: Option<String>,
    pub refresh: CompletionRefresh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompletionRefresh {
    Reports,
    All,
    None,
}

/// The serialized body of an `action_jobs` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobPayload {
    pub plan: Vec<JobSpec>,
    pub completion: CompletionSpec,
    /// Literal secrets (config values, http-auth passwords) redacted from
    /// every persisted run line for this job.
    #[serde(default)]
    pub redactions: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_action_rehydrates_to_the_matching_command() {
        for (action, expected) in [
            (
                AppAction::Start,
                DokkuCommand::PsStart { app: app("alpha") },
            ),
            (AppAction::Stop, DokkuCommand::PsStop { app: app("alpha") }),
            (
                AppAction::Restart,
                DokkuCommand::PsRestart { app: app("alpha") },
            ),
            (
                AppAction::Rebuild,
                DokkuCommand::PsRebuild { app: app("alpha") },
            ),
        ] {
            let job = JobSpec::AppAction {
                app: "alpha".to_owned(),
                action,
            };
            assert_eq!(job.to_commands().expect("commands"), vec![expected]);
        }
    }

    #[test]
    fn scale_rehydrates_with_its_entries() {
        let job = JobSpec::AppScale {
            app: "alpha".to_owned(),
            scales: vec![ScaleEntry::new("web", 2)],
        };
        assert_eq!(
            job.to_commands().expect("commands"),
            vec![DokkuCommand::PsScaleSet {
                app: app("alpha"),
                scales: vec![ScaleEntry::new("web", 2)],
            }]
        );
    }

    #[test]
    fn destroy_rehydrates_with_force() {
        assert_eq!(
            JobSpec::AppDestroy {
                app: "alpha".to_owned()
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::AppsDestroy {
                app: app("alpha"),
                force: true,
            }]
        );
    }

    #[test]
    fn service_specs_rehydrate_through_the_plugin_gate() {
        let job = JobSpec::ServiceCreate {
            plugin: "redis".into(),
            service: "candid".into(),
            options: ServiceCreateOptions::default(),
        };
        assert_eq!(
            job.to_commands().expect("commands"),
            vec![DokkuCommand::ServiceCreate {
                plugin: crate::domain::ServicePlugin::try_from("redis").expect("redis"),
                service: crate::domain::ServiceName::try_from("candid").expect("candid"),
                options: ServiceCreateOptions::default(),
            }]
        );
        let bad = JobSpec::ServiceCreate {
            plugin: "maria".into(),
            service: "candid".into(),
            options: ServiceCreateOptions::default(),
        };
        assert!(bad.to_commands().is_err(), "unknown plugins are rejected");
        let bad = JobSpec::ServiceCreate {
            plugin: "redis".into(),
            service: "Bad Name".into(),
            options: ServiceCreateOptions::default(),
        };
        assert!(
            bad.to_commands().is_err(),
            "invalid service names are rejected"
        );
    }

    #[test]
    fn service_create_options_revalidate_on_rehydration() {
        let job = JobSpec::ServiceCreate {
            plugin: "redis".into(),
            service: "candid".into(),
            options: ServiceCreateOptions {
                image: Some("redis".into()),
                image_version: Some("7.2".into()),
                custom_env: Some("USER=alpha".into()),
                config_options: Some("--appendonly yes".into()),
            },
        };
        let commands = job.to_commands().expect("commands");
        let DokkuCommand::ServiceCreate { options, .. } = &commands[0] else {
            panic!("expected service create");
        };
        assert_eq!(options.image_version.as_deref(), Some("7.2"));

        let tampered = JobSpec::ServiceCreate {
            plugin: "redis".into(),
            service: "candid".into(),
            options: ServiceCreateOptions {
                config_options: Some("bad'quote".into()),
                ..Default::default()
            },
        };
        assert!(
            tampered.to_commands().is_err(),
            "tampered options must not reach the host"
        );
    }

    #[test]
    fn volume_specs_revalidate_the_mount_spec() {
        assert_eq!(
            JobSpec::VolumeMount {
                app: "alpha".into(),
                spec: "/host:/data:ro".into(),
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::StorageMount {
                app: app("alpha"),
                mount: MountSpec::try_from("/host:/data:ro").expect("mount"),
            }]
        );
        let bad = JobSpec::VolumeMount {
            app: "alpha".into(),
            spec: "relative:path".into(),
        };
        assert!(
            bad.to_commands().is_err(),
            "relative host paths are rejected"
        );
    }

    #[test]
    fn tampered_app_names_are_rejected_at_rehydration() {
        let job = JobSpec::AppAction {
            app: "Bad_App".into(),
            action: AppAction::Restart,
        };
        assert!(job.to_commands().is_err());
    }

    #[test]
    fn lock_unlock_and_rename_rehydrate() {
        assert_eq!(
            JobSpec::AppLock {
                app: "alpha".into()
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::AppsLock { app: app("alpha") }]
        );
        assert_eq!(
            JobSpec::AppUnlock {
                app: "alpha".into()
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::AppsUnlock { app: app("alpha") }]
        );
        assert_eq!(
            JobSpec::AppRename {
                app: "alpha".into(),
                new_name: "beta".into(),
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::AppsRename {
                app: app("alpha"),
                new_name: app("beta"),
            }]
        );
        let bad = JobSpec::AppRename {
            app: "alpha".into(),
            new_name: "Bad_Name".into(),
        };
        assert!(bad.to_commands().is_err(), "invalid new names are rejected");
    }

    #[test]
    fn domain_specs_rehydrate_and_revalidate() {
        assert_eq!(
            JobSpec::DomainsAdd {
                app: "alpha".into(),
                domains: vec!["one.example.com".into()],
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::DomainsAdd {
                app: app("alpha"),
                domains: vec![DomainName::try_from("one.example.com").expect("d")],
            }]
        );
        assert_eq!(
            JobSpec::DomainsSet {
                app: "alpha".into(),
                domains: vec!["one.example.com".into(), "two.example.com".into()],
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::DomainsSet {
                app: app("alpha"),
                domains: vec![
                    DomainName::try_from("one.example.com").expect("d"),
                    DomainName::try_from("two.example.com").expect("d"),
                ],
            }]
        );
        assert!(
            JobSpec::DomainsAdd {
                app: "alpha".into(),
                domains: vec!["bad_domain".into()],
            }
            .to_commands()
            .is_err(),
            "invalid domains are rejected"
        );
        assert!(
            JobSpec::DomainsRemove {
                app: "alpha".into(),
                domains: Vec::new(),
            }
            .to_commands()
            .is_err(),
            "empty domain lists are rejected"
        );
    }

    #[test]
    fn resource_specs_rehydrate_and_revalidate() {
        assert_eq!(
            JobSpec::ResourceLimit {
                app: "alpha".into(),
                process_type: "web".into(),
                cpu: Some("1".into()),
                memory: Some("128".into()),
                memory_swap: None,
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::ResourceLimit {
                app: app("alpha"),
                process_type: "web".into(),
                cpu: Some("1".into()),
                memory: Some("128".into()),
                memory_swap: None,
            }]
        );
        assert_eq!(
            JobSpec::ResourceReserve {
                app: "alpha".into(),
                process_type: "worker".into(),
                cpu: None,
                memory: Some("512".into()),
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::ResourceReserve {
                app: app("alpha"),
                process_type: "worker".into(),
                cpu: None,
                memory: Some("512".into()),
            }]
        );
        assert_eq!(
            JobSpec::ResourceLimitClear {
                app: "alpha".into(),
                process_type: "web".into(),
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::ResourceLimitClear {
                app: app("alpha"),
                process_type: "web".into(),
            }]
        );
        assert!(
            JobSpec::ResourceLimit {
                app: "alpha".into(),
                process_type: "web.1".into(),
                cpu: Some("1".into()),
                memory: None,
                memory_swap: None,
            }
            .to_commands()
            .is_err(),
            "invalid process types are rejected"
        );
        assert!(
            JobSpec::ResourceLimit {
                app: "alpha".into(),
                process_type: "web".into(),
                cpu: Some("1 2".into()),
                memory: None,
                memory_swap: None,
            }
            .to_commands()
            .is_err(),
            "invalid resource values are rejected"
        );
        assert!(
            JobSpec::ResourceReserve {
                app: "alpha".into(),
                process_type: "web".into(),
                cpu: None,
                memory: None,
            }
            .to_commands()
            .is_err(),
            "empty resource sets are rejected"
        );
    }

    #[test]
    fn cron_specs_rehydrate_and_revalidate() {
        assert_eq!(
            JobSpec::CronRun {
                app: "alpha".into(),
                cron_id: "a1b2c3".into(),
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::CronRun {
                app: app("alpha"),
                cron_id: "a1b2c3".into(),
            }]
        );
        assert_eq!(
            JobSpec::CronSuspend {
                app: "alpha".into(),
                cron_id: "a1b2c3".into(),
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::CronSuspend {
                app: app("alpha"),
                cron_id: "a1b2c3".into(),
            }]
        );
        assert_eq!(
            JobSpec::CronResume {
                app: "alpha".into(),
                cron_id: "a1b2c3".into(),
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::CronResume {
                app: app("alpha"),
                cron_id: "a1b2c3".into(),
            }]
        );
        assert!(
            JobSpec::CronRun {
                app: "alpha".into(),
                cron_id: "bad id".into(),
            }
            .to_commands()
            .is_err(),
            "invalid cron ids are rejected"
        );
    }

    #[test]
    fn maintenance_and_http_auth_specs_rehydrate_and_revalidate() {
        assert_eq!(
            JobSpec::MaintenanceEnable {
                app: "alpha".into()
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::MaintenanceEnable { app: app("alpha") }]
        );
        assert_eq!(
            JobSpec::HttpAuthDisable {
                app: "alpha".into()
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::HttpAuthDisable { app: app("alpha") }]
        );
        assert_eq!(
            JobSpec::HttpAuthAddUser {
                app: "alpha".into(),
                username: "alice".into(),
                password: "s3cr3t".into(),
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::HttpAuthAddUser {
                app: app("alpha"),
                username: "alice".into(),
                password: "s3cr3t".into(),
            }]
        );
        assert!(
            JobSpec::HttpAuthAddUser {
                app: "alpha".into(),
                username: "bad user".into(),
                password: "x".into(),
            }
            .to_commands()
            .is_err(),
            "invalid usernames are rejected"
        );
        assert!(
            JobSpec::HttpAuthAddUser {
                app: "alpha".into(),
                username: "alice".into(),
                password: "it's".into(),
            }
            .to_commands()
            .is_err(),
            "quote-carrying passwords are rejected"
        );
    }

    #[test]
    fn payload_redactions_roundtrip() {
        let payload = JobPayload {
            plan: vec![JobSpec::HttpAuthAddUser {
                app: "alpha".into(),
                username: "alice".into(),
                password: "s3cr3t".into(),
            }],
            completion: CompletionSpec {
                success_message: "done".into(),
                redirect: None,
                refresh: CompletionRefresh::None,
            },
            redactions: vec!["s3cr3t".into()],
        };
        let json = serde_json::to_string(&payload).expect("serialize");
        let parsed: JobPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.redactions, vec!["s3cr3t".to_owned()]);
    }

    #[test]
    fn build_config_specs_rehydrate_and_revalidate() {
        assert_eq!(
            JobSpec::BuildpacksSet {
                app: "alpha".into(),
                buildpack: "https://example.com/bp".into(),
                index: Some(2),
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::BuildpacksSet {
                app: app("alpha"),
                buildpack: "https://example.com/bp".into(),
                index: Some(2),
            }]
        );
        assert_eq!(
            JobSpec::BuildpacksAdd {
                app: "alpha".into(),
                buildpack: "https://example.com/bp".into(),
                index: None,
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::BuildpacksAdd {
                app: app("alpha"),
                buildpack: "https://example.com/bp".into(),
                index: None,
            }]
        );
        assert_eq!(
            JobSpec::BuildpacksRemove {
                app: "alpha".into(),
                buildpack: "https://example.com/bp".into(),
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::BuildpacksRemove {
                app: app("alpha"),
                buildpack: "https://example.com/bp".into(),
            }]
        );
        assert_eq!(
            JobSpec::BuildpacksClear {
                app: "alpha".into()
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::BuildpacksClear { app: app("alpha") }]
        );
        assert_eq!(
            JobSpec::BuilderSet {
                app: "alpha".into(),
                property: "selected".into(),
                value: Some("dockerfile".into()),
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::BuilderSet {
                app: app("alpha"),
                property: "selected".into(),
                value: Some("dockerfile".into()),
            }]
        );
        assert!(
            JobSpec::BuildpacksSet {
                app: "alpha".into(),
                buildpack: "it's".into(),
                index: None,
            }
            .to_commands()
            .is_err(),
            "quote-carrying buildpacks are rejected"
        );
        assert!(
            JobSpec::BuildpacksSet {
                app: "alpha".into(),
                buildpack: "https://example.com/bp".into(),
                index: Some(0),
            }
            .to_commands()
            .is_err(),
            "index 0 is rejected"
        );
        assert!(
            JobSpec::BuilderSet {
                app: "alpha".into(),
                property: "detected".into(),
                value: Some("dockerfile".into()),
            }
            .to_commands()
            .is_err(),
            "read-only properties are rejected"
        );
        assert!(
            JobSpec::BuilderSet {
                app: "alpha".into(),
                property: "selected".into(),
                value: Some("podman".into()),
            }
            .to_commands()
            .is_err(),
            "unknown builders are rejected"
        );
    }

    #[test]
    fn destructive_specs_are_flagged() {
        assert!(
            JobSpec::AppDestroy {
                app: "alpha".into()
            }
            .is_destructive()
        );
        assert!(
            JobSpec::ServiceDestroy {
                plugin: "redis".into(),
                service: "candid".into(),
            }
            .is_destructive()
        );
        assert!(
            !JobSpec::AppAction {
                app: "alpha".into(),
                action: AppAction::Restart,
            }
            .is_destructive()
        );
    }

    #[test]
    fn config_specs_rehydrate_and_revalidate() {
        let set = JobSpec::ConfigSet {
            app: "alpha".into(),
            vars: vec![EnvVar {
                key: "FOO".into(),
                value: "bar".into(),
            }],
        };
        assert_eq!(
            set.to_commands().expect("commands"),
            vec![DokkuCommand::ConfigSet {
                app: app("alpha"),
                vars: vec![EnvVar {
                    key: "FOO".into(),
                    value: "bar".into(),
                }],
            }]
        );
        let bad_key = JobSpec::ConfigSet {
            app: "alpha".into(),
            vars: vec![EnvVar {
                key: "FOO-BAR".into(),
                value: "x".into(),
            }],
        };
        assert!(bad_key.to_commands().is_err(), "invalid keys are rejected");
        let bad_value = JobSpec::ConfigSet {
            app: "alpha".into(),
            vars: vec![EnvVar {
                key: "FOO".into(),
                value: "it's unsafe".into(),
            }],
        };
        assert!(
            bad_value.to_commands().is_err(),
            "quote-carrying values are rejected"
        );
        assert_eq!(
            JobSpec::ConfigUnset {
                app: "alpha".into(),
                keys: vec!["GONE".into()],
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::ConfigUnset {
                app: app("alpha"),
                keys: vec!["GONE".into()],
            }]
        );
    }

    #[test]
    fn git_set_spec_rehydrates_and_revalidates() {
        assert_eq!(
            JobSpec::GitSet {
                app: "alpha".into(),
                property: "deploy-branch".into(),
                value: Some("main".into()),
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::GitSet {
                app: app("alpha"),
                property: "deploy-branch".into(),
                value: Some("main".into()),
            }]
        );
        assert_eq!(
            JobSpec::GitSet {
                app: "alpha".into(),
                property: "deploy-branch".into(),
                value: None,
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::GitSet {
                app: app("alpha"),
                property: "deploy-branch".into(),
                value: None,
            }]
        );
        assert_eq!(
            JobSpec::GitSet {
                app: "alpha".into(),
                property: "deploy-branch".into(),
                value: Some("  ".into()),
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::GitSet {
                app: app("alpha"),
                property: "deploy-branch".into(),
                value: None,
            }],
            "blank values clear like an omitted value"
        );
        assert!(
            JobSpec::GitSet {
                app: "alpha".into(),
                property: "keep-git-dir".into(),
                value: Some("true".into()),
            }
            .to_commands()
            .is_err(),
            "unknown git properties are rejected"
        );
        assert!(
            JobSpec::GitSet {
                app: "alpha".into(),
                property: "deploy-branch".into(),
                value: Some("it's".into()),
            }
            .to_commands()
            .is_err(),
            "invalid branch names are rejected"
        );
    }

    #[test]
    fn git_deploy_specs_rehydrate_and_revalidate() {
        assert_eq!(
            JobSpec::GitSync {
                app: "alpha".into(),
                repo: "https://github.com/org/repo.git".into(),
                git_ref: Some("main".into()),
                build_mode: GitBuildMode::BuildIfChanges,
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::GitSync {
                app: app("alpha"),
                repo: "https://github.com/org/repo.git".into(),
                git_ref: Some("main".into()),
                build_mode: GitBuildMode::BuildIfChanges,
            }]
        );
        assert_eq!(
            JobSpec::GitFromImage {
                app: "alpha".into(),
                image: "ghcr.io/org/app:v1".into(),
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::GitFromImage {
                app: app("alpha"),
                image: "ghcr.io/org/app:v1".into(),
            }]
        );
        assert_eq!(
            JobSpec::GitFromArchive {
                app: "alpha".into(),
                archive_url: "https://example.com/app.tar.gz".into(),
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::GitFromArchive {
                app: app("alpha"),
                archive_url: "https://example.com/app.tar.gz".into(),
            }]
        );
        assert!(
            JobSpec::GitSync {
                app: "alpha".into(),
                repo: "github.com/org/repo".into(),
                git_ref: None,
                build_mode: GitBuildMode::NoBuild,
            }
            .to_commands()
            .is_err(),
            "scheme-less remotes are rejected"
        );
        assert!(
            JobSpec::GitSync {
                app: "alpha".into(),
                repo: "https://github.com/org/repo.git".into(),
                git_ref: Some("it's".into()),
                build_mode: GitBuildMode::Build,
            }
            .to_commands()
            .is_err(),
            "quote-carrying refs are rejected"
        );
        assert!(
            JobSpec::GitFromImage {
                app: "alpha".into(),
                image: "it's".into(),
            }
            .to_commands()
            .is_err(),
            "quote-carrying images are rejected"
        );
        assert!(
            JobSpec::GitFromArchive {
                app: "alpha".into(),
                archive_url: "ftp://example.com/app.tar.gz".into(),
            }
            .to_commands()
            .is_err(),
            "non-http archives are rejected"
        );
    }

    #[test]
    fn letsencrypt_specs_rehydrate_and_revalidate() {
        for (action, expected) in [
            (
                LetsencryptAction::Enable,
                DokkuCommand::LetsencryptEnable { app: app("alpha") },
            ),
            (
                LetsencryptAction::Disable,
                DokkuCommand::LetsencryptDisable { app: app("alpha") },
            ),
            (
                LetsencryptAction::Revoke,
                DokkuCommand::LetsencryptRevoke { app: app("alpha") },
            ),
            (
                LetsencryptAction::Cleanup,
                DokkuCommand::LetsencryptCleanup { app: app("alpha") },
            ),
        ] {
            assert_eq!(
                JobSpec::LetsencryptAction {
                    app: "alpha".into(),
                    action,
                }
                .to_commands()
                .expect("commands"),
                vec![expected]
            );
        }
        assert_eq!(
            JobSpec::LetsencryptCronJob { add: true }
                .to_commands()
                .expect("commands"),
            vec![DokkuCommand::LetsencryptCronJob { add: true }]
        );
        assert_eq!(
            JobSpec::LetsencryptSet {
                app: "alpha".into(),
                property: "email".into(),
                value: Some("ops@example.com".into()),
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::LetsencryptSet {
                app: app("alpha"),
                property: "email".into(),
                value: Some("ops@example.com".into()),
            }]
        );
        assert_eq!(
            JobSpec::LetsencryptSet {
                app: "alpha".into(),
                property: "staging".into(),
                value: Some("true".into()),
            }
            .to_commands()
            .expect("commands"),
            vec![DokkuCommand::LetsencryptSet {
                app: app("alpha"),
                property: "staging".into(),
                value: Some("true".into()),
            }]
        );
        for tampered in [
            JobSpec::LetsencryptSet {
                app: "alpha".into(),
                property: "root-password".into(),
                value: Some("x".into()),
            },
            JobSpec::LetsencryptSet {
                app: "alpha".into(),
                property: "email".into(),
                value: Some("not-an-email".into()),
            },
            JobSpec::LetsencryptSet {
                app: "alpha".into(),
                property: "email".into(),
                value: Some("o'brien@example.com".into()),
            },
            JobSpec::LetsencryptSet {
                app: "alpha".into(),
                property: "staging".into(),
                value: Some("yes".into()),
            },
            JobSpec::LetsencryptSet {
                app: "alpha".into(),
                property: "staging".into(),
                value: None,
            },
        ] {
            assert!(
                tampered.to_commands().is_err(),
                "tampered {tampered:?} must be rejected"
            );
        }
        assert!(
            JobSpec::LetsencryptAction {
                app: "Bad_App".into(),
                action: LetsencryptAction::Enable,
            }
            .to_commands()
            .is_err(),
            "tampered app names are rejected"
        );
    }

    #[test]
    fn job_payload_roundtrips_through_json() {
        let payload = JobPayload {
            plan: vec![JobSpec::AppAction {
                app: "alpha".into(),
                action: AppAction::Rebuild,
            }],
            completion: CompletionSpec {
                success_message: "Rebuilt 'alpha'.".into(),
                redirect: None,
                refresh: CompletionRefresh::Reports,
            },
            redactions: Vec::new(),
        };
        let json = serde_json::to_string(&payload).expect("serialize");
        let parsed: JobPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, payload);
        let commands = parsed.plan[0].to_commands().expect("commands");
        assert_eq!(
            commands,
            vec![DokkuCommand::PsRebuild { app: app("alpha") }]
        );
    }

    fn app(name: &str) -> AppName {
        AppName::try_from(name).expect("valid app name")
    }
}
