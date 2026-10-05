# AGENTS.md

Server-rendered Rust (actix-web 4) management UI for Dokku. No SPA, no Node, no docker.sock — Dokku is driven over SSH via russh. Design rationale and deploy runbook: `PLAN.md`. Fixture provenance: `tests/fixtures/README.md`.

## Commands — everything runs in Docker, not on the host

The dev image (`Dockerfile.dev`) carries rustfmt/clippy/llvm-tools/cargo-llvm-cov/bacon and the Tailwind standalone binary; the host has none of these. Always go through the Makefile:

- `make up` — dev server on http://localhost:8080 + Tailwind watch
- `make test` — Tailwind build + `cargo test` (in the container)
- `make lint` — `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings`; must be clean
- `make fmt`, `make css`, `make bash`, `make prod-build`, `make coverage`, `make coverage-gate`

Why not bare `cargo test`: `static/css/app.css` is a generated, gitignored artifact and several tests request `/static/css/app.css`. On a fresh checkout tests fail until the CSS is built — hence `make test` runs tailwindcss first. If you must run cargo directly: `docker compose run --rm web cargo test <name-filter>` after the CSS exists.

- Coverage runs must stay in the Linux container — `cargo llvm-cov` is known-flaky on macOS. Gate: ≥90% lines, ignoring `src/main.rs|src/dokku/russh_client.rs` (override with `IGNORE_FOR_COVERAGE`).
- `make bash` then `bacon test` gives the TDD watch loop.

## Testing quirks

- `tests/russh_integration.rs` is self-contained: it spins up an in-process fake SSH server, so it's part of normal `make test`; no live host needed.
- `tests/dokku_smoke.rs` is `#[ignore]`d — requires env pointing at the live Dokku host (`DOKKU_HOST`, `DOKKU_SSH_KEY_PATH`, `DOKKU_SSH_USER`). Only run with `cargo test -- --ignored`, deliberately against the host.
- Integration helpers live in `tests/common/mod.rs`: `test_state*()` (MockClient + temp SQLite), `login()`, `complete_setup()`, `extract_csrf()`. Run-fragment bodies carry a random-token SSE URL — extract it with `run_url()`; never hardcode `/actions/runs/1/events`.
- `tests/horizontal.rs` proves the multi-container topology: two independent `AppState`s (separate `MockClient`s, separate `SnapshotStore`s) sharing one SQLite file — built by `test_state_pair()` / `states_over_shared_db()` in `tests/common/mod.rs`. Any change to run or snapshot storage must keep these green.
- `tests/fixtures/` are golden outputs captured from the real host (dokku 0.38.4). Service DSNs are redacted (`XXXXXX`) and a test asserts DSNs never surface in parsed data — keep it that way.

## Architecture essentials

- Single crate, lib/bin split: `src/main.rs` is a thin init; everything testable sits behind `dokku_ui::run()` in `src/lib.rs`.
- Functional core, imperative shell: `src/domain/` is pure (no IO) — newtypes (`AppName`, `Email`, `Password`), `DokkuCommand::argv()`, Dokku output parsers. IO lives behind traits: `DokkuClient` in `src/dokku/` with a fixture-backed `MockClient` for tests and `RusshClient` (the only russh-dependent file) in prod; SQLite via sqlx in `src/storage/` (runtime queries, no macros, migrations embedded from `migrations/`).
- Routes are registered in one place: `src/web/mod.rs`. Auth middleware allowlist: `src/web/auth_middleware.rs`. Every POST handler takes `CsrfForm<T>` — don't bypass the CSRF extractor.
- Service (postgres/mysql/redis/mongo) and storage/volume pages reuse the app-tab pattern: instant shell + HTMX partials + the streamed run modal. Handlers: `src/web/services.rs`, `src/web/volumes.rs`; shared run/fragment plumbing: `src/web/fragments.rs`, `src/web/runs.rs`.
- **Multi-container safe by design**: every piece of request-path state lives in SQLite, shared by all containers through the same host mount. Action runs persist to `action_runs`/`action_run_lines` (`src/storage/runs.rs`); run ids are random 64-hex tokens (`auth::csrf::generate_token`), and the SSE route (`src/web/runs.rs`) polls the DB with a 25→400ms backoff instead of a tokio watch, so any process can stream any run. The dashboard snapshot is published to a singleton `snapshots` row (`src/storage/snapshots.rs` + `src/dokku/snapshot.rs`); per-app patches run under a short `BEGIN IMMEDIATE` read-modify-write so concurrent patches from different processes never lose an update, a full refresh yields to a patch that lands while its SSH build is running (the row carries a millisecond `updated_at` marker), and a cold-starting container serves the row with zero SSH. Finished runs are pruned 300s after finishing; runs whose owner died are swept to a failed outcome after 600s without output or a heartbeat — the run task heartbeats every 60s, so a silent long build is never mistaken for dead (`RETAIN_FINISHED_SECS`/`ORPHAN_AFTER_SECS`). `ps:scale web=N` is supported — see `tests/horizontal.rs`.
- Askama compile-time templates — template errors surface at compile time, not at request time.

