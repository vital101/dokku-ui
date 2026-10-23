# 07 - Volumes step

Read `00-overview.md`, `01-domain-storage-entry.md`, `03-wizard-draft-and-plan.md`, and `04-session-routes-and-name-step.md` first. This adds step 4.

## Files

- Modify `src/web/wizard.rs` (handlers + template struct).
- Modify `src/web/mod.rs` (routes).
- Create `templates/apps/wizard/volumes.html`.
- Extend `tests/wizard.rs`.

## Routes

Add before `/apps/{name}`:

```rust
.route("/apps/new/volumes", web::get().to(wizard::volumes_step))
.route("/apps/new/volumes/create", web::post().to(wizard::create_volume))
.route("/apps/new/volumes/attach-entry", web::post().to(wizard::attach_volume_entry))
.route("/apps/new/volumes/attach-path", web::post().to(wizard::attach_volume_path))
.route("/apps/new/volumes/remove", web::post().to(wizard::remove_volume))
```

## GET `/apps/new/volumes`

Require a draft name, else redirect to `/apps/new`.

Load entries:

```rust
let entries = match state.dokku.exec(&DokkuCommand::StorageListEntries).await {
    Ok(output) => parse_storage_entries(&output.stdout),
    Err(err) => {
        entries_error = Some(err.to_string());
        Vec::new()
    }
};
```

Use `StorageEntry` (`name`, `host_path`). Build a small view model for the current selections so askama does not need to match the enum:

```rust
struct SelectedVolumeRow {
    mode_label: &'static str, // "create named entry" | "attach entry" | "attach host path"
    source: String,
    container: String,
}
```

Render the current draft volumes as these rows, the existing entries, and the three add forms. A failed list only removes the attach-entry select and shows a notice; create and attach-path stay available.

## POST create

```rust
#[derive(Deserialize)]
pub struct CreateVolumeForm {
    entry: String,
    container: String,
}
```

- Trim both.
- Validate with `StorageEntryName::try_from_create` (rejects `legacy-*`) and `is_valid_volume_container_path`.
- If the entry list loaded and already contains the name, reject: “Entry … already exists; attach it instead.”
- Reject duplicate `(VolumeMode::Create, entry, container)` and `MAX_VOLUMES`.
- Push `WizardVolume { mode: VolumeMode::Create, source: entry, container }`, save, redirect `/apps/new/volumes`.

## POST attach-entry

```rust
#[derive(Deserialize)]
pub struct AttachVolumeEntryForm {
    entry: String,
    container: String,
}
```

- Re-fetch the storage entries list; if it fails, reject with a retry message.
- Require the entry name to exist in that list.
- Validate with `StorageEntryName::try_from` (allows existing `legacy-*` entries) and container path.
- Reject duplicate `(AttachEntry, entry, container)` and `MAX_VOLUMES`.
- Push and redirect.

## POST attach-path

```rust
#[derive(Deserialize)]
pub struct AttachVolumePathForm {
    host: String,
    container: String,
}
```

- Trim both.
- Build `MountSpec::try_from(format!("{host}:{container}"))`; map the error into the page error.
- Reject duplicate `(AttachPath, host, container)` and `MAX_VOLUMES`.
- Push `WizardVolume { mode: VolumeMode::AttachPath, source: host, container }`, save, redirect.

## POST remove

Reuse the same `RemoveVolumeForm { index: usize }` pattern as services. Remove in range, save, redirect.

## Template

`volumes.html`:

- Extends `base.html`, heading “New app - Volumes”, progress `active_step = "volumes"`.
- Current selections table: mode label (`create named entry`, `attach entry`, `attach host path`), source, container, Remove form with CSRF.
- “Create named entry” card: entry name + container path; help text says the host directory is created under dokku storage.
- “Attach registered entry” card: select from loaded entries (label includes host path) + container path; hidden/empty when list failed.
- “Attach host path” card: host path or docker volume + container path.
- `entries_error` notice.
- Back link to `/apps/new/services`; “Review plan” link to `/apps/new/review`.
- No `hx-*` attributes.

## Tests

- GET renders the fixture entries from `tests/fixtures/list_entries.json`.
- Create appends a `Create` volume.
- Creating an entry that already exists is rejected.
- Attach-entry appends an `AttachEntry` volume.
- Attaching a fixture `legacy-*` entry succeeds.
- Attach-path appends an `AttachPath` volume.
- Duplicate volume is rejected.
- Relative container path, bad entry name, and whitespace host path are rejected.
- Remove drops the row.
- A failed `storage:list-entries` shows a notice, still allows create and attach-path, and still renders Review.
- No `StorageCreate`/`StorageMount`/`StorageMountEntry` call runs during any step POST.
- CSRF is enforced on all volume POSTs.

Focused command:

```text
docker compose run --rm web cargo test --test wizard volume
```

## Definition of done

- Create always means a named storage entry with the default dokku host path.
- Attach-entry uses the named-entry mount form; attach-path uses the existing colon form.
- The step is optional: users can continue with zero volumes.
