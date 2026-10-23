# 09 - Execute and completion

Read `00-overview.md`, `03-wizard-draft-and-plan.md`, and `08-review-and-plan-preview.md` first. This turns the reviewed plan into one persisted run.

## Files

- Modify `src/web/fragments.rs`: add a small `run_fragment` helper and make `start_action_run` use it.
- Modify `src/web/wizard.rs` (execute handler).
- Modify `src/web/mod.rs` (route).
- Extend `tests/wizard.rs`.

## Fragment helper

In `fragments.rs`:

```rust
pub(super) fn run_fragment(
    run_id: &str,
    title: String,
    refresh_url: Option<String>,
) -> Result<HttpResponse, AppError> {
    render(&RunPartial {
        run_url: format!("/actions/runs/{run_id}/events"),
        title,
        refresh_url,
    })
}
```

Replace the inline `render(&RunPartial { ... })` in `start_action_run` with this helper. No behavior change.

The wizard needs this because it must enqueue, clear the draft only on success, and then render the same fragment.

## Route

```rust
.route("/apps/new/execute", web::post().to(wizard::execute))
```

Register before `/apps/{name}`.

## Handler

```rust
pub async fn execute(
    state: web::Data<AppState>,
    session: Session,
    req: HttpRequest,
    _form: CsrfForm<super::apps::ActionForm>,
) -> Result<HttpResponse, AppError>
```

1. Require a manager and a draft name; otherwise redirect to `/` / `/apps/new`.
2. `let plan = draft.to_plan()`. On error:
   - htmx -> `modal_error(message)`
   - no JS -> flash error, redirect `/apps/new/review`
3. Compute counts from the draft:

```rust
let n_services = draft.services.len();
let n_volumes = draft.volumes.len();
let Some(app) = draft.name.clone() else {
    set_flash(&session, FlashLevel::Error, "Start by naming the app.");
    return Ok(see_other("/apps/new"));
};
```

4. Build the request:

```rust
let request = RunRequest {
    subject: app.clone(),
    operation: "app.create".to_owned(),
    target_kind: TargetKind::App,
    title: format!("Creating {app} and resources..."),
    plan,
    completion: RunCompletion {
        success_message: format!(
            "App '{app}' created with {n_services} service(s) and {n_volumes} volume(s)."
        ),
        redirect: Some(format!("/apps/{app}")),
        refresh: RunRefresh::Reports,
    },
    refresh_url: None,
    redactions: Vec::new(),
};
```

5. htmx path:
   - `enqueue_action_run(...)` directly (not `start_action_run`, because the draft must be cleared only after a successful enqueue).
   - On `Ok(run_id)`: `clear_draft(&session); run_fragment(&run_id, request.title.clone(), None)`.
   - On `Err`: `modal_error(err.to_string())`; leave the draft intact.
6. no-JS path:
   - `enqueue_action_run(...)`.
   - On `Ok`: `clear_draft`, success flash “Queued: creating app '{app}'.”, redirect `/activity`.
   - On `Err`: error flash, redirect `/apps/new/review`.
7. Never enqueue twice from one request.

`/activity` is used for the no-JS redirect because the app may not exist yet and the job runs asynchronously.

## Semantics to preserve

- One run, one job. Service create/link and volume create/mount are in the same plan, in the order produced by `to_plan()`.
- The app is created first. If a later step fails, no rollback happens; the run log shows the stopping point. Document this in the UI copy if needed, but do not add rollback.
- The run is audited under the app (`TargetKind::App`, `app.create`).
- No secrets are in the plan, so `redactions` stays empty.
- `RunRefresh::Reports` refreshes the app snapshot after success.

## Tests

- htmx execute returns 200 with `data-run-url`; `common::run_url()` extracts it.
- The mutation commands run in order:
  `AppsCreate`, optional `BuilderSet`, `BuildpacksAdd...`, `ServiceCreate`/`ServiceLink`, `StorageCreate`/`StorageMountEntry`.
- A service-attach-only plan omits `ServiceCreate`.
- A volume attach-path plan calls `StorageMount` with the colon form.
- no-JS execute redirects to `/activity` with a “Queued” flash.
- invalid draft renders a modal error (htmx) or redirects to review (no JS) and enqueues no job.
- after a successful enqueue, `GET /apps/new` shows an empty name field (draft cleared).
- a failed enqueue leaves the draft intact (only possible to simulate with a repository failure; skip if the harness cannot inject one).

Focused command:

```text
docker compose run --rm web cargo test --test wizard execute
```

## Definition of done

- The only execution path is the persisted job queue.
- htmx streams through the existing run modal and redirects to `/apps/{app}` on success.
- no-JS clients get a queued flash and land on `/activity`.
- Draft clearing is coupled to a successful enqueue.
