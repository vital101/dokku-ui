# 10 - Remove the old flow and document invariants

Read `00-overview.md` and all previous steps. This is cleanup only; the wizard must already work end to end.

## Files

- Modify `src/web/apps.rs`.
- Modify `src/web/mod.rs`.
- Delete `templates/apps/new.html` if step 04 did not already delete it.
- Modify `AGENTS.md`.
- Update `tests/app_routes.rs` and `tests/rbac.rs` where they use the old create flow.

## Remove the old create path

In `src/web/apps.rs`:

- Remove `pub struct CreateForm`.
- Remove `pub async fn create(...)`.
- Remove now-unused imports only if the compiler reports them. Do not remove `DokkuCommand`, `CsrfForm`, or `run_synchronously` if other handlers still use them.
- Keep `new_form` removed from step 04.

In `src/web/mod.rs`:

- Remove the `POST /apps` route (`.route("/apps", web::post().to(apps::create))`).
- Confirm the wizard routes are all registered before `/apps/{name}` and its tab routes.
- Confirm no duplicate `GET /apps/new` remains.

## Update existing tests

`tests/app_routes.rs`:

- Remove or rewrite the three `create_app_*` tests that POST `/apps`.
- Keep the unauthenticated-route list; add every wizard route.
- Replace the old create-success assertion with a wizard navigation test:
  - setup -> GET `/apps/new` -> POST name -> expect `/apps/new/build`.
- Keep invalid-name coverage in `tests/wizard.rs` if duplicated here.

`tests/rbac.rs`:

- Replace the operator `POST /apps` create assertion with a wizard step POST (for example POST `/apps/new` with a valid name) and keep the expected redirect to `/apps/new/build`.
- Confirm viewer GET `/apps/new` still redirects and viewer POSTs remain 403 via middleware.

Do not weaken the RBAC tests: the middleware remains the authoritative gate.

## AGENTS.md

Add a short “App creation wizard” subsection under architecture essentials. Keep it concise and factual:

- `/apps/new` is a five-step session-draft wizard; draft key `app_wizard_draft`.
- Wizard routes must stay registered before `/apps/{name}`.
- Review renders the exact `JobSpec::to_commands()` argv.
- Execution is one persisted job (`app.create`, `TargetKind::App`) with order: `AppCreate`, builder/buildpacks, service create/link, volume create/mount.
- `JobSpec::AppCreate` now exists and is what the wizard uses.
- Buildpacks require `herokuish`; Auto + URLs inserts `builder:set ... selected herokuish`.
- Volume creation uses 0.38.4 `storage:create` + named `storage:mount ... --container-dir`; arbitrary host paths still use colon-form `MountSpec`.
- No rollback: a later failure leaves a partially configured app and the run log shows where it stopped.
- Service listing uses `<plugin>:list` only and is limited to installed plugins.

Do not add a new migration; none is needed.

## Verification for this step

```text
make fmt
make lint
docker compose run --rm web cargo test --test app_routes
docker compose run --rm web cargo test --test rbac
docker compose run --rm web cargo test --test wizard
```

## Definition of done

- No code path creates an app outside the wizard/persisted job.
- No dead old-handler code remains.
- AGENTS.md describes the new invariants.
- All existing non-wizard tests pass.
