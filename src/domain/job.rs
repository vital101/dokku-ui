use serde::{Deserialize, Serialize};

use crate::domain::AppName;
use crate::domain::command::DokkuCommand;
use crate::domain::env_file::{is_valid_config_key, is_valid_config_value};
use crate::domain::mount_spec::MountSpec;
use crate::domain::service_name::ServiceName;
use crate::domain::service_plugin::ServicePlugin;
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
    ServiceAction {
        plugin: String,
        service: String,
        action: ServiceAction,
    },
    ServiceCreate {
        plugin: String,
        service: String,
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
            JobSpec::ServiceCreate { plugin, service } => {
                let (plugin, service) = parse_service(plugin, service)?;
                Ok(vec![DokkuCommand::ServiceCreate { plugin, service }])
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
        };
        assert_eq!(
            job.to_commands().expect("commands"),
            vec![DokkuCommand::ServiceCreate {
                plugin: crate::domain::ServicePlugin::try_from("redis").expect("redis"),
                service: crate::domain::ServiceName::try_from("candid").expect("candid"),
            }]
        );
        let bad = JobSpec::ServiceCreate {
            plugin: "maria".into(),
            service: "candid".into(),
        };
        assert!(bad.to_commands().is_err(), "unknown plugins are rejected");
        let bad = JobSpec::ServiceCreate {
            plugin: "redis".into(),
            service: "Bad Name".into(),
        };
        assert!(
            bad.to_commands().is_err(),
            "invalid service names are rejected"
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
