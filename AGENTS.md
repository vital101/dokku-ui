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
- `tests/fixtures/` are golden outputs captured from the real host (dokku 0.38.4). Service DSNs are redacted (`XXXXXX`) in the fixtures; the parser now reads the `Dsn` line into `ServiceInfo`, but the UI masks it by default and only the re-auth-gated `/services/{plugin}/{svc}/dsn/reveal` endpoint (with `Cache-Control: no-store` + audit) emits it unmasked.

## Architecture essentials

- Single crate, lib/bin split: `src/main.rs` is a thin init; everything testable sits behind `dokku_ui::run()` in `src/lib.rs`.
- Functional core, imperative shell: `src/domain/` is pure (no IO) — newtypes (`AppName`, `Email`, `Password`), `DokkuCommand::argv()`, Dokku output parsers. IO lives behind traits: `DokkuClient` in `src/dokku/` with a fixture-backed `MockClient` for tests and `RusshClient` (the only russh-dependent file) in prod; SQLite via sqlx in `src/storage/` (runtime queries, no macros, migrations embedded from `migrations/`).
- Routes are registered in one place: `src/web/mod.rs`. Auth middleware allowlist: `src/web/auth_middleware.rs` (includes the public password-reset prefix `/reset/`). Every POST handler takes `CsrfForm<T>` — don't bypass the CSRF extractor.
- **RBAC is enforced in the auth middleware**, not in handlers: it resolves the session user each request and gates via pure `permission_for(method, path)` (`src/auth/rbac.rs`) — `/users*` and `/settings*` are `ManageUsers`, GETs are `View`, a small self-service set (`/logout`, `/password`, `/reauth`, toast acks) is open to all roles, everything else is `ManageApps`. Unknown roles fail closed to `viewer`; sessions for deleted users are purged. Admin screens: `/users` (create/role/delete/reset-link, last-admin guards in `rbac.rs`, atomic `*_guarded` statements in `storage/users.rs`) and `/settings` (instance settings: public URL, login banner, app-filter globs, activity/run-log TTL overrides applied at run-prune time with env fallback). `/password` is self-service; reset links are admin-generated, hashed, single-use, 24h (`migrations/0012`), and both a reset and a self-service change purge the user's other sessions (`SqliteSessionStore::delete_user_sessions`). Viewer UI hiding is partial (Users/Settings nav is an htmx partial); the 403 is authoritative.
- Service destroy is blocked while apps are linked (`<plugin>:links` check in `services::destroy`; fails closed if the check errors; enqueue-time, so a tiny TOCTOU window remains). `letsencrypt:set` email/staging is a validated, audited run on the TLS tab; the email must pass `is_valid_letsencrypt_email` (no `'` — SSH `xargs` re-split invariant).
- **Service plugin catalog is 12 entries** (`src/domain/service_plugin.rs`: postgres/mysql/redis/mongo + mariadb/memcached/rabbitmq/clickhouse + couchdb/elasticsearch/nats/solr — the full dokku-org datastore set); `/partials/service-nav` filters it through the capabilities `plugin:list` inventory so only installed plugins appear (unknown probe ⇒ full catalog). Advanced create flags (`--image`, `--image-version`, `--custom-env "K=V;K=V"`, `--config-options`) are source-locked to the 1.x plugin generation and re-validated at job rehydration; custom-env values are redacted.
- **Theming is a CSS-variable flip**: `[data-theme="light"]` in `assets/input.css` overrides Tailwind's neutral ramp + status accents; `static/js/theme.js` sets the attribute from a cookie before paint. Templates stay dark-first and use literal `slate-*` utilities. No inline JS/CSS: palette (`/palette.json` + `static/js/palette.js`) and theme toggles are external scripts; palette markup lives in `base.html` so Tailwind's source scan sees its classes.
- Service (postgres/mysql/redis/mongo) and storage/volume pages reuse the app-tab pattern: instant shell + HTMX partials + the streamed run modal. Handlers: `src/web/services.rs`, `src/web/volumes.rs`; shared run/fragment plumbing: `src/web/fragments.rs`, `src/web/runs.rs`.
- Deploy (`src/web/deploy.rs`) and TLS (`src/web/tls.rs`) are app tabs too. Deploy covers the SSH push URL, `git:report` summary, deploy-branch `git:set`, the three deploy jobs (`git:sync`/`git:from-image`/`git:from-archive`), the GitHub webhook card + secret reveal, and a lazy `logs:failed` card. Pure git validators/redaction fragments live in `src/domain/git.rs`; the webhook HMAC/repo/branch predicates in `src/domain/webhook.rs`. TLS renders `letsencrypt:list`/`active`, enable/disable/revoke/cleanup, the server-wide cron toggle, and a `certs:report` card; every letsencrypt command is `Requirement::Plugin("letsencrypt")`-gated so the tab explains itself when the plugin is absent.
- **The GitHub webhook receiver is the only unauthenticated non-auth route** (`POST /webhooks/github/{app}`, allowlisted via the `/webhooks/` prefix in `auth_middleware`). It must keep its raw-body 256 KiB cap, constant-time HMAC-SHA256 check (`ring` direct dep + `subtle`), host-aware repo matching, and 404 for unknown/disabled apps. Deliveries enqueue through `fragments::enqueue_action_run_as` with the `github-webhook` system actor (`user_id: None`), and the webhook secret plus any remote-URL credentials go into `JobPayload.redactions`.
- **Multi-container safe by design**: every piece of request-path state lives in SQLite, shared by all containers through the same host mount. Action runs persist to `action_runs`/`action_run_lines` (`src/storage/runs.rs`); run ids are random 64-hex tokens (`auth::csrf::generate_token`), and the SSE route (`src/web/runs.rs`) polls the DB with a 25→400ms backoff instead of a tokio watch, so any process can stream any run. The dashboard snapshot is published to a singleton `snapshots` row (`src/storage/snapshots.rs` + `src/dokku/snapshot.rs`); per-app patches run under a short `BEGIN IMMEDIATE` read-modify-write so concurrent patches from different processes never lose an update, a full refresh yields to a patch that lands while its SSH build is running (the row carries a millisecond `updated_at` marker), and a cold-starting container serves the row with zero SSH. Finished runs are pruned 300s after finishing; runs whose owner died are swept to a failed outcome after 600s without output or a heartbeat — the run task heartbeats every 60s, so a silent long build is never mistaken for dead (`RETAIN_FINISHED_SECS`/`ORPHAN_AFTER_SECS`). `ps:scale web=N` is supported — see `tests/horizontal.rs`.
- Askama compile-time templates — template errors surface at compile time, not at request time.

