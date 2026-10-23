use serde::{Deserialize, Serialize};

use crate::domain::AppName;
use crate::domain::build::{BUILDERS, is_valid_buildpack};
use crate::domain::job::JobSpec;
use crate::domain::mount_spec::MountSpec;
use crate::domain::service_create::ServiceCreateOptions;
use crate::domain::service_name::ServiceName;
use crate::domain::service_plugin::ServicePlugin;
use crate::domain::storage_entry::{StorageEntryName, is_valid_volume_container_path};

pub const MAX_BUILDPACKS: usize = 20;
pub const MAX_SERVICES: usize = 10;
pub const MAX_VOLUMES: usize = 10;

/// The in-session draft of the app-creation wizard. Pure data: the web layer
/// persists it into the SQLite-backed session, `to_plan` turns it into the
/// exact persisted job plan. Values are re-trimmed and re-validated there, so
/// a stale or hand-edited draft can never reach the host unvalidated.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppWizardDraft {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub builder: Option<String>,
    #[serde(default)]
    pub buildpacks: Vec<String>,
    #[serde(default)]
    pub services: Vec<WizardService>,
    #[serde(default)]
    pub volumes: Vec<WizardVolume>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WizardService {
    pub plugin: String,
    pub service: String,
    /// true = create then link; false = link an existing service.
    pub create: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeMode {
    Create,
    AttachEntry,
    AttachPath,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WizardVolume {
    pub mode: VolumeMode,
    /// Entry name for Create/AttachEntry; host path or docker volume for AttachPath.
    pub source: String,
    pub container: String,
}

impl AppWizardDraft {
    /// Builds the exact command plan as persisted job specs. Order: app
    /// create, builder/buildpacks, service create/link, volume create/mount.
    /// Existence of services or storage entries is *not* checked here — that
    /// needs IO and lives in the web steps; job rehydration re-validates.
    pub fn to_plan(&self) -> Result<Vec<JobSpec>, WizardError> {
        let mut plan = Vec::new();

        let name = self.name.as_deref().map(str::trim).unwrap_or_default();
        if name.is_empty() {
            return Err(WizardError::MissingName);
        }
        let app = AppName::try_from(name)
            .map_err(|err| WizardError::InvalidName(err.to_string()))?
            .to_string();

        plan.push(JobSpec::AppCreate { app: app.clone() });

        let builder = self
            .builder
            .as_deref()
            .map(str::trim)
            .filter(|builder| !builder.is_empty());
        let buildpacks: Vec<String> = self
            .buildpacks
            .iter()
            .map(|buildpack| buildpack.trim().to_owned())
            .filter(|buildpack| !buildpack.is_empty())
            .collect();
        if buildpacks.len() > MAX_BUILDPACKS {
            return Err(WizardError::TooManyBuildpacks);
        }
        match builder {
            Some(builder) => {
                if !BUILDERS.contains(&builder) {
                    return Err(WizardError::InvalidBuilder(builder.to_owned()));
                }
                if !buildpacks.is_empty() && builder != "herokuish" {
                    return Err(WizardError::BuildpackBuilderConflict(builder.to_owned()));
                }
                plan.push(builder_set(&app, builder));
            }
            None if !buildpacks.is_empty() => {
                plan.push(builder_set(&app, "herokuish"));
            }
            None => {}
        }
        for buildpack in buildpacks {
            if !is_valid_buildpack(&buildpack) {
                return Err(WizardError::InvalidBuildpack(buildpack));
            }
            plan.push(JobSpec::BuildpacksAdd {
                app: app.clone(),
                buildpack,
                index: None,
            });
        }

        if self.services.len() > MAX_SERVICES {
            return Err(WizardError::TooManyServices);
        }
        let mut seen_services: Vec<(String, String)> = Vec::new();
        for service in &self.services {
            let plugin = ServicePlugin::try_from(service.plugin.trim())
                .map_err(|err| WizardError::InvalidServicePlugin(err.to_string()))?;
            let name = ServiceName::try_from(service.service.trim())
                .map_err(|err| WizardError::InvalidServiceName(err.to_string()))?;
            let key = (plugin.as_str().to_owned(), name.as_str().to_owned());
            if seen_services.contains(&key) {
                return Err(WizardError::DuplicateService(format!(
                    "{}:{}",
                    key.0, key.1
                )));
            }
            seen_services.push(key.clone());
            if service.create {
                plan.push(JobSpec::ServiceCreate {
                    plugin: key.0.clone(),
                    service: key.1.clone(),
                    options: ServiceCreateOptions::default(),
                });
            }
            plan.push(JobSpec::ServiceLink {
                plugin: key.0,
                service: key.1,
                app: app.clone(),
            });
        }

        if self.volumes.len() > MAX_VOLUMES {
            return Err(WizardError::TooManyVolumes);
        }
        let mut seen_volumes: Vec<(VolumeMode, String, String)> = Vec::new();
        for volume in &self.volumes {
            let source = volume.source.trim();
            let container = volume.container.trim();
            let key = (volume.mode, source.to_owned(), container.to_owned());
            if seen_volumes.contains(&key) {
                return Err(WizardError::DuplicateVolume(format!("{}:{}", key.1, key.2)));
            }
            seen_volumes.push(key.clone());
            match volume.mode {
                VolumeMode::Create => {
                    let entry = StorageEntryName::try_from_create(source)
                        .map_err(|err| WizardError::InvalidStorageEntry(err.to_string()))?;
                    if !is_valid_volume_container_path(container) {
                        return Err(WizardError::InvalidContainerPath(container.to_owned()));
                    }
                    plan.push(JobSpec::StorageCreate {
                        entry: entry.to_string(),
                    });
                    plan.push(JobSpec::StorageMountEntry {
                        app: app.clone(),
                        entry: entry.to_string(),
                        container: container.to_owned(),
                    });
                }
                VolumeMode::AttachEntry => {
                    // Shape-only parse: an existing `legacy-*` entry is a
                    // valid mount target even though it cannot be created.
                    let entry = StorageEntryName::try_from(source)
                        .map_err(|err| WizardError::InvalidStorageEntry(err.to_string()))?;
                    if !is_valid_volume_container_path(container) {
                        return Err(WizardError::InvalidContainerPath(container.to_owned()));
                    }
                    plan.push(JobSpec::StorageMountEntry {
                        app: app.clone(),
                        entry: entry.to_string(),
                        container: container.to_owned(),
                    });
                }
                VolumeMode::AttachPath => {
                    let mount = MountSpec::try_from(format!("{source}:{container}"))
                        .map_err(|err| WizardError::InvalidHostMount(err.to_string()))?;
                    plan.push(JobSpec::VolumeMount {
                        app: app.clone(),
                        spec: mount.arg(),
                    });
                }
            }
        }

        Ok(plan)
    }
}

fn builder_set(app: &str, builder: &str) -> JobSpec {
    JobSpec::BuilderSet {
        app: app.to_owned(),
        property: "selected".to_owned(),
        value: Some(builder.to_owned()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WizardError {
    #[error("app name must not be empty")]
    MissingName,
    #[error("invalid app name: {0}")]
    InvalidName(String),
    #[error("invalid builder `{0}`")]
    InvalidBuilder(String),
    #[error("invalid buildpack `{0}`")]
    InvalidBuildpack(String),
    #[error("buildpacks require the herokuish builder, but `{0}` is selected")]
    BuildpackBuilderConflict(String),
    #[error("too many buildpacks; at most {MAX_BUILDPACKS} are allowed")]
    TooManyBuildpacks,
    #[error("invalid service plugin: {0}")]
    InvalidServicePlugin(String),
    #[error("invalid service name: {0}")]
    InvalidServiceName(String),
    #[error("duplicate service `{0}`")]
    DuplicateService(String),
    #[error("too many services; at most {MAX_SERVICES} are allowed")]
    TooManyServices,
    #[error("invalid storage entry name: {0}")]
    InvalidStorageEntry(String),
    #[error("invalid container path `{0}`")]
    InvalidContainerPath(String),
    #[error("duplicate volume `{0}`")]
    DuplicateVolume(String),
    #[error("too many volumes; at most {MAX_VOLUMES} are allowed")]
    TooManyVolumes,
    #[error("invalid host mount: {0}")]
    InvalidHostMount(String),
    #[error("invalid job plan: {0}")]
    Job(String),
}

/// One human-readable row of the review page: what the command does and the
/// exact argv it will run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanPreviewLine {
    pub label: &'static str,
    pub argv: String,
}

/// Renders the plan for the review page. Re-runs `to_commands()` so the
/// preview can never show something the executor would reject. Only wizard
/// specs are previewable, and no spec that carries secrets is ever rendered.
pub fn plan_preview(plan: &[JobSpec]) -> Result<Vec<PlanPreviewLine>, WizardError> {
    let mut lines = Vec::new();
    for spec in plan {
        let label = match spec {
            JobSpec::AppCreate { .. } => "Create app",
            JobSpec::BuilderSet { .. } => "Set builder",
            JobSpec::BuildpacksAdd { .. } => "Add buildpack",
            JobSpec::ServiceCreate { .. } => "Create service",
            JobSpec::ServiceLink { .. } => "Link service",
            JobSpec::StorageCreate { .. } => "Create volume",
            JobSpec::StorageMountEntry { .. } => "Mount volume",
            JobSpec::VolumeMount { .. } => "Mount host path",
            _ => {
                return Err(WizardError::Job(
                    "spec is not part of the wizard plan".to_owned(),
                ));
            }
        };
        let commands = spec
            .to_commands()
            .map_err(|err| WizardError::Job(err.to_string()))?;
        for command in commands {
            lines.push(PlanPreviewLine {
                label,
                argv: command.argv().join(" "),
            });
        }
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::command::DokkuCommand;

    fn draft(name: &str) -> AppWizardDraft {
        AppWizardDraft {
            name: Some(name.to_owned()),
            ..Default::default()
        }
    }

    #[test]
    fn name_only_plan_is_app_create() {
        assert_eq!(
            draft("alpha").to_plan().expect("plan"),
            vec![JobSpec::AppCreate {
                app: "alpha".into(),
            }]
        );
    }

    #[test]
    fn missing_name_is_rejected() {
        for name in [None, Some(""), Some("   ")] {
            let d = AppWizardDraft {
                name: name.map(str::to_owned),
                ..Default::default()
            };
            assert_eq!(d.to_plan(), Err(WizardError::MissingName), "{name:?}");
        }
        assert!(matches!(
            draft("Bad_App").to_plan(),
            Err(WizardError::InvalidName(_))
        ));
    }

    #[test]
    fn auto_builder_with_buildpack_inserts_herokuish() {
        let d = AppWizardDraft {
            name: Some("alpha".into()),
            buildpacks: vec!["https://github.com/heroku/heroku-buildpack-nodejs".into()],
            ..Default::default()
        };
        assert_eq!(
            d.to_plan().expect("plan"),
            vec![
                JobSpec::AppCreate {
                    app: "alpha".into(),
                },
                JobSpec::BuilderSet {
                    app: "alpha".into(),
                    property: "selected".into(),
                    value: Some("herokuish".into()),
                },
                JobSpec::BuildpacksAdd {
                    app: "alpha".into(),
                    buildpack: "https://github.com/heroku/heroku-buildpack-nodejs".into(),
                    index: None,
                },
            ]
        );
    }

    #[test]
    fn explicit_herokuish_with_buildpack_has_one_builder_set() {
        let d = AppWizardDraft {
            name: Some("alpha".into()),
            builder: Some("herokuish".into()),
            buildpacks: vec!["https://example.com/bp.git".into(), "  ".into()],
            ..Default::default()
        };
        let plan = d.to_plan().expect("plan");
        let builder_sets = plan
            .iter()
            .filter(|spec| matches!(spec, JobSpec::BuilderSet { .. }))
            .count();
        assert_eq!(builder_sets, 1);
        assert_eq!(
            plan[1],
            JobSpec::BuilderSet {
                app: "alpha".into(),
                property: "selected".into(),
                value: Some("herokuish".into()),
            }
        );
        assert_eq!(plan.len(), 3, "blank buildpack entries are ignored");
    }

    #[test]
    fn explicit_dockerfile_alone_sets_the_builder() {
        let d = AppWizardDraft {
            name: Some("alpha".into()),
            builder: Some("dockerfile".into()),
            ..Default::default()
        };
        assert_eq!(
            d.to_plan().expect("plan"),
            vec![
                JobSpec::AppCreate {
                    app: "alpha".into(),
                },
                JobSpec::BuilderSet {
                    app: "alpha".into(),
                    property: "selected".into(),
                    value: Some("dockerfile".into()),
                },
            ]
        );
    }

    #[test]
    fn non_herokuish_builder_with_buildpack_is_rejected() {
        let d = AppWizardDraft {
            name: Some("alpha".into()),
            builder: Some("dockerfile".into()),
            buildpacks: vec!["https://example.com/bp.git".into()],
            ..Default::default()
        };
        assert_eq!(
            d.to_plan(),
            Err(WizardError::BuildpackBuilderConflict("dockerfile".into()))
        );
        let bad = AppWizardDraft {
            name: Some("alpha".into()),
            builder: Some("podman".into()),
            ..Default::default()
        };
        assert_eq!(
            bad.to_plan(),
            Err(WizardError::InvalidBuilder("podman".into()))
        );
        let bad = AppWizardDraft {
            name: Some("alpha".into()),
            buildpacks: vec!["it's".into()],
            ..Default::default()
        };
        assert!(matches!(
            bad.to_plan(),
            Err(WizardError::InvalidBuildpack(_))
        ));
    }

    #[test]
    fn too_many_buildpacks_is_rejected() {
        let d = AppWizardDraft {
            name: Some("alpha".into()),
            buildpacks: (0..=MAX_BUILDPACKS)
                .map(|i| format!("https://example.com/{i}"))
                .collect(),
            ..Default::default()
        };
        assert_eq!(d.to_plan(), Err(WizardError::TooManyBuildpacks));
    }

    #[test]
    fn create_service_pushes_create_then_link() {
        let d = AppWizardDraft {
            name: Some("alpha".into()),
            services: vec![WizardService {
                plugin: "redis".into(),
                service: "candid".into(),
                create: true,
            }],
            ..Default::default()
        };
        assert_eq!(
            d.to_plan().expect("plan"),
            vec![
                JobSpec::AppCreate {
                    app: "alpha".into(),
                },
                JobSpec::ServiceCreate {
                    plugin: "redis".into(),
                    service: "candid".into(),
                    options: ServiceCreateOptions::default(),
                },
                JobSpec::ServiceLink {
                    plugin: "redis".into(),
                    service: "candid".into(),
                    app: "alpha".into(),
                },
            ]
        );
    }

    #[test]
    fn attach_service_pushes_link_only() {
        let d = AppWizardDraft {
            name: Some("alpha".into()),
            services: vec![WizardService {
                plugin: "postgres".into(),
                service: "primary".into(),
                create: false,
            }],
            ..Default::default()
        };
        assert_eq!(
            d.to_plan().expect("plan"),
            vec![
                JobSpec::AppCreate {
                    app: "alpha".into(),
                },
                JobSpec::ServiceLink {
                    plugin: "postgres".into(),
                    service: "primary".into(),
                    app: "alpha".into(),
                },
            ]
        );
    }

    #[test]
    fn duplicate_and_invalid_services_are_rejected() {
        let dup = AppWizardDraft {
            name: Some("alpha".into()),
            services: vec![
                WizardService {
                    plugin: "redis".into(),
                    service: "candid".into(),
                    create: true,
                },
                WizardService {
                    plugin: "redis".into(),
                    service: "candid".into(),
                    create: false,
                },
            ],
            ..Default::default()
        };
        assert!(matches!(
            dup.to_plan(),
            Err(WizardError::DuplicateService(_))
        ));
        let bad_plugin = AppWizardDraft {
            name: Some("alpha".into()),
            services: vec![WizardService {
                plugin: "maria".into(),
                service: "candid".into(),
                create: true,
            }],
            ..Default::default()
        };
        assert!(matches!(
            bad_plugin.to_plan(),
            Err(WizardError::InvalidServicePlugin(_))
        ));
        let bad_name = AppWizardDraft {
            name: Some("alpha".into()),
            services: vec![WizardService {
                plugin: "redis".into(),
                service: "bad name".into(),
                create: true,
            }],
            ..Default::default()
        };
        assert!(matches!(
            bad_name.to_plan(),
            Err(WizardError::InvalidServiceName(_))
        ));
        let too_many = AppWizardDraft {
            name: Some("alpha".into()),
            services: (0..=MAX_SERVICES)
                .map(|i| WizardService {
                    plugin: "redis".into(),
                    service: format!("cache-{i}"),
                    create: false,
                })
                .collect(),
            ..Default::default()
        };
        assert_eq!(too_many.to_plan(), Err(WizardError::TooManyServices));
    }

    #[test]
    fn create_volume_pushes_create_then_named_mount() {
        let d = AppWizardDraft {
            name: Some("alpha".into()),
            volumes: vec![WizardVolume {
                mode: VolumeMode::Create,
                source: "uploads".into(),
                container: "/app/uploads".into(),
            }],
            ..Default::default()
        };
        assert_eq!(
            d.to_plan().expect("plan"),
            vec![
                JobSpec::AppCreate {
                    app: "alpha".into(),
                },
                JobSpec::StorageCreate {
                    entry: "uploads".into(),
                },
                JobSpec::StorageMountEntry {
                    app: "alpha".into(),
                    entry: "uploads".into(),
                    container: "/app/uploads".into(),
                },
            ]
        );
    }

    #[test]
    fn attach_entry_pushes_named_mount_only() {
        let d = AppWizardDraft {
            name: Some("alpha".into()),
            volumes: vec![WizardVolume {
                mode: VolumeMode::AttachEntry,
                source: "shared".into(),
                container: "/data".into(),
            }],
            ..Default::default()
        };
        assert_eq!(
            d.to_plan().expect("plan"),
            vec![
                JobSpec::AppCreate {
                    app: "alpha".into(),
                },
                JobSpec::StorageMountEntry {
                    app: "alpha".into(),
                    entry: "shared".into(),
                    container: "/data".into(),
                },
            ]
        );
    }

    #[test]
    fn attach_entry_allows_existing_legacy_names_but_create_does_not() {
        let attach = AppWizardDraft {
            name: Some("alpha".into()),
            volumes: vec![WizardVolume {
                mode: VolumeMode::AttachEntry,
                source: "legacy-151e4f1a23".into(),
                container: "/data".into(),
            }],
            ..Default::default()
        };
        assert_eq!(
            attach.to_plan().expect("plan")[1],
            JobSpec::StorageMountEntry {
                app: "alpha".into(),
                entry: "legacy-151e4f1a23".into(),
                container: "/data".into(),
            }
        );
        let create = AppWizardDraft {
            name: Some("alpha".into()),
            volumes: vec![WizardVolume {
                mode: VolumeMode::Create,
                source: "legacy-151e4f1a23".into(),
                container: "/data".into(),
            }],
            ..Default::default()
        };
        assert!(matches!(
            create.to_plan(),
            Err(WizardError::InvalidStorageEntry(_))
        ));
    }

    #[test]
    fn attach_path_uses_the_colon_form_spec() {
        let d = AppWizardDraft {
            name: Some("alpha".into()),
            volumes: vec![WizardVolume {
                mode: VolumeMode::AttachPath,
                source: "/var/lib/dokku/data/storage/shared".into(),
                container: "/mnt/shared".into(),
            }],
            ..Default::default()
        };
        assert_eq!(
            d.to_plan().expect("plan"),
            vec![
                JobSpec::AppCreate {
                    app: "alpha".into(),
                },
                JobSpec::VolumeMount {
                    app: "alpha".into(),
                    spec: "/var/lib/dokku/data/storage/shared:/mnt/shared".into(),
                },
            ]
        );
    }

    #[test]
    fn invalid_and_duplicate_volumes_are_rejected() {
        let dup = AppWizardDraft {
            name: Some("alpha".into()),
            volumes: vec![
                WizardVolume {
                    mode: VolumeMode::AttachEntry,
                    source: "shared".into(),
                    container: "/data".into(),
                },
                WizardVolume {
                    mode: VolumeMode::AttachEntry,
                    source: "shared".into(),
                    container: "/data".into(),
                },
            ],
            ..Default::default()
        };
        assert!(matches!(
            dup.to_plan(),
            Err(WizardError::DuplicateVolume(_))
        ));
        let bad_entry = AppWizardDraft {
            name: Some("alpha".into()),
            volumes: vec![WizardVolume {
                mode: VolumeMode::AttachEntry,
                source: "bad entry".into(),
                container: "/data".into(),
            }],
            ..Default::default()
        };
        assert!(matches!(
            bad_entry.to_plan(),
            Err(WizardError::InvalidStorageEntry(_))
        ));
        let relative = AppWizardDraft {
            name: Some("alpha".into()),
            volumes: vec![WizardVolume {
                mode: VolumeMode::Create,
                source: "uploads".into(),
                container: "relative/path".into(),
            }],
            ..Default::default()
        };
        assert!(matches!(
            relative.to_plan(),
            Err(WizardError::InvalidContainerPath(_))
        ));
        let bad_path = AppWizardDraft {
            name: Some("alpha".into()),
            volumes: vec![WizardVolume {
                mode: VolumeMode::AttachPath,
                source: "/host".into(),
                container: "relative".into(),
            }],
            ..Default::default()
        };
        assert!(matches!(
            bad_path.to_plan(),
            Err(WizardError::InvalidHostMount(_))
        ));
        let too_many = AppWizardDraft {
            name: Some("alpha".into()),
            volumes: (0..=MAX_VOLUMES)
                .map(|i| WizardVolume {
                    mode: VolumeMode::AttachEntry,
                    source: format!("entry-{i}"),
                    container: "/data".into(),
                })
                .collect(),
            ..Default::default()
        };
        assert_eq!(too_many.to_plan(), Err(WizardError::TooManyVolumes));
    }

    #[test]
    fn trimming_is_consistent_even_for_untrimmed_drafts() {
        let d = AppWizardDraft {
            name: Some("  alpha  ".into()),
            builder: Some("  ".into()),
            volumes: vec![WizardVolume {
                mode: VolumeMode::AttachEntry,
                source: "  shared  ".into(),
                container: "  /data  ".into(),
            }],
            ..Default::default()
        };
        assert_eq!(
            d.to_plan().expect("plan"),
            vec![
                JobSpec::AppCreate {
                    app: "alpha".into(),
                },
                JobSpec::StorageMountEntry {
                    app: "alpha".into(),
                    entry: "shared".into(),
                    container: "/data".into(),
                },
            ]
        );
    }

    #[test]
    fn preview_renders_the_exact_named_mount_line() {
        let lines = plan_preview(&[
            JobSpec::StorageCreate {
                entry: "uploads".into(),
            },
            JobSpec::StorageMountEntry {
                app: "alpha".into(),
                entry: "uploads".into(),
                container: "/app/uploads".into(),
            },
        ])
        .expect("preview");
        assert_eq!(
            lines,
            vec![
                PlanPreviewLine {
                    label: "Create volume",
                    argv: "storage:create uploads".into(),
                },
                PlanPreviewLine {
                    label: "Mount volume",
                    argv: "storage:mount alpha uploads --container-dir /app/uploads".into(),
                },
            ]
        );
        assert!(
            plan_preview(&[JobSpec::AppDestroy {
                app: "alpha".into(),
            }])
            .is_err(),
            "non-wizard specs are never previewed"
        );
        assert!(
            plan_preview(&[JobSpec::AppCreate {
                app: "Bad App".into(),
            }])
            .is_err(),
            "tampered specs fail before the preview is shown"
        );
    }

    #[test]
    fn preview_labels_every_wizard_step() {
        let d = AppWizardDraft {
            name: Some("alpha".into()),
            buildpacks: vec!["https://example.com/bp.git".into()],
            services: vec![WizardService {
                plugin: "redis".into(),
                service: "candid".into(),
                create: true,
            }],
            volumes: vec![
                WizardVolume {
                    mode: VolumeMode::Create,
                    source: "uploads".into(),
                    container: "/app/uploads".into(),
                },
                WizardVolume {
                    mode: VolumeMode::AttachPath,
                    source: "/host".into(),
                    container: "/data".into(),
                },
            ],
            ..Default::default()
        };
        let plan = d.to_plan().expect("plan");
        let lines = plan_preview(&plan).expect("preview");
        let labels: Vec<&str> = lines.iter().map(|line| line.label).collect();
        assert_eq!(
            labels,
            vec![
                "Create app",
                "Set builder",
                "Add buildpack",
                "Create service",
                "Link service",
                "Create volume",
                "Mount volume",
                "Mount host path",
            ]
        );
        assert_eq!(lines[0].argv, "apps:create alpha");
        let DokkuCommand::AppsCreate { .. } = plan[0].to_commands().expect("cmd")[0] else {
            panic!("expected apps:create");
        };
    }
}
