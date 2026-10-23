# 02 - Commands and job specs

Read `00-overview.md` and `01-domain-storage-entry.md` first. This step makes the wizard plan expressible as persisted jobs. No web code yet.

## Files

- Modify `src/domain/command.rs`.
- Modify `src/domain/job.rs`.

## New `DokkuCommand` variants

Add next to the existing storage variants:

```rust
StorageCreate {
    entry: StorageEntryName,
},
StorageMountEntry {
    app: AppName,
    entry: StorageEntryName,
    container: String,
},
```

Import `StorageEntryName` at the top of `command.rs`.

Exact argv:

```rust
DokkuCommand::StorageCreate { entry } =>
    vec!["storage:create".into(), entry.as_str().into()],

DokkuCommand::StorageMountEntry { app, entry, container } =>
    vec![
        "storage:mount".into(),
        app.as_str().into(),
        entry.as_str().into(),
        "--container-dir".into(),
        container.clone(),
    ],
```

`storage:mount <app> <entry> --container-dir <path>` is the 0.38.4 named-entry form. The entry name comes from `storage:create`; the container path must be absolute.

## Timeout and capability gate

- Add `StorageCreate { .. }` and `StorageMountEntry { .. }` to the `CommandTimeout::Indefinite` arm in `DokkuCommand::timeout()` (next to `StorageMount`/`StorageUnmount`).
- The existing `requirement()` fallthrough already marks storage commands `Requirement::Core`; add a short test asserting `StorageCreate` and `StorageMountEntry` are `Support::Supported` under capabilities. Do not introduce a new gate.

## New `JobSpec` variants

Add:

```rust
AppCreate {
    app: String,
},
StorageCreate {
    entry: String,
},
StorageMountEntry {
    app: String,
    entry: String,
    container: String,
},
```

In `JobSpec::to_commands()`:

- `AppCreate` parses `AppName` and returns `DokkuCommand::AppsCreate`.
- `StorageCreate` parses `StorageEntryName::try_from_create` (rejects the reserved `legacy-` prefix) and returns `DokkuCommand::StorageCreate`.
- `StorageMountEntry` parses `AppName`, parses `StorageEntryName` (shape only, so an existing `legacy-*` entry can be attached), checks `is_valid_volume_container_path(&container)`, and returns `DokkuCommand::StorageMountEntry`.

Use `JobSpecError::Invalid` for validation failures. `is_destructive()` stays unchanged (only app/service destroy).

## Tests

In `command.rs`:

- `storage_create_argv` -> `["storage:create", "uploads"]`
- `storage_mount_entry_argv` -> `["storage:mount", "alpha", "uploads", "--container-dir", "/app/uploads"]`
- storage commands are `Requirement::Core`
- storage commands are `CommandTimeout::Indefinite`

In `job.rs`:

- `app_create_rehydrates`
- `storage_create_rehydrates`
- `storage_mount_entry_rehydrates`
- `storage_mount_entry_accepts_legacy_entry_names`
- `storage_mount_entry_rejects_tampered_values` (bad app, bad entry, relative container, colon in container)
- `storage_create_rejects_reserved_legacy_prefix`
- `app_create_is_not_destructive`

Focused command:

```text
docker compose run --rm web cargo test 'command::tests'
docker compose run --rm web cargo test 'job::tests'
```

## Definition of done

- `JobSpec` remains fully exhaustive in `to_commands()`.
- Tampered database payloads fail before any SSH command is built.
- No web/domain wizard code yet.
