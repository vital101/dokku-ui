# 05 - Build step

Read `00-overview.md`, `03-wizard-draft-and-plan.md`, and `04-session-routes-and-name-step.md` first. This adds step 2.

## Files

- Modify `src/web/wizard.rs` (handlers + template struct).
- Modify `src/web/mod.rs` (route).
- Create `templates/apps/wizard/build.html`.
- Extend `tests/wizard.rs`.

## Route

Add before `/apps/{name}`:

```rust
.service(
    web::resource("/apps/new/build")
        .route(web::get().to(wizard::build_step))
        .route(web::post().to(wizard::save_build)),
)
```

## Form and parsing

```rust
#[derive(Deserialize)]
pub struct BuildForm {
    #[serde(default)]
    builder: String,
    #[serde(default)]
    buildpacks: String,
}
```

On `POST /apps/new/build`:

1. Load the draft; if `draft.name` is `None`, redirect to `/apps/new`.
2. Trim `builder`. Empty means Auto (`None`). Otherwise it must be in `domain::build::BUILDERS`; reject with “Unknown builder.”
3. Split `buildpacks` on lines, trim each, drop blanks.
4. Enforce at most `MAX_BUILDPACKS`; validate each with `is_valid_buildpack`; reject with the offending line in the message.
5. If buildpacks are non-empty and the selected builder is explicit and not `herokuish`, reject: “Buildpacks require the herokuish builder.”
   Do **not** auto-change an explicit builder. For Auto + buildpacks, keep `builder = None`; `to_plan` inserts `herokuish`.
6. Save `draft.builder` and `draft.buildpacks`; redirect `/apps/new/services`.
7. On error, re-render the build page with `error` and the submitted values.

On `GET /apps/new/build`:

- Require a draft name, otherwise redirect to `/apps/new`.
- Render the current values: builder (empty for Auto) and buildpacks joined with `\n`.

## Template

`build.html`:

- Extends `base.html`, heading “New app - Build”.
- Includes the progress partial with `active_step = "build"`.
- Error block when set.
- Form `method="post" action="/apps/new/build"` with CSRF.
- Builder `<select name="builder">`: first option `value=""` labelled “Auto (detect)”, then every `BUILDERS` entry.
- Buildpacks `<textarea name="buildpacks" rows="5">`; help text: one URL/path per line, applied in order, requires herokuish; blank means auto-detect.
- “Back” link to `/apps/new`; submit button “Next: services”.
- No `hx-*` attributes and no inline JS.

## Tests

Extend `tests/wizard.rs`:

- after saving a name, `GET /apps/new/build` is 200 and shows the builder select
- Auto + no URLs saves and redirects to `/apps/new/services`
- `herokuish` + one URL saves and redirects
- unknown builder re-renders with “Unknown builder”
- invalid buildpack (`it's bad`) re-renders with an error
- `dockerfile` + a URL re-renders with the conflict message
- going back to Build preserves the saved builder and URLs
- no `AppsCreate`/`BuilderSet`/`BuildpacksAdd` call happens during the step POST

Focused command:

```text
docker compose run --rm web cargo test --test wizard build
```

## Definition of done

- Build step never executes dokku; it only updates the session draft.
- Auto without URLs stores `builder = None` and no buildpacks.
- Auto with URLs stores `builder = None`; the plan later inserts `herokuish`.
- Explicit non-herokuish builder plus buildpacks is rejected here.