## Dokku-integration invariants (don't "fix" these)

- Target host runs dokku 0.38.4, which does NOT support `--format json` for `ps:scale` or `<plugin>:info` (current dokku.com docs describe a newer release) — both are parsed from plain text. Parsers, argv (`src/domain/command.rs`), and fixtures must stay in lockstep.
- `git:report` prints `Git sha: HEAD` for unborn refs and empty values for unset fields; the parser only accepts hash-shaped commits. `git:sync`'s build flags (`--build`/`--build-if-changes`) precede the app name, and `git:set <app> deploy-branch` with no value clears. `git:public-key` exits 1 on a host with no generated deploy key — the UI treats that as a guidance state, not an error.
- `logs:failed <app>` prints just the `=====> <app> failed deploy logs` header when no deploy failed (no `!` warning on 0.38.4); the parser drops the header and `----->` banners but keeps `remote: !` lines. The app log process filter is `logs --ps <process>` and is valid only for the `logs` source.
- `letsencrypt:cron-job` on plugin 0.20.4 takes `--add`/`--remove` with no status query, so the auto-renew card is action-only.
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

## P4 increments (landed — invariants to keep)

- **Ports tab** (`src/web/ports.rs`): `ports:report/add/set/remove/clear` + read-only `proxy:report`, source-verified against dokku `v0.38.4` (`plugins/ports|proxy`). Mappings validate quote/whitespace-free (`src/domain/port.rs`) and re-validate at job rehydration. Fixtures are synthetic/source-verified until a host capture.
- **Scheduler selection** (Build tab): only `selected` (`scheduler:set <app> selected [value]`, blank clears) — 0.38.4 has no per-scheduler property namespaces. Offer `k3s`/`null` only when `plugin:list` reports `scheduler-k3s`/`scheduler-null` installed.
- **App-wide cron toggles** fan out one `CronSuspend`/`CronResume` per matching task (0.38.4 requires a task id; there is no app-wide command).
- **http-auth allowed-IP bypasses** (`add-allowed-ip`/`remove-allowed-ip`/`set-allowed-ips`, no addresses = clear); `is_valid_allowed_ip` shape-validates nginx-style addresses (CIDR, `all`, `unix:`) and rejects quotes/whitespace; the plugin re-validates at command time. The plugin is absent on the target host → capability-gated explanatory state.
- **Service live logs**: `GET /services/{plugin}/{svc}/logs/stream` SSE; the follow flag is `--tail` (`$2`) with the count as `$3`. Dropping the viewer aborts only the channel (same shared-session contract as app logs).
- **Viewer UI hiding**: every mutation-bearing page/partial receives `can_manage`; controls are omitted for viewers. The middleware 403 remains authoritative. `New app`/`New service`/`/apps/new` guard by redirect.
- **Destroy-while-linked** is re-checked in the job executor (`workers::precondition`) and fails closed; links unknown ⇒ nothing runs.
- **Deploy history** is read from `action_runs` filtered to `git.sync`/`git.from-image`/`git.from-archive`.
- **SSH keys** (`/keys`, admin-only via `permission_for`): `DokkuClient::exec_with_stdin` writes the public key to stdin (never argv) for `ssh-keys:add`; list parses the `sshcommand list` text format; add/remove audit as `ssh-key.*` runs with no lines.
- **Reverse-proxy auth** (env-only): `TRUSTED_PROXY_CIDRS` (empty ⇒ off), `PROXY_AUTH_HEADER` (default `x-forwarded-user`), `PROXY_AUTH_DEFAULT_ROLE` (default `viewer`). The header is honored only when `req.peer_addr()` ∈ CIDRs; auto-registered users get the unusable `!proxy-auth` password hash; the session is renewed before binding the user.
- **DSN reveal**: locked decision 4 reversed — `ServiceInfo.dsn` is parsed, masked by default, and revealed only under re-auth with `no-store` plus a `service.dsn.reveal` audit run.

## Deploy

- Prod deploys via `git push dokku main` (the `dokku` git remote). The app must have `/app/data` storage-mounted (SQLite + SSH keys); healthcheck is `/healthz`; full runbook in `PLAN.md` §11.
- If `PLAN.md` and the `Makefile` disagree, the Makefile wins.
