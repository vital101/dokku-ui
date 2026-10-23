# 11 - Integration tests and verification

Read `00-overview.md` and all previous steps. This is the final gate. Do not change the behavior designed in steps 01-10; add tests and fix only real failures.

## Files

- Complete `tests/wizard.rs`.
- Update any remaining old-flow assertions in `tests/app_routes.rs` / `tests/rbac.rs`.
- No new fixtures are required; reuse existing ones.

## Test harness

Use `mod common;` and `test_state_with_client(MockClient::new().stub(...))`.

Stubs needed by the full wizard:

- `DokkuCommand::DokkuVersion` -> `tests/fixtures/dokku_version.txt`
- `DokkuCommand::PluginList` -> `tests/fixtures/plugin_list.txt`
- `DokkuCommand::Help { family }` for Logs/NginxAccessLogs/NginxErrorLogs -> `logs_help.txt` / `nginx_help.txt`
- `DokkuCommand::AppsList` -> `"=====> My Apps\n"` (or `alpha` for duplicate-name coverage)
- service lists:
  - `ServiceList { postgres }` -> `postgres_list.txt`
  - `ServiceList { mysql }` -> `mysql_list.txt`
  - `ServiceList { redis }` -> `redis_list.txt`
  - `ServiceList { mongo }` -> `mongo_list.txt`
- `DokkuCommand::StorageListEntries` -> `list_entries.json`
- mutation commands used by execute:
  - `AppsCreate`
  - `BuilderSet` (`selected = herokuish` or the chosen builder)
  - `BuildpacksAdd`
  - `ServiceCreate` (default options)
  - `ServiceLink`
  - `StorageCreate`
  - `StorageMountEntry`
  - `StorageMount` (attach-path)
- snapshot/read commands after the run (`AppsReport`, `PsReport`, etc.) can use the client default or explicit `DokkuOutput::ok("")`; do not assert on them.

Helpers to add locally:

```rust
async fn csrf_for(app: &impl Service<...>, path: &str, cookie: &Cookie) -> String
async fn walk_to_review(...) -> Cookie
async fn wait_for_mutation_calls(client: &MockClient, expected: &[DokkuCommand])
fn mutation_calls(client: &MockClient) -> Vec<DokkuCommand>
```

`common::run_url()` already extracts the SSE URL.

## Required tests

### Auth / CSRF / roles

- Unauthenticated GET on every wizard route redirects to `/login`.
- Viewer GET `/apps/new` redirects to `/` with the role flash.
- Viewer POST on every wizard mutation route is 403 (middleware), including `/apps/new/execute`.
- Missing/incorrect CSRF returns 400 on name/build/services/volumes/execute.

### Name

- valid name persists and redirects to Build
- invalid name re-renders and never calls `AppsCreate`
- existing app name (snapshot knows it) is rejected
- reset clears the draft

### Build

- Auto + no buildpacks -> next step
- Auto + URL -> next step
- `herokuish` + URL -> next step
- `dockerfile` + URL -> error
- unknown builder / invalid URL -> error
- back navigation preserves values

### Services

- GET renders fixtures for installed plugins
- attach existing, create new, remove
- duplicate and existing-name create rejected
- capabilities/list failure shows a notice but Review is still reachable
- no `ServiceCreate`/`ServiceLink` during step posts

### Volumes

- GET renders fixture entries (including `legacy-*`)
- create, attach entry (including `legacy-*`), attach path, remove
- duplicate / invalid entry / relative container / whitespace path rejected
- list failure still allows create, attach-path, and Review
- no storage mutations during step posts

### Review

- exact ordered argv:
  ```text
  apps:create alpha
  builder:set alpha selected herokuish
  buildpacks:add alpha https://example.com/bp
  postgres:create cache
  postgres:link cache alpha
  storage:create uploads
  storage:mount alpha uploads --container-dir /app/uploads
  ```
- attach-path preview contains `storage:mount alpha /srv/files:/app/files`
- invalid draft renders an error and no execute form
- review GET does not call any mutation

### Execute

- htmx POST returns 200 with `data-run-url`
- mutation calls occur in plan order (filter reads out of `client.calls()`)
- create-service path calls `ServiceCreate` before `ServiceLink`
- attach-only path omits `ServiceCreate`
- create-volume path calls `StorageCreate` before `StorageMountEntry`
- attach-path uses `StorageMount`
- no-JS POST redirects to `/activity` with a “Queued” flash
- invalid draft does not enqueue
- successful enqueue clears the draft (`GET /apps/new` shows an empty name)

### Cross-container

Run the existing suite unchanged:

```text
docker compose run --rm web cargo test --test horizontal
```

The wizard adds no storage schema and keeps request-path state in the shared session store, so these tests must remain green.

## Final verification

```text
make fmt
make lint
make test
make coverage-gate
```

- `make test` builds Tailwind first; a bare `cargo test` may fail until `static/css/app.css` exists.
- If coverage is below 90%, add focused tests for the uncovered wizard branches rather than excluding files.
- Do not mark any new test `#[ignore]`.

## Definition of done

- Every wizard route and branch is covered.
- Existing app/service/volume/horizontal tests still pass.
- `make lint` clean and `make coverage-gate` passes.
- `AGENTS.md` matches the implemented behavior.
