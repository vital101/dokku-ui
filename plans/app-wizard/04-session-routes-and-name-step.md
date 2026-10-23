# 04 - Session, routes, and name step

Read `00-overview.md` and `03-wizard-draft-and-plan.md` first. This step boots the wizard module and completes step 1. Later files add the remaining steps.

## Files

- Create `src/web/wizard.rs`.
- Modify `src/web/mod.rs`: add `mod wizard;` and register the routes below before `/apps/{name}`.
- Modify `src/web/apps.rs`: remove `NewFormPage` and `new_form` (the GET route is replaced). Keep `CreateForm` and `create` for now; step 10 removes them.
- Delete `templates/apps/new.html`.
- Create `templates/apps/wizard/name.html`.
- Create `templates/apps/wizard/partials/progress.html`.
- Create `tests/wizard.rs` with the name-step tests.

## Routes

Register these as resources before the existing `/apps/{name}` route block:

```rust
.service(
    web::resource("/apps/new")
        .route(web::get().to(wizard::name_step))
        .route(web::post().to(wizard::save_name)),
)
.route("/apps/new/reset", web::post().to(wizard::reset))
```

Keep the old `POST /apps` route until step 10. Do not add `/apps/new/build` yet.

## Session helpers in `web/wizard.rs`

```rust
const DRAFT_KEY: &str = "app_wizard_draft";

fn load_draft(session: &Session) -> Result<AppWizardDraft, AppError>;
fn save_draft(session: &Session, draft: &AppWizardDraft) -> Result<(), AppError>;
fn clear_draft(session: &Session);
```

- `load_draft` returns default on `Ok(None)`.
- On a deserialization error, log `tracing::warn!`, remove the corrupt key, and return default.
- `save_draft` maps `session.insert` errors to `AppError::Internal`.
- `clear_draft` ignores a remove error after logging.

## Shared step helper

Every handler:

1. Calls `current_user(&state, &session).await?`.
2. If `!user.role.can_manage_apps()`, sets an error flash and returns `see_other("/")`.
3. Loads the draft.
4. Ensures `ensure_csrf(&session)` exists.

Keep this inline or add one small helper; do not add a new extractor.

## Name step

Form:

```rust
#[derive(Deserialize)]
pub struct NameForm {
    name: String,
}
```

`GET /apps/new` (`name_step`):

- Render `templates/apps/wizard/name.html`.
- Template fields: `email`, `csrf_token`, `flash`, `active_step: &'static str` (`"name"`), `error: Option<String>`, `name: String` (draft value or empty).

`POST /apps/new` (`save_name`):

- Trim the name.
- Validate with `AppName::try_from`.
- Best-effort duplicate check: `state.snapshot.ensure_loaded().await`; if it succeeds and the app is in `snapshot.apps`, show “An app named … already exists.” Ignore snapshot errors and continue.
- On error, re-render the page with `error: Some(message)` and the submitted name. Do not redirect (keeps input).
- On success, set `draft.name = Some(name)`, `save_draft`, redirect `/apps/new/build`.

`POST /apps/new/reset` (`reset`):

- `clear_draft`, set a success flash “Wizard reset.”, redirect `/apps/new`.

## Templates

`name.html`:

- Extends `base.html`, heading “New app”.
- Includes `apps/wizard/partials/progress.html`.
- Shows the error block when `error` is set.
- Form `method="post" action="/apps/new"`, CSRF hidden input, name input (same constraints as the old form), `Next` button, `Cancel` link to `/`.
- No `hx-*` attributes.

`partials/progress.html`:

- Four numbered steps plus Review: Name, Build, Services, Volumes, Review.
- Highlight `active_step`; completed/later steps are muted.
- Use literal Tailwind `slate-*` utilities already used in the repo.
- No links for future steps; Name may link to `/apps/new`.

## Tests in `tests/wizard.rs`

Use `mod common;` and the existing helpers (`complete_setup`, `extract_csrf`, `form_request`, `get_body`, `location`, `seed_user_with_role`, `test_state`).

- unauthenticated `GET /apps/new` redirects to `/login`
- viewer `GET /apps/new` redirects to `/` with the role flash
- setup -> `GET /apps/new` is 200 and contains `name="name"`
- invalid name (`Bad_App`) re-renders with “Invalid app name” and never calls `AppsCreate`
- valid name redirects to `/apps/new/build`
- reset clears the saved name (GET after reset shows an empty input)

Focused command:

```text
docker compose run --rm web cargo test --test wizard
```

## Definition of done

- Wizard GET replaces the old form; POST `/apps` still works until step 10.
- Routes are registered before `/apps/{name}`.
- Name step persists to the session and round-trips on re-render.
- No build/services/volumes code yet.