## Dokku-integration invariants (don't "fix" these)

- Target host runs dokku 0.38.4, which does NOT support `--format json` for `ps:scale` or `<plugin>:info` (current dokku.com docs describe a newer release) — both are parsed from plain text. Parsers, argv (`src/domain/command.rs`), and fixtures must stay in lockstep.
- The installed service-plugin generation prints `<plugin>:list` as a `=====> <Name> services` banner plus bare service names (no status/version columns), and `<plugin>:logs` takes the follow flag as `$2` and the tail count as `$3` — `ServiceLogs` therefore carries an explicit empty-string argv element, and `russh_client::shell_command` must keep quoting argv (never revert to bare `join(" ")`) for that empty slot to survive SSH.
- `storage:report` with no app argument reports every app in one command; on 0.38.4 it exposes only build/deploy/run mount keys (the dotted `attachment.N.*` keys are newer), so the volumes page parses that plain text.
- There is no native stats command. Service stats run a **fixed read-only script** via `<plugin>:enter <svc> sh -c …` (the plugin prints a `-----> Filesystem changes…` banner that parsers must skip; requires a running container; cgroup v2 only — missing values degrade to `—`). Volume usage runs via `storage:exec <entry> -- sh -c …` (`docker run --rm -v <host_path>:/data alpine:3`; entry names map from `storage:list-entries --format json`; the first use pulls `alpine:3` once). Scripts live in `src/domain/command.rs` and must never interpolate user input. Because dokku's SSH wrapper re-splits `$SSH_ORIGINAL_COMMAND` with `xargs -n 1` + `readarray`, every argv payload must be single-line and single-quote-free — xargs doesn't understand shell-style `'\''` escapes, and multi-line arguments get split by the per-token `echo` (a real host error was exit 2 `xargs: unmatched single quote … sh: -c: option requires an argument`).
- One generic SSE route `/actions/runs/{id}/events` serves every action run (app, service, volume); it is login-gated, not scoped per subject. Ids are 64-hex tokens; unknown or malformed ids 404.
- `storage::connect` retries migrations at boot (5 attempts, 0.5s-doubling backoff between them) because every container runs them simultaneously against the shared file on a fresh deploy — exactly one wins the DDL race, the rest pick up its schema.
- Mutating actions (start/stop/restart/rebuild/scale/destroy) intentionally have NO timeout so long builds stream to completion; read commands use `COMMAND_TIMEOUT_SECS` (default 30s).
- One persistent SSH connection, one channel per command. A fresh connection per command trips the host's SSH rate limit; russh tests assert the connection count stays at 1.
- HTMX fragments always return HTTP 200 with a retry card on failure (see `error_fragment` in `src/web/apps.rs`) — htmx doesn't swap 4xx/5xx responses, so error statuses would leave skeletons spinning forever.
- Strict CSP, no inline JS/CSS: page JS is `static/js/actions.js`, htmx is vendored at `static/js/htmx.min.js`, and htmx's auto-injected `<style>` tag is disabled via a meta config. Don't add inline scripts or styles.

## Conventions

