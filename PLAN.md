# Dokku UI — Implementation Plan

A server-rendered web UI for Dokku (a "Dokku Pro" clone), starting with authentication, a dashboard, and basic application management.

Non-negotiables: **no SPA**, **Rust + actix-web**, **development in Docker**, **deployable to Dokku itself**, **idiomatic Rust**, **functional patterns**, **TDD**, **>90% line coverage**.

---

## 1. Locked decisions

| Decision | Choice | Rationale |
|---|---|---|
| Language | Rust 2024 edition, stable toolchain | Latest idioms |
| Web framework | actix-web 4 | Required |
| Dokku access | `ssh dokku@$DOKKU_HOST`, executed via **russh** (pure-Rust SSH) | App runs as a normal Dokku app, no docker.sock, no host mounts, works for dev (host.docker.internal) and prod (same host) |
| SSH transport | russh, version pinned, isolated in one module | No openssh binary in the image; `DokkuClient` trait seam contains the risk |
| Auth model | Multi-user, SQLite, argon2id hashes | Matches Dokku Pro; supports roles later |
| First admin | First-run setup wizard (redirect until a user exists) | Discoverable, no secrets in env |
| Sessions | Server-side, SQLite-backed, opaque signed cookie (custom `actix_session::SessionStore`) | Revocable, logout-everywhere possible, functional |
| Templates | Askama (compile-time) | Fastest, template errors are compile errors — most Rust-idiomatic |
| Styling | Tailwind via standalone CLI (no Node) | Modern look, no JS, no npm |
| Local state | SQLite via sqlx (runtime queries, embedded migrations) | Single file, Dokku storage-mount friendly |
| Coverage tool | cargo-llvm-cov, gate at 90% lines | Accurate, works in Linux dev container |
| Crate layout | Single crate, `lib.rs` + `main.rs` split, modules by concern | Domain logic unit-testable without actix |
| CI | Local only for now (`make coverage` gate) | Keep milestone focused |

## 2. Core principles (Rust idioms & functional patterns)

- **Functional core, imperative shell**: all parsing, validation, authorization logic, and data transforms are pure functions in `domain/` and `storage/` repos. IO (SSH, DB, HTTP) lives behind traits at the edges. Handlers orchestrate: parse request → call pure/service functions → render template.
- **Parse, don't validate**: inputs cross boundaries as newtypes. `struct AppName(String)` built only via `TryFrom<&str>`, so a handler holding an `AppName` never re-validates. Same for `Email`, `Password` (validated before hashing), `SessionKey`-adjacent types, `CsrfToken`.
- **Exhaustive enums + match** for commands (`DokkuCommand`), statuses (`ProcessStatus`), errors (`AppError`) — adding a variant forces every consumer to handle it.
- **Errors**: `thiserror` for library-ish error enums inside the crate; `anyhow` never crosses the web boundary — handlers return `Result<Response, AppError>` and `AppError` renders typed error pages. `main` is the only place allowed to `expect`/`unwrap` (startup invariants).
- **Immutability by default**; interior mutability only in state (`Mutex`/`RwLock` around sqlx pool wrappers, caches).
- **Dependency injection everywhere**: `AppState { db, dokku: Box<dyn DokkuClient>, settings }`. Tests inject a fixture-backed `MockClient` and a temp SQLite DB. Tests never open sockets.
- **Lib/bin split**: `src/lib.rs` (everything testable) + `src/main.rs` (5 lines: init tracing, call `dokku_ui::run()`). Enables integration tests.

## 3. Stack / dependencies

| Crate | Purpose |
|---|---|
| actix-web 4, actix-session 0.11 | HTTP + session middleware (custom store) |
| russh 0.63 (default-features off, ring/rsa/flate2) | SSH exec to dokku host |
| tokio (rt-multi-thread, macros, time), futures-util | Runtime, timeouts, `join_all` fan-out |
| sqlx 0.9 (sqlite, runtime-tokio, migrate) | DB, no macros (no DATABASE_URL needed at compile time) |
| argon2 0.6 | argon2id hashing/verification |
| askama 0.16 | Compile-time templates |
| actix-files | `/static` serving |
| serde, serde_json | JSON parsing of `dokku *:report --format json`, session payloads |
| tracing, tracing-subscriber (env-filter) | Structured logs, `#[instrument]` on handlers |
| thiserror | Error enums |
| rand 0.9 | Session ids, CSRF tokens (128+ bits, hex) |
| time 0.3 | Timestamps (UTC unix epoch in DB) |
| ring + subtle | GitHub webhook HMAC-SHA256 + constant-time signature compare |
| async-trait | Dyn-compatible `DokkuClient` trait |
| (dev) bacon, cargo-llvm-cov, rustfmt, clippy | Watch-loop TDD, coverage, lint |

## 4. Architecture

```
dokku-ui/
├── Cargo.toml                 # edition = "2024"
├── Dockerfile                 # prod: multi-stage (cargo-chef + tailwind)
├── Dockerfile.dev             # dev: rust + bacon + cargo-llvm-cov + tailwind CLI
├── docker-compose.yml         # services: web (bacon), css (tailwind watch)
├── Makefile                   # up, test, coverage, lint, fmt, prod-build, deploy-help
├── app.json                   # Dokku healthcheck (/healthz)
├── assets/input.css           # tailwind entry
├── tailwind.config.js         # scans templates/**/*.html + src/**/*.rs
├── migrations/
│   ├── 0001_users.sql
│   └── 0002_sessions.sql
├── templates/
│   ├── base.html              # sidebar + topbar + flash + layout
│   ├── partials/app_badge.html, flash.html, csrf_field.html
│   ├── setup.html, login.html
│   ├── dashboard.html
│   ├── app/show.html, config.html, logs.html, delete.html
│   ├── app/partials/{overview,processes,services,config,logs,loading,error}.html
│   └── error.html
├── tests/                     # integration tests (actix_web::test)
│   ├── auth_flows.rs
│   ├── app_routes.rs
│   ├── fixtures/              # golden dokku output files
│   │   ├── apps_list.txt, ps_report.json, apps_report.json,
│   │   └── config_masked.txt, logs.txt
└── src/
    ├── main.rs                # tracing init + run()
    ├── lib.rs                  # run(config), build_app(state) -> testable
    ├── settings.rs            # Settings::from_env() -> Result<Self>
    ├── error.rs               # AppError (thiserror) + ResponseError impl
    ├── domain/                # PURE — no IO, highest test density
    │   ├── mod.rs
    │   ├── app_name.rs        # AppName newtype + parsing rules
    │   ├── email.rs           # Email newtype
    │   ├── password.rs        # Password newtype (policy), into hash-able secret
    │   ├── command.rs         # DokkuCommand enum -> argv()
    │   ├── parse.rs           # parse_apps_list, parse_config, parse_report_json, ...
    │   ├── validate.rs        # CSRF compare, misc pure validators
    │   └── types.rs           # DokkuApp, ProcessStatus, EnvVar, AppStats, LogLines
    ├── dokku/
    │   ├── mod.rs
    │   ├── client.rs          # trait DokkuClient + DokkuOutput/DokkuError
    │   ├── russh_client.rs    # the ONLY russh-dependent file
    │   ├── snapshot.rs        # Snapshot + SnapshotStore + 30-min background refresher
    │   └── mock.rs            # fixture-backed MockClient (used by tests)
    ├── storage/
    │   ├── mod.rs             # pool init, migrate, WAL
    │   ├── users.rs           # UsersRepo (trait + Sqlite impl)
    │   └── sessions.rs        # SqliteSessionStore (impl actix_session::SessionStore)
    ├── auth/
    │   ├── mod.rs             # hash/verify wrappers (argon2), session helpers
    │   ├── csrf.rs            # CsrfForm extractor
    │   └── middleware.rs      # redirect-to-/login gate + first-run gate
    └── web/
        ├── mod.rs             # route registration (single source of truth)
        ├── state.rs           # AppState
        ├── flash.rs           # set/get/clear flash via session (pure parts)
        ├── render.rs          # askama -> HttpResponse helper
        ├── dashboard.rs, setup.rs, login.rs, logout.rs
        ├── apps/              # list/create/overview/config/logs/delete/actions
        └── security_headers.rs
```

