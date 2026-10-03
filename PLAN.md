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
| actix-web 4, actix-session 0.10, actix-web-lab? (no) | HTTP + session middleware (custom store) |
| russh 0.5x (pinned) + russh-keys? (bundled in russh 0.45+) | SSH exec to dokku host |
| tokio (rt-multi-thread, macros, time), futures-util | Runtime, timeouts, `join_all` fan-out |
| sqlx 0.8 (sqlite, runtime-tokio, migrate, time) | DB, no macros (no DATABASE_URL needed at compile time) |
| argon2 0.5 + password-hash 0.5 | argon2id hashing/verification |
| askama 0.12 | Compile-time templates |
| actix-files | `/static` serving |
| serde, serde_json | JSON parsing of `dokku *:report --format json`, session payloads |
| tracing, tracing-subscriber (env-filter) | Structured logs, `#[instrument]` on handlers |
| thiserror | Error enums |
| rand 0.8 / getrandom | Session ids, CSRF tokens (128+ bits, base64url) |
| time 0.3 | Timestamps (UTC unix epoch in DB) |
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
    │   ├── snapshot.rs        # Snapshot + SnapshotStore + background refresher
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
| `/apps/{name}/logs` | GET | yes | → app/logs.html | `?lines=N` (200 default, 1000 max, min 10) |
| `/apps/{name}/delete` | GET | yes | → app/delete.html | confirmation page |
| `/apps/{name}/delete` | POST | yes | `apps::delete` | confirm form input must echo app name |
| `/apps/{name}/start` | POST | yes | `apps::action` | flash success/error |
| `/apps/{name}/stop` | POST | yes | `apps::action` | |
| `/apps/{name}/restart` | POST | yes | `apps::action` | |
| `/static/*` | GET | no | actix-files | immutable cache headers, content-hash names |
| `404/500` | — | — | error.html | via `AppError: ResponseError` |

All POSTs are `CsrfForm<T>` + auth-gated. No-JS-safe: every action is a plain form + server redirect; confirmations are pages, not JS dialogs.

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
- Collect stdout/stderr, read exit status, 30s timeout via `tokio::time::timeout` (`COMMAND_TIMEOUT_SECS`).
- Host key policy: accept-on-first-use into `known_hosts`-style file at `DOKKU_SSH_HOST_KEYS_PATH`; optional pre-pinned file works read-only.
- Errors: `DokkuError::Connect | Timeout | Exit { code, stderr }` — `Exit` messages surface stderr for the user flash; `Connect/Timeout` render 503-style error page.
- A background refresher (`SnapshotStore` + `spawn_refresher`) keeps an in-memory, parsed snapshot of the whole host warm (app list + `ps:report` + `apps:report`, fetched concurrently with 4 permits over the persistent SSH session). Every page read then serves from the snapshot (`Arc` clone, no IO); the first request after boot falls back to one synchronous refresh. After start/stop/restart the affected app is re-fetched synchronously before redirecting; create/destroy trigger a full refresh. A miss (app not listed) triggers a live `apps:list` fallback before 404ing, covering apps created via the CLI within the staleness window. Refresh failures keep the last good snapshot and log a warning. dokku boots its plugin system per command (~750ms), so this removes SSH latency from every request. A single multi-app `ps:report` invocation was tried and rejected: dokku 0.38 only reports the first app argument.

## 9. Templates & UI

- `base.html`: fixed sidebar (Dashboard; App list; per-app nav: Overview, Config, Logs) + topbar (user email, logout) + flash partial (success/error banners).
- Pages listed in §7. Askama derive structs (`#[derive(Template)] #[template(path = "app/show.html")]`) hold plain domain types — templates can't do logic beyond simple display, which keeps HTML pure.
- Tailwind: `assets/input.css` → `static/css/app.css` (watch in dev via css service, minified in prod build). Content scan covers `templates/**` and `src/**/*.rs` (badge classes built in Rust).
- Zero JavaScript in v1. Dark palette, clean tables, status badges (green/red/gray) — Dokku Pro-like.
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
| `COMMAND_TIMEOUT_SECS` | 30 | — | SSH exec timeout |
| `SNAPSHOT_REFRESH_SECS` | 15 | — | background snapshot refresh interval (positive) |
| `COOKIE_SECURE` | false | true | Secure flag |
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
| sqlite contention | WAL + busy_timeout; single-instance assumption documented |
| Tailwind standalone binary availability | Pinned version; verify arm64 + x64 in scaffold milestone |

## 15. Explicit defaults (change on request)

- Password minimum length **12**; argon2id default params; no complexity theater.
- Config page shows **masked** values only in v1 (unmask/edit is post-v1 with re-auth).
- Session TTL **7 days** rolling; host key **accept-on-first-use** with optional pinned file.
- Logs default **200** lines, max **1000**, newest-last (dokku order).
- Single instance, no horizontal scaling; UTC everywhere.
- No login rate-limiting in v1 (documented follow-up).

## 16. Post-v1 backlog (context, not scope)

Config set/unset UI (re-auth to unmask), live log tailing (websockets/SSE), deployments & build logs, one-off `run` commands, user management/roles UI, audit log, login rate limiting, Let's Encrypt/certs UI, ssh-keys management, plugin screens, backup/export, i18n, themes.

---

**Definition of done (v1)**: every route behind auth (except healthz/login/setup/static); CSRF on all POSTs; deployable via `git push dokku main` with healthcheck; `make coverage` ≥ 90% lines; `cargo clippy -D warnings` and `cargo fmt --check` clean; all dokku parsing covered by golden fixtures validated against the target host.
