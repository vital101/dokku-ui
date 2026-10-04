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
- Integration helpers live in `tests/common/mod.rs`: `test_state*()` (MockClient + temp SQLite), `login()`, `complete_setup()`, `extract_csrf()`.
- `tests/fixtures/` are golden outputs captured from the real host (dokku 0.38.4). Service DSNs are redacted (`XXXXXX`) and a test asserts DSNs never surface in parsed data — keep it that way.

## Architecture essentials

- Single crate, lib/bin split: `src/main.rs` is a thin init; everything testable sits behind `dokku_ui::run()` in `src/lib.rs`.
- Functional core, imperative shell: `src/domain/` is pure (no IO) — newtypes (`AppName`, `Email`, `Password`), `DokkuCommand::argv()`, Dokku output parsers. IO lives behind traits: `DokkuClient` in `src/dokku/` with a fixture-backed `MockClient` for tests and `RusshClient` (the only russh-dependent file) in prod; SQLite via sqlx in `src/storage/` (runtime queries, no macros, migrations embedded from `migrations/`).
- Routes are registered in one place: `src/web/mod.rs`. Auth middleware allowlist: `src/web/auth_middleware.rs`. Every POST handler takes `CsrfForm<T>` — don't bypass the CSRF extractor.
- Service (postgres/mysql/redis/mongo) and storage/volume pages reuse the app-tab pattern: instant shell + HTMX partials + the streamed run modal. Handlers: `src/web/services.rs`, `src/web/volumes.rs`; shared run/fragment plumbing: `src/web/fragments.rs`, `src/web/runs.rs`.
- Askama compile-time templates — template errors surface at compile time, not at request time.

## Dokku-integration invariants (don't "fix" these)

- Target host runs dokku 0.38.4, which does NOT support `--format json` for `ps:scale` or `<plugin>:info` (current dokku.com docs describe a newer release) — both are parsed from plain text. Parsers, argv (`src/domain/command.rs`), and fixtures must stay in lockstep.
- The installed service-plugin generation prints `<plugin>:list` as a `=====> <Name> services` banner plus bare service names (no status/version columns), and `<plugin>:logs` takes the follow flag as `$2` and the tail count as `$3` — `ServiceLogs` therefore carries an explicit empty-string argv element, and `russh_client::shell_command` must keep quoting argv (never revert to bare `join(" ")`) for that empty slot to survive SSH.
- `storage:report` with no app argument reports every app in one command; on 0.38.4 it exposes only build/deploy/run mount keys (the dotted `attachment.N.*` keys are newer), so the volumes page parses that plain text.
- There is no native stats command. Service stats run a **fixed read-only script** via `<plugin>:enter <svc> sh -c …` (the plugin prints a `-----> Filesystem changes…` banner that parsers must skip; requires a running container; cgroup v2 only — missing values degrade to `—`). Volume usage runs via `storage:exec <entry> -- sh -c …` (`docker run --rm -v <host_path>:/data alpine:3`; entry names map from `storage:list-entries --format json`; the first use pulls `alpine:3` once). Scripts live in `src/domain/command.rs` and must never interpolate user input. Because dokku's SSH wrapper re-splits `$SSH_ORIGINAL_COMMAND` with `xargs -n 1` + `readarray`, every argv payload must be single-line and single-quote-free — xargs doesn't understand shell-style `'\''` escapes, and multi-line arguments get split by the per-token `echo` (a real host error was exit 2 `xargs: unmatched single quote … sh: -c: option requires an argument`).
- One generic SSE route `/actions/runs/{id}/events` serves every action run (app, service, volume); it is login-gated, not scoped per subject.
- Mutating actions (start/stop/restart/rebuild/scale/destroy) intentionally have NO timeout so long builds stream to completion; read commands use `COMMAND_TIMEOUT_SECS` (default 30s).
- One persistent SSH connection, one channel per command. A fresh connection per command trips the host's SSH rate limit; russh tests assert the connection count stays at 1.
- HTMX fragments always return HTTP 200 with a retry card on failure (see `error_fragment` in `src/web/apps.rs`) — htmx doesn't swap 4xx/5xx responses, so error statuses would leave skeletons spinning forever.
- Strict CSP, no inline JS/CSS: page JS is `static/js/actions.js`, htmx is vendored at `static/js/htmx.min.js`, and htmx's auto-injected `<style>` tag is disabled via a meta config. Don't add inline scripts or styles.

## Conventions

- Rust 2024 edition, MSRV 1.85. `thiserror` enums; `anyhow` never crosses the web boundary; no `unwrap`/`expect` outside `main` and tests.
- Tailwind v4 standalone CLI (no `tailwind.config.js`); `@source` directives in `assets/input.css` scan `templates/` and `src/`, so Tailwind classes built as Rust strings are picked up.
- TDD is the working style; definition of done = tests green, `make lint` clean, `make coverage-gate` ≥ 90%.
- Dev state lives in gitignored `dev-data/` (SQLite DB, SSH key, known_hosts); settings default to it (`src/settings.rs`). `SECRET_KEY` default is a committed dev value — never reuse it for prod.

## Deploy

- Prod deploys via `git push dokku main` (the `dokku` git remote). The app must have `/app/data` storage-mounted (SQLite + SSH keys); healthcheck is `/healthz`; full runbook in `PLAN.md` §11.
- If `PLAN.md` and the `Makefile` disagree, the Makefile wins.