- Rust 2024 edition, MSRV 1.85. `thiserror` enums; `anyhow` never crosses the web boundary; no `unwrap`/`expect` outside `main` and tests.
- Tailwind v4 standalone CLI (no `tailwind.config.js`); `@source` directives in `assets/input.css` scan `templates/` and `src/`, so Tailwind classes built as Rust strings are picked up.
- TDD is the working style; definition of done = tests green, `make lint` clean, `make coverage-gate` ≥ 90%.
- Dev state lives in gitignored `dev-data/` (SQLite DB, SSH key, known_hosts); settings default to it (`src/settings.rs`). `SECRET_KEY` default is a committed dev value — never reuse it for prod.

## P0 foundations (landed — invariants to keep)

- **Every mutation is a persisted job** (`action_jobs`, migration `0006`):
  handlers build validated `JobSpec` plans (`src/domain/job.rs`) and enqueue via
  `fragments::enqueue_action_run`; an immediate executor claims **its own job
  by id** (`claim_by_id` — never "the oldest", which in a same-second race
  could be another process's and strand both) and verifies `claimed_by`
  ownership before running an already-claimed job; the worker pool
  (`dokku::spawn_worker` in `lib.rs`) reclaims expired leases cross-container.
  Retries: transient `Connect`/`Timeout` only, exponential backoff; `Exit`
  never; destructive specs `max_attempts=1`. Payloads rehydrate through the
  newtype constructors, so tampered rows fail without touching the host.
  `JobPayload.redactions` carries literal secrets (config values, basic-auth
  passwords) that are masked from every persisted run line.
- **P1 app configuration UI**: app tabs Domains, Cron, Build, and Settings
  (rename, deploy lock, maintenance, HTTP basic auth). New commands live in
  `DokkuCommand` with exhaustive `requirement()` gates — `maintenance` and
  `http-auth` are `Plugin`-gated and render an explanatory state when absent.
  Domain names, buildpack URLs, builder properties/values, cron ids, process
  types, and resource values all have pure validators (`domain/domain_name.rs`,
  `domain/build.rs`, `domain/resource.rs`, `domain/cron.rs`) re-checked at job
  rehydration. Plugin/cron/buildpacks `:help` output is fixture-locked; the
  nginx log families share the `nginx:help` probe.
- **Audit**: `action_runs` carries actor/operation/target_kind/parent_run_id
  (migration `0005`); run lines are redacted before persisting
  (`domain::redact`, literal secrets + `KEY:`/URL-credential masking).
  Retention: `ACTIVITY_TTL_SECS` (90d) / `RUN_LOG_TTL_SECS` (7d) settings.
  Activity pages: `/activity` (+`?user=`), per-app, per-service.
- **Toasts**: `GET /actions/toasts` polled from `base.html` every 5s; durable
  per-user acks via `POST /actions/runs/{id}/ack` (migration `0008`).
- **Capabilities** (`domain::capabilities`, migration `0007`): one shared
  probe row (`dokku --version`, `plugin:list`, `<family> --help`); handlers
  gate UI, never fail at command time. `DokkuCommand::requirement()` is
  exhaustive — new variants must declare their gate.
- **Config editing** (`domain::env_file`): `.env` parse/diff/validate. Values
  must be single-line and quote-free (dokku's SSH wrapper re-splits with
  `xargs -n 1`); reject anything else. Reveal/edit are gated by the re-auth
  window (`session["reauth_until"]`, `REAUTH_TTL_SECS`), and revealed
  responses carry `Cache-Control: no-store`.
- **Live logs** (`web::logs::log_stream`, `static/js/logs.js`): one SSE stream
  per viewer (`/apps/{name}/logs/stream?source=logs|nginx:access-logs|nginx:error-logs&tail=1`);
  sources are capability-gated. When the browser disconnects, the stream drops
  the mpsc receiver and `RusshClient`'s `is_closed()` poll aborts the SSH
  channel with `DokkuError::StreamClosed` — **never** tear down the shared
  session for a viewer disconnect (guarded in `run_command`).

## Deploy

- Prod deploys via `git push dokku main` (the `dokku` git remote). The app must have `/app/data` storage-mounted (SQLite + SSH keys); healthcheck is `/healthz`; full runbook in `PLAN.md` §11.
- If `PLAN.md` and the `Makefile` disagree, the Makefile wins.