### Key seams (testability contracts)

```rust
#[async_trait::async_trait]
pub trait DokkuClient: Send + Sync {
    async fn exec(&self, cmd: &DokkuCommand) -> Result<DokkuOutput, DokkuError>;
}

pub struct DokkuOutput { pub exit_code: i32, pub stdout: String, pub stderr: String }

pub enum DokkuCommand {
    AppsList,
    AppsCreate { app: AppName },
    AppsDestroy { app: AppName, force: bool },
    AppsReport { app: AppName },
    PsReport { app: AppName },
    PsStart { app: AppName }, PsStop { app: AppName }, PsRestart { app: AppName },
    ConfigShow { app: AppName, masked: bool },
    Logs { app: AppName, num_lines: u32 },
}
impl DokkuCommand { pub fn argv(&self) -> Vec<Cow<'_, str>>; } // pure, unit-tested
```

Clock/randomness for sessions are plain `rand` usage seeded from OS — token generation is a pure function of random bytes (`Token::from_bytes(bytes)`) so entropy itself isn't mocked.

## 5. Data model

```sql
-- 0001_users.sql
CREATE TABLE users (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  email         TEXT NOT NULL UNIQUE COLLATE NOCASE,
  password_hash TEXT NOT NULL,           -- argon2id PHC string
  created_at    INTEGER NOT NULL          -- unix epoch (UTC)
);

-- 0002_sessions.sql
CREATE TABLE sessions (
  id         TEXT PRIMARY KEY,            -- opaque random id (cookie value)
  data       TEXT NOT NULL,               -- session-state JSON (user_id, csrf_token, flash, ...)
  expires_at INTEGER NOT NULL
);
CREATE INDEX idx_sessions_expires_at ON sessions(expires_at);
```

DB opened with `PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON;`. Migrations embedded via `sqlx::migrate!`. Expired sessions purged lazily on save/load (pure function picks purge candidates; repo executes).

## 6. Authentication design

- **Password hashing**: argon2id, `Argon2::default()` params (m=19 MiB, t=2, p=1), PHC string stored. `hash(Password) -> HashString` and `verify(Password, &HashString) -> bool` wrap argon2; unit tests cover hash/verify + wrong password (cost: low-param variant in tests via injectable params).
- **Login flow**: `POST /login` (CsrfForm<LoginRequest>) → `UsersRepo::find_by_email` → verify → `session.renew()` + insert `user_id` + new `csrf_token` → redirect `/`. Failures: generic "Invalid email or password", no user enumeration, session-scoped flash.
- **Setup wizard**: auth middleware short-circuits: `users_count == 0` → any route redirects to `/setup`. `POST /setup` creates the admin, logs in, `/setup` redirects away permanently once users exist (middleware-protected both ways).
- **Middleware** (`auth::middleware`): allowlist `/healthz`, `/static/*`, `/login`, `/setup`, `/favicon.ico`. Everything else requires `session.user_id`. Unauthenticated HTML GET → 302 `/login` (with `next` param honored post-login); POST → 302 `/login` (no auth-gated forms reachable anyway).
- **Logout**: `POST /logout` (CSRF-protected) → delete session row + clear cookie → redirect `/login`.
- **CSRF**: 128-bit token in session data, rendered into every form via a `csrf_field.html` partial, enforced by a custom extractor `CsrfForm<T>` (rejects POST without valid token, double-submit comparison constant-time via `subtle`-backed `constant_time_eq`). Handlers take `CsrfForm<CreateApp>` — impossible to forget.
- **Cookies**: session id set by `SessionMiddleware` with `CookieSecurity::Secure` off in dev (no TLS locally) / on in prod (behind Dokku nginx), `SameSite=Lax`, HttpOnly.
- **Session TTL**: 7-day rolling (actix-session TTL + `expires_at` column).
- Post-v1 (documented, not built): login rate-limiting, email confirmation, password reset.

## 7. Routes

