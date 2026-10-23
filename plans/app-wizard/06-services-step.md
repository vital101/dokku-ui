# 06 - Services step

Read `00-overview.md`, `03-wizard-draft-and-plan.md`, and `04-session-routes-and-name-step.md` first. This adds step 3 with add/remove-while-accumulating semantics.

## Files

- Modify `src/dokku/service_pages.rs`: add `pub async fn service_names(...)`.
- Modify `src/dokku/mod.rs`: export `service_names`.
- Modify `src/web/wizard.rs` (handlers + template struct).
- Modify `src/web/mod.rs` (routes).
- Create `templates/apps/wizard/services.html`.
- Extend `tests/wizard.rs`.

## Service-name helper

In `service_pages.rs`:

```rust
pub async fn service_names(
    client: &dyn DokkuClient,
    plugin: ServicePlugin,
) -> Result<Vec<String>, DokkuError> {
    let output = client.exec(&DokkuCommand::ServiceList { plugin }).await?;
    Ok(parse_service_list(&output.stdout))
}
```

Add a unit test using `tests/fixtures/redis_list.txt` and a failure test. This avoids the per-service `<plugin>:info` calls used by `plugin_services`.

## Routes

Add before `/apps/{name}`:

```rust
.route("/apps/new/services", web::get().to(wizard::services_step))
.route("/apps/new/services/attach", web::post().to(wizard::attach_service))
.route("/apps/new/services/create", web::post().to(wizard::create_service))
.route("/apps/new/services/remove", web::post().to(wizard::remove_service))
```

## GET `/apps/new/services`

Require a draft name, else redirect to `/apps/new`.

Load available services:

1. `state.capabilities.ensure_loaded().await`.
2. On error: `services_error = Some(err.to_string())`, no plugins/services; still render and allow “Next”.
3. On success: iterate `ServicePlugin::all()`; skip only when `supports_plugin(plugin.as_str()) == Support::PluginMissing`. Keep `Unknown` (treat as available).
4. For each plugin call `service_names(&*state.dokku, plugin).await`:
   - `Ok(names)` -> append `AvailableService { plugin, service }`.
   - `Err(err)` -> push `(plugin.as_str(), err.to_string())` into `list_errors`; continue.

View models:

```rust
struct AvailableService {
    plugin: ServicePlugin,
    service: String,
}

struct SelectedServiceRow {
    plugin_label: String,
    service: String,
    mode_label: &'static str, // "create + link" | "attach"
}
```

Template select option value is `"<plugin>:<service>"`; label is `"<display_name> / <service>"`. Build `SelectedServiceRow` from the draft for the current-selections table.

## POST attach

```rust
#[derive(Deserialize)]
pub struct AttachServiceForm {
    service_key: String, // "<plugin>:<service>"
}
```

- Split once on `:`; reject malformed.
- Validate `ServicePlugin` and `ServiceName`.
- Re-fetch `service_names` for that plugin. If it fails, re-render with “Could not read … services; retry.”
- Require the service to exist in that list.
- Reject duplicate `(plugin, service)` in the draft and `MAX_SERVICES`.
- Push `WizardService { create: false }`, save, redirect `/apps/new/services`.

## POST create

```rust
#[derive(Deserialize)]
pub struct CreateServiceForm {
    plugin: String,
    service: String,
}
```

- Validate plugin and service name.
- Verify the plugin is installed via `state.capabilities.current()` / `ensure_loaded`; reject `PluginMissing`.
- Re-fetch that plugin's service list; require the list call to succeed.
- Reject if the name already exists in the list (“A … service named … already exists; attach it instead.”).
- Reject duplicates in the draft and `MAX_SERVICES`.
- Push `WizardService { create: true }`, save, redirect.

## POST remove

```rust
#[derive(Deserialize)]
pub struct RemoveServiceForm {
    index: usize,
}
```

Remove when `index < draft.services.len()`, save, redirect. An out-of-range index is a no-op redirect (no error page needed).

## Template

`services.html`:

- Extends `base.html`, heading “New app - Services”, progress `active_step = "services"`.
- Current selections table: plugin display name, service, “create + link” or “attach”, Remove form per row (CSRF).
- Attach card: select of `available_services`; disabled/empty-state when none.
- Create card: plugin select of installed plugins + service name input.
- `services_error` and per-plugin `list_errors` notices. These must not block “Next”.
- Back link to `/apps/new/build`; “Next: volumes” link to `/apps/new/volumes`.
- No `hx-*` attributes.

## Tests

- GET renders existing services from stubbed `<plugin>:list` calls for installed plugins.
- Attach existing appends and re-renders it.
- Create new appends with `create = true`.
- Creating a service whose name is already listed is rejected.
- Duplicate attach is rejected.
- Remove drops the row.
- A failed capabilities probe shows a notice but “Next” still renders.
- No `ServiceCreate`/`ServiceLink` command is called during any step POST.
- CSRF is enforced on attach/create/remove.

Focused command:

```text
docker compose run --rm web cargo test --test wizard service
```

## Definition of done

- Only installed datastore plugins are offered.
- The step is optional: users can continue with zero services.
- All additions are validated before entering the session draft.
