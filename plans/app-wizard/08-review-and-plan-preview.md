# 08 - Review and plan preview

Read `00-overview.md`, `03-wizard-draft-and-plan.md`, and `04-session-routes-and-name-step.md` first. This adds the review page; execution itself is step 9.

## Files

- Modify `src/web/wizard.rs` (handler + template struct).
- Modify `src/web/mod.rs` (route).
- Create `templates/apps/wizard/review.html`.
- Extend `tests/wizard.rs`.

## Route

```rust
.route("/apps/new/review", web::get().to(wizard::review))
```

Register before `/apps/{name}`.

## Handler

`GET /apps/new/review`:

1. Require a draft name, else redirect to `/apps/new`.
2. `let plan = draft.to_plan()`. On error, render the review page with `error` and edit links; do not build a preview.
3. `let preview = plan_preview(&plan)`. On error, same.
4. Render:

```rust
struct ServiceReviewRow {
    plugin_label: String,
    service: String,
    mode_label: &'static str,
}

struct VolumeReviewRow {
    mode_label: &'static str,
    source: String,
    container: String,
}

struct ReviewPage<'a> {
    email: &'a str,
    csrf_token: &'a str,
    flash: Option<&'a FlashMessage>,
    active_step: &'static str, // "review"
    error: Option<String>,
    name: String,
    builder_label: String,
    buildpacks: Vec<String>,
    services: Vec<ServiceReviewRow>,
    volumes: Vec<VolumeReviewRow>,
    preview: Vec<PlanPreviewLine>,
}
```

Build `ServiceReviewRow` from each draft service: parse `ServicePlugin` for the display label (fallback to the raw plugin string), and set `mode_label` to “create + link” or “attach”. Build `VolumeReviewRow` with the mode label “create named entry”, “attach entry”, or “attach host path”. This keeps template logic trivial and avoids parsing domain enums in askama.

Builder label:

- `Some(name)` -> the name
- `None` + no buildpacks -> `Auto (detect)`
- `None` + buildpacks -> `Auto -> herokuish`

## Template

`review.html`:

- Extends `base.html`, heading “New app - Review”, progress `active_step = "review"`.
- Error block when `error` is set.
- Summary cards with “Edit” links:
  - Name -> `/apps/new`
  - Build -> `/apps/new/build`
  - Services -> `/apps/new/services`
  - Volumes -> `/apps/new/volumes`
- Services table/list shows plugin display name, service, mode.
- Volumes list shows mode, source, container.
- “Plan” section renders `preview`: numbered label plus `<code>` argv line per command.
- Execute form:
  ```html
  <form method="post" action="/apps/new/execute"
        hx-post="/apps/new/execute" hx-target="#modal-content" hx-swap="innerHTML"
        data-action-form>
  ```
  CSRF hidden input, submit button with class `app-action` labelled “Create app”.
- Reset form: `method="post" action="/apps/new/reset"` with CSRF, button “Start over”.
- `{% include "partials/modal.html" %}` at the end.
- No inline JS/CSS.

The preview must render the real `argv()` values, not a hand-written approximation. This is the user’s verification step.

## Tests

- After a complete draft, review is 200 and shows the app name, builder, services, and volumes.
- Preview contains, in order:
  - `apps:create alpha`
  - `builder:set alpha selected herokuish`
  - `buildpacks:add alpha https://example.com/bp`
  - `postgres:create cache`
  - `postgres:link cache alpha`
  - `storage:create uploads`
  - `storage:mount alpha uploads --container-dir /app/uploads`
- Attach-path preview contains `storage:mount alpha /srv/files:/app/files`.
- An invalid draft (e.g. explicit non-herokuish builder with buildpacks injected into the session) renders an error and no execute button.
- No command executes on GET review.
- Review never renders secrets (there are none today; keep the assertion broad by checking the known form values only).

Focused command:

```text
docker compose run --rm web cargo test --test wizard review
```

## Definition of done

- The page shows the exact plan that execution will run.
- Errors are recoverable via the Edit links.
- The execute form is wired but execution is added in step 9.