| Route | Method | Auth | Handler → Template | Notes |
|---|---|---|---|---|
| `/healthz` | GET | no | inline 200/503 | checks DB `SELECT 1`; used by `app.json` |
| `/setup` | GET/POST | only when 0 users | `setup.rs` → setup.html | wizard, see §6 |
| `/login` | GET/POST | no | `login.rs` → login.html | `next` redirect support |
| `/logout` | POST | yes | `logout.rs` | |
| `/` | GET | yes | `dashboard.rs` → dashboard.html | stats + app table |
| `/apps` | POST | yes | `apps::create` | name via `AppName`; flash + redirect |
| `/apps/new` | GET | yes | → apps/new.html | no-JS create form page |
| `/apps/{name}` | GET | yes | `apps::show` → app/show.html | `apps:report` + `ps:report` |
| `/apps/{name}/config` | GET | yes | → app/config.html | `config:show --masked` |
| `/apps/{name}/logs` | GET | yes | → app/logs.html | `?lines=N` (200 default, 1000 max, min 10); fragment renders lines |
| `/apps/{name}/partials/{tab}` | GET | yes | `apps::*_partial` | HTMX fragments (overview/processes/services/config/logs) |
| `/apps/{name}/partials/delete-confirm` | GET | yes | `apps::delete_confirm_modal` | delete-confirmation modal fragment |
| `/actions/runs/{id}/events` | GET | yes | `runs::action_events` | generic SSE stream of any run's output + `done` outcome (app, service, or volume) |
| `/refresh` | POST | yes | `pages::refresh_now` | manual full snapshot refresh from the dashboard |
| `/apps/{name}/delete` | GET | yes | → app/delete.html | confirmation page (no-JS fallback) |
| `/apps/{name}/delete` | POST | yes | `apps::delete` | confirm form input must echo app name |
| `/apps/{name}/start` | POST | yes | `apps::action` | HX: streamed run modal; plain: flash + redirect |
| `/apps/{name}/stop` | POST | yes | `apps::action` | |
| `/apps/{name}/restart` | POST | yes | `apps::action` | |
| `/apps/{name}/rebuild` | POST | yes | `apps::action` | |
| `/apps/{name}/scale` | POST | yes | `apps::scale` | HX: streamed run modal; plain: flash + redirect |
| `/services/{plugin}` | GET/POST | yes | `services::index`/`services::create` | `{plugin}` validated against the supported set (postgres/mysql/redis/mongo); POST creates the service (HX: run modal; plain: flash) |
| `/services/{plugin}/new` | GET | yes | → services/new.html | no-JS create form |
| `/services/{plugin}/partials/list` | GET | yes | `services::list_partial` | HTMX fragment: `<plugin>:list` + per-service `<plugin>:info` |
| `/services/{plugin}/{service}` | GET | yes | `services::show` → services/show.html | detail shell, Overview tab + lifecycle/expose/destroy actions; unknown service 404s |
| `/services/{plugin}/{service}/links` | GET | yes | → services/links.html | Links tab shell |
| `/services/{plugin}/{service}/logs` | GET | yes | → services/logs.html | `?lines=N`, same clamp as app logs |
| `/services/{plugin}/{service}/partials/{overview,links,logs,delete-confirm}` | GET | yes | `services::*_partial` | HTMX fragments; failures stay 200 retry cards |
| `/services/{plugin}/{service}/delete` | GET | yes | → services/delete_confirm.html | confirmation page (no-JS fallback) |
| `/services/{plugin}/{service}/{start,stop,restart,destroy,expose,unexpose,link,unlink}` | POST | yes | `services::*` | HX: streamed run modal; plain: flash + redirect. destroy echoes the typed service name; link/unlink take `app` |
| `/volumes` | GET | yes | `volumes::index` → volumes/list.html | shell; data via partial |
| `/volumes/partials/list` | GET | yes | `volumes::list_partial` | all-apps `storage:report` in one command + mount form |
| `/volumes/mount` | POST | yes | `volumes::mount` | `MountSpec`-validated before any SSH; HX: run modal; plain: flash |
| `/volumes/unmount` | POST | yes | `volumes::unmount` | takes `app` + `host:container` locator |
| `/static/*` | GET | no | actix-files | immutable cache headers, content-hash names |
| `404/500` | — | — | error.html | via `AppError: ResponseError` |

All POSTs are `CsrfForm<T>` + auth-gated. Actions stay no-JS-safe: without JS the forms POST and redirect with a flash, and delete uses the standalone confirmation page. With JS, action forms post via htmx into a modal that streams the command's output over SSE (buttons disable while a run is in flight), and delete opens a confirmation dialog first. App-tab data loading is HTMX (shell renders first, fragments fill in); without JS the shells still show nav and actions, just no data.

## 8. Dokku integration

### Command mapping (pure, exhaustive)
`DokkuCommand::argv()` produces e.g. `["apps:destroy", "my-app", "--force"]`. Unit tests assert the exact argv for every variant (table-driven tests).

### Parsers (pure, fixture-locked)
- `parse_apps_list` — tolerant line-split, trims, skips blank lines, strips ANSI.
- `parse_report_json::<Report>` — serde over `dokku ... --format json`; typed structs (`PsReport`, `AppsReport`) with `#[serde(default)]` for version drift; unknown fields ignored.
- `parse_config_show` — key=value lines; `--masked` values flagged; detects dokku's two formats (config:show vs `config:show --masked` differs per version).
- `parse_logs` — strip ANSI color codes (pure regex/char filter), pass-through lines, cap length.
- Golden fixture files under `tests/fixtures/` for each dokku output format; parsers tested against fixtures, **and** fixtures are validated once against the real host (documented dokku version) before milestone sign-off.

