# 03 - Wizard draft and plan builder

Read `00-overview.md`, `01-domain-storage-entry.md`, and `02-commands-and-job-specs.md` first. This is the pure core of the wizard: no IO, no web types.

## Files

- Create `src/domain/app_wizard.rs`.
- Update `src/domain/mod.rs` to add `pub mod app_wizard;` and re-export `AppWizardDraft`, `PlanPreviewLine`, `VolumeMode`, `WizardError`, `WizardService`, `WizardVolume`, `plan_preview`.

## Draft types

```rust
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
```

Constants:

```rust
pub const MAX_BUILDPACKS: usize = 20;
pub const MAX_SERVICES: usize = 10;
pub const MAX_VOLUMES: usize = 10;
```

`WizardError` is a `thiserror` enum with readable variants: missing name, invalid app name, invalid builder, invalid buildpack, buildpack/builder conflict, too many buildpacks, invalid service plugin/name, duplicate service, too many services, invalid storage entry, invalid container path, duplicate volume, too many volumes, invalid host mount, and a wrapped job error string.

## `to_plan`

```rust
impl AppWizardDraft {
    pub fn to_plan(&self) -> Result<Vec<JobSpec>, WizardError>;
}
```

Rules and order:

1. Trim the name, require it, and validate it with `AppName::try_from`.
2. Always push `JobSpec::AppCreate { app }` first.
3. Builder/buildpacks:
   - `Some(builder)`: trim and require it to be in `BUILDERS`.
   - Non-empty buildpacks with `Some(builder)` where builder != `herokuish` -> `WizardError`.
   - If buildpacks are non-empty and builder is `None`, the plan must insert `BuilderSet { property: "selected", value: Some("herokuish") }`.
   - If builder is `Some`, insert one `BuilderSet` with that value.
   - Then push one `BuildpacksAdd { index: None }` per URL, in input order.
   - Validate each URL with `is_valid_buildpack`; trim and ignore blanks; enforce `MAX_BUILDPACKS`.
4. Services, in draft order:
   - Validate `ServicePlugin::try_from(plugin)` and `ServiceName::try_from(service)`.
   - Reject duplicate `(plugin, service)` pairs.
   - If `create`, push `JobSpec::ServiceCreate` with `ServiceCreateOptions::default()`.
   - Always push `JobSpec::ServiceLink`.
   - Enforce `MAX_SERVICES`.
5. Volumes, in draft order:
   - `Create`: validate with `StorageEntryName::try_from_create` (rejects `legacy-*`) and `is_valid_volume_container_path`; push `JobSpec::StorageCreate`, then `JobSpec::StorageMountEntry`.
   - `AttachEntry`: validate with `StorageEntryName::try_from` (allows existing `legacy-*` entries) and container path; push `JobSpec::StorageMountEntry`.
   - `AttachPath`: build `MountSpec::try_from(format!("{host}:{container}"))` after trimming; push `JobSpec::VolumeMount { spec: mount.arg() }`.
   - Reject duplicate `(mode, source, container)` triples.
   - Enforce `MAX_VOLUMES`.

Do not check service existence or storage-entry existence here; those need IO and belong in the web steps. The job revalidation catches tampering later.

## Plan preview

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanPreviewLine {
    pub label: &'static str,
    pub argv: String,
}

pub fn plan_preview(plan: &[JobSpec]) -> Result<Vec<PlanPreviewLine>, WizardError>;
```

For each spec call `to_commands()`, then emit one line per command with:

- label: `Create app`, `Set builder`, `Add buildpack`, `Create service`, `Link service`, `Create volume`, `Mount volume`, `Mount host path`
- argv: `command.argv().join(" ")`

Map `JobSpecError` to `WizardError`.

## Tests

Add unit tests for:

- name only -> `[AppCreate]`
- auto + buildpack -> `[AppCreate, BuilderSet(herokuish), BuildpacksAdd]`
- explicit `herokuish` + buildpack -> one `BuilderSet(herokuish)`
- explicit `dockerfile` alone -> `[AppCreate, BuilderSet(dockerfile)]`
- `dockerfile` + buildpack -> error
- create service -> `[AppCreate, ServiceCreate, ServiceLink]`
- attach service -> `[AppCreate, ServiceLink]`
- duplicate service -> error
- create volume -> `[AppCreate, StorageCreate, StorageMountEntry]`
- attach entry -> `[AppCreate, StorageMountEntry]`
- attach an existing `legacy-*` entry -> allowed
- create a `legacy-*` entry -> error
- attach path -> `[AppCreate, VolumeMount]`
- duplicate volume, invalid entry name, relative container, too many items -> errors
- preview argv contains the exact `storage:mount ... --container-dir ...` line
- missing name -> error

Focused command:

```text
docker compose run --rm web cargo test app_wizard
```

## Definition of done

- Pure module only; no `DokkuClient`, no session, no templates.
- Every plan path is covered by at least one test.
- Trimming is consistent: `to_plan` trims even if a caller saved untrimmed values.