### russh client (isolated)
- Connect: `DOKKU_SSH_USER@DOKKU_HOST:DOKKU_SSH_PORT`, ed25519 key from `DOKKU_SSH_KEY_PATH` (OpenSSH/PEM formats accepted).
- **Persistent SSH session**: the client keeps one authenticated connection alive (keepalive 30s) and opens one channel per command; stale sessions are detected (`is_closed`/transport errors) and reconnected transparently. (v1 shipped one session per command; bursts of ~15 connections per dashboard load tripped the host's ufw rate limit on port 22, so pooling was pulled forward.)
- Collect stdout/stderr, read exit status. Read commands (reports, config, logs, lists) are bounded by `tokio::time::timeout` at `COMMAND_TIMEOUT_SECS`; mutating actions (`ps:start/stop/restart/rebuild`, `ps:scale`, `apps:destroy`) have **no timeout** (`CommandTimeout::Indefinite`) — long builds stream to completion and the SSH keepalive surfaces dead connections instead of a timer.
- Host key policy: accept-on-first-use into `known_hosts`-style file at `DOKKU_SSH_HOST_KEYS_PATH`; optional pre-pinned file works read-only.
- Errors: `DokkuError::Connect | Timeout | Exit { code, stderr }` — `Exit` messages surface stderr for the user flash; `Connect/Timeout` render 503-style error page.
- A background refresher (`SnapshotStore` + `spawn_refresher`) keeps a cheap snapshot of the whole host (app list + `ps:report` + `apps:report`, fetched concurrently with 4 permits over the persistent SSH session) on a **30-minute** cadence (`SNAPSHOT_REFRESH_SECS`, default 1800). The heavier per-app details (builds, domains, service links, DNS) are **not** fetched in the background. The snapshot is serialized to a singleton `snapshots` row in SQLite (`src/storage/snapshots.rs`), so every container serves the same data and a cold-starting container reads the row with **zero SSH**; the dashboard renders from it (no IO beyond one row read + JSON parse), the first request after a completely empty DB falls back to one synchronous refresh, and a **"Refresh data"** button (`POST /refresh`) forces a fresh pass on demand (instantly visible to every container). After start/stop/restart/scale/create only that app's cheap ps/apps reports are re-fetched (`refresh_app_reports`, applied to the shared row under a `BEGIN IMMEDIATE` read-modify-write so concurrent per-app patches from different containers never lose siblings, and a full refresh skips its save when the row was patched after its build started — the row's millisecond `updated_at` marker) before redirecting; the overview fragment then fetches the app's details on demand, so an action click no longer pays for two full detail passes. Destroy triggers a full refresh. A miss (app not listed) triggers a live `apps:list` fallback before 404ing, covering apps created via the CLI within the staleness window. Refresh failures keep the last published row and log a warning. dokku boots its plugin system per command (~750ms), so keeping the background pass small is what keeps host load down. A single multi-app `ps:report` invocation was tried and rejected: dokku 0.38 only reports the first app argument.
- **App pages load on demand**: every `/apps/{name}/*` route renders an instant shell (nav, action buttons, skeleton) and each tab's data is fetched by an HTMX fragment request to `/apps/{name}/partials/{tab}`. The overview fragment calls `SnapshotStore::refresh_app` (ps/apps report + builds/domains/links/DNS for that one app); processes/config/logs query dokku directly; services fetches links live (`plugin:list` + `app links`). Failed fragments render a small retry card (HTTP 200) instead of a full-page error.
- **Streamed action runs**: start/stop/restart/rebuild/scale/destroy execute as a persisted run. The handler responds instantly with a run fragment; `static/js/actions.js` opens an `EventSource` on the generic `/actions/runs/{id}/events` route (one endpoint serves app, service, and volume runs), which replays buffered output and follows the run until a `done` event (`{ok, message, redirect}`). Runs and their lines live in `action_runs`/`action_run_lines` (`src/storage/runs.rs`) with random 64-hex ids, so the stream can be served by **any** process — the SSE handler polls the DB with a 25→400ms backoff instead of a tokio watch. Output is line-buffered server-side; `DokkuClient::exec_streaming` forwards SSH chunks as they arrive (`RusshClient`, same timeout semantics as `exec`; the trait default emits stdout as one chunk so mocks need no changes). Finished runs are retained 5 minutes so a page reload mid-run replays the log; runs whose owner process died are swept to a failed outcome after 10 minutes without output or a heartbeat (`ORPHAN_AFTER_SECS`, matching the SSE handler's own stall detector — the run task bumps the row every 60s, so a silent-but-alive build is never falsely failed). The SSE response sets `X-Accel-Buffering: no` so nginx does not buffer it. On success the run task refreshes the snapshot (`refresh_app_reports`, a full refresh after destroy, or nothing for service/volume runs whose `data-refresh` re-pulls the affected partial) while the browser swaps in the outcome banner and refreshes the current tab fragment.

## 9. Templates & UI

- `base.html`: fixed sidebar (Dashboard; App list; per-app nav: Overview, Config, Logs) + topbar (user email, logout) + flash partial (success/error banners).
- Pages listed in §7. Askama derive structs (`#[derive(Template)] #[template(path = "app/show.html")]`) hold plain domain types — templates can't do logic beyond simple display, which keeps HTML pure.
- Tailwind: `assets/input.css` → `static/css/app.css` (watch in dev via css service, minified in prod build). Content scan covers `templates/**` and `src/**/*.rs` (badge classes built in Rust).
- HTMX 2.0.4 (vendored at `static/js/htmx.min.js`, `defer` in `base.html`): app-tab pages are shells with `hx-get`/`hx-trigger="load"` skeletons, and the logs line-count form re-fetches the fragment via `hx-include`. No inline scripts, so the existing CSP keeps working; htmx's auto-injected indicator `<style>` tag is disabled via a `htmx-config` meta tag because `style-src 'self'` (no `unsafe-inline`) would block it. Fragments always answer 200 — failures (including "app was deleted") swap in a retry card, since htmx does not swap 4xx/5xx responses and the skeleton would spin forever. Actions post into `#modal-content` (overlay lives in `apps/base_app.html`): a native submit listener disables all `.app-action` controls while a run is in flight and the clicked one spins (CSS in `assets/input.css`); run modals expand to a near-fullscreen, internally-scrolling output window (`data-run-open` sizing in `assets/input.css`), while confirm/error dialogs stay small. `static/js/actions.js` (external, CSP-safe) consumes the SSE stream, shows the green/red outcome banner, refreshes the current fragment via `htmx.ajax`, and navigates home when the `done` payload carries a redirect (destroy). Vendored rather than CDN so the UI works offline/self-hosted; the prod Dockerfile copies the whole `static/` tree. Dark palette, clean tables, status badges (green/red/gray) — Dokku Pro-like.
- Security headers middleware: `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, `Content-Security-Policy: default-src 'self'; style-src 'self'; img-src 'self' data:`.

## 10. Configuration (env)

| Var | Dev default | Prod | Purpose |
|---|---|---|---|
| `PORT` | 8080 | set by Dokku | listen port |
| `DOKKU_HOST` | `host.docker.internal` | the host / service name | SSH target |
| `DOKKU_SSH_PORT` | 22 | 22 | |
| `DOKKU_SSH_USER` | `dokku` | `dokku` | |
| `DOKKU_SSH_KEY_PATH` | `./dev-data/ssh/id_ed25519` | `/app/data/ssh/id_ed25519` | ed25519 private key |
| `DOKKU_SSH_HOST_KEYS_PATH` | `./dev-data/ssh/known_hosts` | `/app/data/ssh/known_hosts` | first-use store |
| `DATABASE_URL` | `sqlite://dev-data/dokku-ui.db` | `sqlite:///app/data/dokku-ui.db` | sqlx SQLite |
| `SECRET_KEY` | dev value committed | Dokku config secret | actix-session cookie signing (≥32 bytes hex) |
| `SESSION_TTL_SECS` | 604800 | — | |
| `COMMAND_TIMEOUT_SECS` | 30 | — | SSH exec timeout for read commands; mutating actions are unbounded |
| `SNAPSHOT_REFRESH_SECS` | 1800 | — | background snapshot refresh interval (positive; 30 min) |
| `COOKIE_SECURE` | false | true | Secure flag |
| `ACTIVITY_TTL_SECS` | 7776000 (90d) | — | audit run retention |
| `RUN_LOG_TTL_SECS` | 604800 (7d) | — | run-line retention (metadata outlives it) |
| `REAUTH_TTL_SECS` | 300 | — | reveal/edit re-auth window |
| `RUST_LOG` | `dokku_ui=debug,tower? n/a` | `info` | tracing filter |

`Settings::from_env()` is a pure function of a `VarMap` (testable without process env); `main` feeds it `std::env`.

## 11. Docker

### Dev (`Dockerfile.dev` + `docker-compose.yml`)
- Base `rust:1-bookworm`; rustup component `llvm-tools-preview`; cargo install `bacon`, `cargo-llvm-cov`; download `tailwindcss` standalone binary (linux x64/arm64 by arch).
- Named volumes: `cargo-cache` (registry/target), bind `./` for live code, `./dev-data` for sqlite + ssh keys.
- Services:
  - `web`: `bacon run` or `bacon test` (long-running, TTY), port 8080, env from `.env.dev`.
  - `css`: `tailwindcss -i assets/input.css -o static/css/app.css --watch`.
- `make up` = `docker compose up`; `make test` = `docker compose run --rm web bacon test`; `make coverage` runs `cargo-llvm-cov --html --fail-under-lines 90` inside the Linux container.

### Prod (`Dockerfile`)
```dockerfile
FROM lukemathwalker/cargo-chef:latest-rust-1 AS chef        # layer caching
FROM chef AS plan
WORKDIR /app
COPY . .
RUN cargo chef prepare --recipe-path recipe.json
FROM chef AS build
WORKDIR /app
COPY --from=plan /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json      # deps cached
COPY . .
RUN cargo build --release --bin dokku-ui
# css stage: download tailwind standalone, build minified app.css
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
RUN useradd --system --create-home --uid 1000 dokku-ui
WORKDIR /app
COPY --from=build /app/target/release/dokku-ui /app/dokku-ui   # askama templates embedded in binary
COPY --from=css /static/css/app.css /app/static/css/app.css
USER dokku-ui
ENV PORT=8080
EXPOSE 8080
ENTRYPOINT ["/app/dokku-ui"]
```
No Node, no openssh, ~minimal surface. `app.json`:

```json
{ "healthchecks": { "web": [{ "type": "path", "name": "web health check", "description": "Checks /healthz", "path": "/healthz" }] } }
```

### Dokku deploy runbook (target: `dokku dokku-ui` app)
```
dokku apps:create dokku-ui
dokku storage:mount dokku-ui /var/lib/dokku/data/services/dokku-ui:/app/data
dokku config:set dokku-ui SECRET_KEY=<64-hex> DOKKU_HOST=<host> COOKIE_SECURE=true \
  DATABASE_URL=sqlite:///app/data/dokku-ui.db DOKKU_SSH_KEY_PATH=/app/data/ssh/id_ed25519 ...
# generate keypair for the app, authorize on the host:
ssh-keygen -t ed25519 -f /var/lib/dokku/data/services/dokku-ui/ssh/id_ed25519 -N ''
dokku ssh-keys:add dokku-ui /var/lib/dokku/data/services/dokku-ui/ssh/id_ed25519.pub
git remote add dokku dokku@<host>:dokku-ui
git push dokku main
```
The app then manages the host it runs on (its own key is scoped as a dokku remote user; least-privilege per-user command policy is a post-v1 hardening note).

**Scaling**: `dokku ps:scale dokku-ui web=3` is safe. All request-path state (sessions, action runs, snapshot) lives in SQLite on the shared `/app/data` mount — WAL + 5s busy timeout, and dokku scale-up is same-host, so all containers share the file. Run ids are random 64-hex tokens and the SSE stream DB-polls, so a modal started on container A streams fine from container B. Each container keeps its own background refresher (N full SSH passes per 30 min across N containers — bounded, acceptable), and a full refresh never clobbers a per-app patch that lands mid-build (the row's millisecond `updated_at` makes the refresh skip its save). On a fresh first deploy, all containers run migrations at boot simultaneously; `storage::connect` retries (5 attempts, 0.5s-doubling backoff) so the losers pick up the winner's schema instead of crash-looping.

## 12. Testing strategy (TDD, >90%)

**Loop**: for every module — write failing test (unit against pure function / integration against route with mock state) → implement → green → refactor. `bacon` watch keeps the loop instant inside the container.

| Layer | What | How |
|---|---|---|
| Unit — domain | `AppName` accept/reject table, `Email`, `Password` policy, `DokkuCommand::argv` table, parsers vs golden fixtures (happy + ANSI noise + malformed + empty), `AppStats` folds, CSRF compare (equal/unequal/lengths) | pure functions, no IO |
| Unit — auth | hash→verify roundtrip, tampered hash fails, argon2 PHC parse, token encoding | low-cost argon2 params injected |
| Unit — storage | repos + session store against `tempfile` SQLite: save/load/delete, expiry purge, login/upsert, unique-email error | `SqlitePool::connect_tmp`/tempfile |
| Unit — settings | `from_env` full/partial/invalid | fake env map |
| Integration — web | actix `test::init_service(build_app(TestState))`: auth gate redirects (all protected routes, parametrized), login success/failure/`next`, setup wizard first-run lockout, CSRF rejection (missing/bad token on every POST), app CRUD flows with `MockClient` (create → list contains; destroy needs name echo; start/stop/restart flash), config page renders masked values, logs `lines` clamping, 404 error page, healthz | `MockClient` fixtures + test DB |
| Coverage | `make coverage` → `cargo llvm-cov --fail-under-lines 90` (HTML report at `target/llvm-cov/html/index.html`) | run in Linux dev container |

Coverage protection rails: `main.rs` stays trivial; anything nontrivial lives behind `run()`/`build_app()` in lib. HTML-specific logic (class-picking helpers etc.) lives in pure functions to stay covered.

## 13. Milestones (each TDD, each ends green + lint + coverage)

1. **Scaffold**: Cargo (lib+bin), both Dockerfiles, compose, Makefile, tailwind pipeline, `run()`/`build_app()`/`healthz`, git init. DoD: `make up` serves styled `/healthz`; `make coverage` works end-to-end; `make lint` (fmt check + clippy `-D warnings`) clean.
2. **Foundations**: `Settings`, `AppError` (+ ResponseError → error.html), migrations, pool init, repos skeleton. DoD: unit tests green.
3. **Domain layer**: newtypes, validators, `DokkuCommand` argv table, parsers + fixtures. DoD: pure-function tests only, no actix dep touched.
4. **Dokku seam**: `DokkuClient` trait, `MockClient`, russh impl (integration-verified manually against `DOKKU_HOST` with a scratch app). DoD: trait-level tests for mock; manual smoke of russh.
5. **Auth**: argon2 wrappers, `SqliteSessionStore`, CSRF extractor, middleware, login/logout/setup pages + base layout. DoD: integration suite for gate/CSRF/login/setup.
6. **Dashboard**: `/` with stats + app table (concurrent `ps:report` fan-out). DoD: mocked integration test asserts statuses/badges.
7. **App management**: new/create/show/delete-confirm/destroy. DoD: full flow integration test incl. name-echo confirmation and error flash on `Exit{code}`.
8. **App ops**: start/stop/restart actions, config view, logs viewer with line clamping. DoD: integration tests per action + parse fixtures.
9. **Polish & gate**: security headers, cookie flags, favicon, empty-states, error pages; coverage ≥ 90%; fmt/clippy clean; prod image builds; deploy runbook verified on real dokku host.

## 14. Risks & mitigations

| Risk | Mitigation |
|---|---|
| russh API churn | Pin exact version; confined to `dokku/russh_client.rs`; trait seam lets us swap to another impl without touching domain/web |
| dokku output varies by version | `--format json` wherever available; `#[serde(default)]`; golden fixtures validated against real host; tolerant parsers |
| actix-session store API friction | Small adapter (save/load/delete) around sqlx; isolated in `storage/sessions.rs` |
| Coverage gate flakiness on macOS | All coverage runs inside Linux container |
| sqlite contention | WAL + busy_timeout; multi-container via the shared `/app/data` mount (same-host only) with per-app patches under `BEGIN IMMEDIATE` read-modify-write |
| Tailwind standalone binary availability | Pinned version; verify arm64 + x64 in scaffold milestone |

## 15. Explicit defaults (change on request)

- Password minimum length **12**; argon2id default params; no complexity theater.
- Config page shows **masked** values only in v1 (unmask/edit is post-v1 with re-auth).
- Session TTL **7 days** rolling; host key **accept-on-first-use** with optional pinned file.
- Logs default **200** lines, max **1000**, newest-last (dokku order).
- Horizontal scaling: `ps:scale web=N` is supported — all request-path state lives in SQLite on the shared `/app/data` mount (see §11; same-host only). UTC everywhere.
- No login rate-limiting in v1 (documented follow-up).
- App Detail v1: scale per process type capped at **100** (UI guard); `ps:scale`
  timeout **120s**, `ps:rebuild` timeout **300s**; resources display-only;
  live CPU/memory deferred (no dokku command).

## 16. Post-v1 backlog (context, not scope)

Deployments & build logs, one-off `run` commands, user management/roles UI, login rate limiting, manual certificate upload, ssh-keys management, plugin screens, backup/export, i18n, themes. (P0 foundations — config editing with re-auth reveal, live log tailing over SSE, durable audit + job queue + toasts, capability detection, RBAC/re-auth seams — the P1 app configuration UI — rename, deploy lock, domains, resource limits, cron, build config, plugin-gated maintenance/basic auth — and the P2 deploy paths + TLS work — Deploy tab, `git:sync`/`from-image`/`from-archive`, GitHub webhooks, Let's Encrypt + cert status, log process filter + failed deploy logs — are landed; see `GAPS.md`.)

### Deferred from App Detail v1

- **Live CPU/memory utilization.** Dokku exposes no `docker stats` passthrough
  over SSH, so v1 shows configured limits (`resource:report`) and container
  state (`ps:inspect`) instead. Fast-follow: a tiny host-side plugin
  (`dokku ui:stats <app>`) wrapping `docker stats --no-stream`.
- **Resource limit/reserve editing.** Display-only in v1; setting requires a
  rebuild to take effect.
- **Service link/unlink.** Display-only in v1 (unlink unsets env vars and
  restarts the app).
- **Per-process restart** (`ps:restart <app> <process-type>`).

## 17. App Detail v1 (implemented)

Extends the per-app UI with deep runtime detail, read from the same
`DokkuClient` seam (all commands pure/argv-tested in `domain/command.rs`):

- **Processes tab** (`GET /apps/{name}/processes` + `.../partials/processes`): desired
  formation (`ps:scale <app>`) merged with observed states (`ps:report`),
  a scale form (`POST /apps/{name}/scale`, `CsrfForm`, capped at
  `SCALE_MAX = 100`, hidden when `ps-can-scale=false` or no formation; the
  `release` process type is shown read-only), per-container detail
  (`ps:inspect`), and resource limits/reservations (`resource:report`). Pure
  assembly lives in `dokku/processes.rs`.
- **Services tab** (`GET /apps/{name}/services` + `.../partials/services`):
  linked services fetched live on demand (`PluginList` + `app links`),
  enriched per service via `<plugin>:info <service>`
  (plain-text report). DSNs are never parsed or rendered; the card shows status,
  version, exposed ports, internal IP, container ID, and linked apps.
- **Rebuild action** (`POST /apps/{name}/rebuild`): reuses the start/stop/
  restart action path. Mutating actions run without a timeout so long builds
  stream to completion. All actions stream their output into
  the modal (see §8), and scale validation failures render a modal error card
  instead of a redirect.
- **Overview enrichment**: last build (`builds:report`), vhost list
  (`domains:report`), and full linked-service list — fetched live for that app
  when the overview fragment loads (`SnapshotStore::refresh_app`), never by the
  background refresher.
- **Degradation:** every detail command is best-effort; failures render empty
  sections / an "unknown" card (or a retry fragment for overview/config/logs)
  rather than a 5xx. Unknown apps still 404 via
  `SnapshotStore::resolve_app`.

**Version constraint:** the target host runs dokku 0.38.4 with redis plugin
1.42.1 and postgres 1.36.4. Those versions do **not** support `--format json`
on `ps:scale` or on `<plugin>:info` (current dokku.com docs describe a newer
release), so both are parsed from plain text. Fixtures were captured from the
host (service DSNs redacted); see `tests/fixtures/README.md`.

---

**Definition of done (v1)**: every route behind auth (except healthz/login/setup/static); CSRF on all POSTs; deployable via `git push dokku main` with healthcheck; `make coverage` ≥ 90% lines; `cargo clippy -D warnings` and `cargo fmt --check` clean; all dokku parsing covered by golden fixtures validated against the target host.

---

## 18. Service & storage pages (implemented)

Sidebar sections for the four installed service plugins (PostgreSQL, MySQL,
Redis, MongoDB) plus app bind mounts.

- **Routes.** Generic `/services/{plugin}` (validated against
  `ServicePlugin::all()`), `/services/{plugin}/new`, and
  `/services/{plugin}/{service}` with Overview/Links/Logs tabs and matching
  `partials/*` fragments. Actions: create, start/stop/restart, destroy (typed
  name), expose/unexpose, link/unlink. Volumes: `/volumes`, mount and unmount
  forms. Unknown plugin/service names 404 on full pages; partial failures stay
  HTTP 200 retry cards.
- **Data.** `plugin_services` runs `<plugin>:list` then a live `<plugin>:info`
  per service (unknown on failure, DSN never parsed); `service_logs` bounds
  output via the positional empty follow-flag slot (`<plugin>:logs svc '' N`),
  which required `russh_client::shell_command` to quote argv; `app_mounts`
  parses the all-apps `storage:report` in one command. Nothing new is cached in
  the snapshot — these pages are live-on-load, like app tabs.
- **Runs.** All actions stream through the shared, persisted-run SSE modal. The
  run route is now generic (`/actions/runs/{id}/events`), the shared pieces
  live in `src/web/fragments.rs`/`src/web/runs.rs`, and `RunRefresh::None`
  handles subjects that are not in the app snapshot (the modal's `data-refresh`
  re-pulls the affected partial instead).
- **Fixtures.** Service-list/log and storage-report fixtures are synthetic but
  source-verified against dokku-redis 1.42.1 and dokku `v0.38.4`; re-capture
  from the live host next session (see `tests/fixtures/README.md`).
- **Stats (no native dokku command).** The service Overview tab lazily loads a
  Resources card (`/services/{plugin}/{service}/partials/stats`): memory
  working set (`memory.current − inactive_file`, docker-stats parity) against
  the container limit or host total, sampled CPU % (~1s, normalized by cores)
  plus cumulative CPU time, data-dir `du` and host-filesystem `df`. The fixed
  read-only script runs through `<plugin>:enter`; a stopped container renders a
  "not running" state. The Volumes page maps each mount to its
  `legacy-<hash>` entry via `storage:list-entries --format json` and lazy-loads
  per-row usage via `storage:exec` (throwaway `alpine:3`, entry at `/data`),
  plus a host-disk summary card. Everything degrades to `—`; failures never
  block the page. Both scripts are single-line and free of `'` because dokku's
  SSH wrapper re-splits `$SSH_ORIGINAL_COMMAND` with `xargs -n 1` + `readarray`
  (shell `'\''` escapes and newlines do not survive that re-split).
- **Deferred:** named storage entries (`storage:create/destroy/info`), service
  clone/promote/backups, pause, per-app storage tab.

## 19. P2: Deploy paths + TLS (implemented)

- **Deploy tab** (`GET /apps/{name}/deploy` + `.../partials/deploy`): push URL
  card (scp-style on port 22, `ssh://` URL form otherwise — scp syntax cannot
  carry a port), deploy public key card (the host has no generated deploy key;
  the absent case renders the `git:generate-deploy-key` guidance), git summary
  from `git:report` (explicit/computed deploy branch, commit only when it is
  hash-shaped — the report prints the literal `HEAD` on unborn refs — source
  image, deploy-branch ref mtime formatted UTC), and a deploy-branch set/clear
  form (`git:set <app> deploy-branch [value]`).
- **Deploy actions** run as jobs with no timeout: `git:sync` (flags
  `--build`/`--build-if-changes` before the app, optional ref),
  `git:from-image`, `git:from-archive`. Validators in `domain/git.rs` are
  re-checked at job rehydration; remotes with userinfo add the full URL,
  `user:token`, and token to `JobPayload.redactions`. Audit operations
  `git.sync`/`git.from-image`/`git.from-archive`, completion refresh `Reports`.
- **GitHub webhooks.** Migration `0010_app_webhooks.sql` stores one config per
  app (repo, branch, build mode, secret, enabled). `POST
  /webhooks/github/{app}` is the first unauthenticated route (allowlisted by
  the `/webhooks/` prefix): 256 KiB body cap, constant-time HMAC-SHA256
  verification (`ring` + `subtle`), `ping` → 200, `push` → repo/branch filter
  (host-aware — `repository.full_name` is only trusted for `github.com`
  remotes) → enqueue `git:sync` with the configured build mode. Unknown or
  disabled apps 404; bad signatures 401. Deliveries are attributed to the
  `github-webhook` system actor (no user id, so they never enter a toast tray)
  and both the remote's credentials and the webhook secret are redacted from
  persisted run lines. The secret is generated once per config, survives
  edits, and is revealed only under the re-auth window with `no-store`.
- **TLS tab** (`GET /apps/{name}/tls`): `letsencrypt:list`/`active` status with
  expiry and renewal countdowns; enable/disable/revoke/cleanup and the
  server-wide `letsencrypt:cron-job --add/--remove` (all `Plugin`-gated, so a
  host without the plugin renders the explanatory state); a read-only
  `certs:report <app>` certificate card (Core, plain text on 0.38.4).
- **Log quick wins.** `logs [-p|--ps <process>]` filters the app log source
  (validated by the process-type validator; the SSE stream takes `?process=`,
  applied only to the app source) on both the bounded partial and the live
  panel. `logs:failed <app>` feeds a lazy Deploy-tab card; the parser drops the
  dokku header/banners and keeps `remote:` error lines. An empty deploy renders
  "No failed deploy logs." — 0.38.4 prints no warning line for that case.
- **Fixtures.** Captured from the live host on 2026-10-06
  (`git_report*`, `git_public_key_missing`, `git_sync`, `letsencrypt_*`,
  `certs_report*`, `logs_failed`); `logs_failed_populated.txt` is synthetic,
  source-verified against dokku `v0.38.4` `plugins/logs/logs.go`. See
  `tests/fixtures/README.md`.

## 20. Git HTTP server spike (paper deliverable)

Goal (Pro parity): per-app HTTP(S) push URLs (`git push https://…`), no
client SSH key needed. Current architecture cannot implement this in the app
container: the UI has no docker.sock, no root on the host, and only the dokku
SSH command surface. The viable shape is a **companion helper** installed on
the host, invoked over the existing SSH seam — not a second network service in
our container (which would require a port, TLS termination, and host
filesystem access to dokku's git repos).

Options evaluated:

1. **`git-http-backend` shim via companion plugin.** A small host script
   (`dokku ui:git-http …`) that nginx/dokku fronts; auth via a per-app token
   minted in our SQLite and verified by the helper (it cannot read our DB, so
   the token would be written to a host file via an SSH invocation, or the
   helper calls back to our authenticated API — neither is clean).
2. **Companion git daemon + dokku receive hook.** `git daemon --inetd`-style
   exposure of `/home/dokku/<app>` with `git-http-backend` and a pre-receive
   hook that validates a token embedded in the URL userinfo. Auth material is
   stored host-side, managed through a `ui:git-token` dokku command the UI
   invokes over SSH.
3. **Native git-over-SSH only (status quo).** Push URLs shown today
   (`dokku@host:app`) already work for anyone whose key is authorized on the
   host; Pro's HTTP server mainly removes key management.

Constraints/decision: any option needs host-side installation and root during
setup (both permitted per GAPS.md "Locked decisions" item 2), plus a durable
per-app token store. Option 2 has the smallest moving surface: one companion
command family (`ui:git-token set/clear`, `ui:git-server status`) invoked over
SSH, one nginx snippet installed by the helper, and token auth handled entirely
host-side (the UI never serves git traffic). Deferred until a drop explicitly
schedules companion-helper infrastructure; no code is committed for this spike.
Until then the Deploy tab exposes the SSH push URL, which needs no host changes.

## 21. P3 increment (implemented)

- **Multi-user + RBAC**: `users.role` (migration `0009`) is now enforced. The
  auth middleware resolves the session user (`find_by_id`) on every request,
  purges sessions whose user was deleted, and gates via pure
  `permission_for(method, path)` (`src/auth/rbac.rs`): `/users*` and
  `/settings*` need `ManageUsers`; GETs are `View`; self-service POSTs
  (`/logout`, `/password`, `/reauth`, toast acks) are open to every role;
  everything else is `ManageApps`. Unauthorized requests render the 403 error
  page. `UsersRepo` gains `list`/`count_admins`/`update_role`/
  `update_password_hash`/`delete`; `insert` takes the role explicitly. Unknown
  roles fail closed to `Viewer`. Admin `/users` screen creates users with an
  initial password (role select), changes roles, and deletes; pure last-admin
  guards prevent deleting/demoting the final admin and self-deletion.
  `/password` is self-service with current-password verification.
- **Advanced service create**: `ServiceCreateOptions` (`src/domain/service_create.rs`)
  carries `image`, `image_version`, `custom_env`, `config_options` — the flags
  source-verified against dokku-postgres 1.36.4 `subcommands/create`
  (flags follow the service name; `--custom-env` is semicolon-delimited).
  Validation is enforced at parse and re-checked at job rehydration; custom-env
  values are added to `JobPayload.redactions`. The `/services/{plugin}/new`
  form has a collapsible advanced section.
- **Datastore catalog 4 → 8**: `ServicePlugin` adds mariadb, memcached,
  rabbitmq, clickhouse (labels + data dirs). The sidebar is served by
  `/partials/service-nav`, which filters `ServicePlugin::all()` through the
  capabilities `plugin:list` inventory (Unknown/absent probe shows the full
  catalog). Unknown plugin URLs still 404.
- **Instance settings**: migration `0011_instance_settings.sql`, shared SQLite
  via `SqliteInstanceSettingsRepo`. Admin `/settings` edits public URL, login
  banner (rendered on `/login`), app-filter globs (dashboard hides matches via
  `dashboard_from_snapshot_filtered`, everything else stays URL-manageable), and
  activity/run-log TTL overrides. TTLs are read by the run-prune path
  (`effective_ttl_secs`) with the env policy as fallback.
- **Polish**: light/dark theme by overriding Tailwind's color variables under
  `[data-theme="light"]` (no template sweep) with a cookie-backed toggle in
  `static/js/theme.js`; ⌘K palette (`/palette.json`, role-gated entries,
  `static/js/palette.js`, markup in `base.html` so Tailwind scans it); dashboard
  "deleting…" badge fed by `active_destruction_subjects()`.
- **Not in this increment**: hiding every mutation control for viewers (the
  server-side 403 is authoritative; the Users/Settings nav is htmx-gated),
  REST JSON:API/JWT/Swagger/batch ops, multi-server, reverse-proxy auth, SSH
  keys, teams, plugin management, git HTTP, DSN reveal, reset links.

## 22. P3 follow-up (implemented)

- **Password reset links** (`migrations/0012_password_resets.sql`): tokens are
  `auth::csrf::generate_token()` values rendered once and stored as SHA-256
  hashes (`domain::password_reset`); `create` invalidates the user's previous
  unused links and garbage-collects used/expired rows, `consume` is a single
  atomic `UPDATE … RETURNING` (single-use, 24h TTL) that runs *before* the
  password write, so a DB failure after consumption simply requires a new link.
  Admins POST `/users/{id}/reset` − requires the instance public URL, otherwise
  a flash error − and share the resulting `/reset/{token}` link out of band;
  the rendered link page is `Cache-Control: no-store`. `/reset/{token}` is
  public in the auth middleware (`/reset/` prefix), uses the session CSRF flow,
  and a successful reset flashes on `/login`.
- **Session revocation**: `SqliteSessionStore::delete_user_sessions(user_id)`
  deletes every session whose state carries that `user_id`
  (`json_extract(data,'$.user_id')`). Self-service password changes purge all
  of the user's sessions and re-issue a fresh flash-bearing session (sign out
  everywhere); generating an admin reset link purges the target's sessions at
  link-creation time, so a copied cookie dies immediately. Anonymous
  pre-login sessions are never touched.
- **Destroy-while-linked guard** (`src/web/services.rs`): `service.destroy`
  fetches `<plugin>:links` after the typed-name check and refuses with the
  linked app list; a link-check failure refuses too (fail closed). The check
  runs at enqueue time, so an app linked in the small window before the
  immediate executor claims the job can still be destroyed; an execution-time
  precondition is the Pro-parity follow-up.
- **`letsencrypt:set`**: `DokkuCommand::LetsencryptSet` + `JobSpec` arm
  (`email` must pass `is_valid_letsencrypt_email` — `Email` rules plus no `'`,
  which dokku's SSH `xargs` re-split cannot carry; `staging` requires an
  explicit `true`/`false` because plugin 0.20.4 has no clear form), an audited
  run from the TLS tab, and re-validation at job rehydration.
- **Not in this increment**: SSH-key management, teams, plugin management,
  REST JSON:API/JWT/Swagger, multi-server, reverse-proxy auth, git HTTP,
  DSN reveal.
