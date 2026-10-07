# dokku-ui ↔ Dokku Pro — Gap Analysis

Goal: eventual feature parity with [Dokku Pro](https://pro.dokku.com/) from this
repo's architecture (actix-web, server-rendered, Dokku over SSH via russh, all
request-path state in shared SQLite).

Status legend: **✅ have** · **◐ partial** · **❌ missing** · **—** not a gap.

Sources: repo inventory (`src/web/mod.rs` route table, `src/domain/command.rs`
argv table, `templates/`, `src/domain/parse.rs`, `PLAN.md`) and the Dokku Pro
docs at `https://pro.dokku.com/docs/` (UI, API, activity, teams, users, git,
webhooks, multi-server, reverse-proxy auth, operating).

## Locked decisions

1. **Parity is the destination, not the next task.** This document only
   records the gap; implementation is scoped separately.
2. **Host-side companion plugin / root access is acceptable.** Plugin
   management, the `teams`/`users` property namespaces, and git-over-HTTP are
   therefore reachable via a small host helper rather than permanently
   divergent. No host.sock / docker.sock dependency is introduced into the
   app itself; helpers are invoked over the existing SSH seam.
3. **Target host stays on dokku 0.38.4** (redis 1.42.1, postgres 1.36.4).
   Newer-dokku-only commands are handled with a reusable **capability
   detection framework** (see below) instead of per-feature version checks.
   Features whose commands do not exist on 0.38.4 and cannot be shimmed by the
   host helper are capability-gated (hidden with an explanatory state).
4. **DSN/credential reveal: reversed (resolved).** Pro's service detail page
   reveals connection strings; parity required the old "DSNs never surface"
   invariant to be deliberately broken. `ServiceInfo.dsn` is now parsed and
   masked by default; the re-auth-gated `POST /services/{plugin}/{svc}/dsn/reveal`
   returns the unmasked value with `Cache-Control: no-store` and a
   `service.dsn.reveal` audit entry. Run lines were already URL-credential
   masked.
5. **Multi-user auth/RBAC: own SQLite RBAC.** Roles (`admin`/`operator`/
   `viewer`) live in our `users` table behind a pure `authorize()` check
   (`src/auth/rbac.rs`), with per-app/service grants as the natural extension.
   Dokku's `teams`/`users` namespaces via the companion helper remain a
   documented reconciliation path, not a P0 dependency. Re-auth for sensitive
   actions uses a short-lived session marker (`reauth_until`, `REAUTH_TTL_SECS`
   default 300s; pure `reauth_valid` in `src/auth/reauth.rs`).

## Baseline — what dokku-ui already implements

- **Auth**: first-run setup wizard, login/logout, argon2id, SQLite sessions
  (rolling 7-day TTL), CSRF on every POST, session renewal, safe `next`.
  Single admin; no multi-user, roles, password reset, or SSH-key management.
- **Dashboard**: snapshot-backed app table with health badges and counters,
  snapshot age, manual `POST /refresh`, per-vhost DNS pre-check.
- **Apps**: create/destroy (typed name, `--force`), start/stop/restart/rebuild,
  scale (`ps:scale`, UI cap 100), processes/containers (`ps:inspect`) and
  resource limits (`resource:report`, read-only), config view (fixed-masked
  values), domains view (`domains:report`), last build/image status
  (`builds:report`), linked services, static log viewer (`logs --num`,
  clamped 10–1000).
- **Services** (postgres/mysql/redis/mongo): list/info, create, start/stop/
  restart/destroy, expose/unexpose, link/unlink, static logs, live resource
  stats via fixed scripts (`<plugin>:enter`), DSNs never parsed.
- **Volumes**: all-app `storage:report` mounts, mount/unmount, per-entry disk
  usage (`storage:exec` + `alpine:3`), host disk summary, and named-entry
  mapping via `storage:list-entries --format json`.
- **Action runs**: every mutation is a persisted run (random 64-hex id) with a
  generic login-gated SSE route (`/actions/runs/{id}/events`), shared across
  containers, 300s finished retention, 600s orphan sweep, 60s heartbeat.
- **Platform**: shared SQLite snapshot/run/session state, background refresher,
  horizontal `ps:scale web=N`, `/healthz`, strict CSP (no inline JS/CSS),
  HTMX partials, compile-time Askama templates.

## What Dokku Pro covers

- **UI**: app dashboard; app tabs Overview, Domains, TLS, Environment,
  Processes, Cron, Build configuration, HTTP basic auth, Logs, Activity,
  Settings; services list/detail with advanced create, live logs, Activity;
  Plugins screen; Teams screen; Users screen; light/dark toggle; queued-job
  toasts; server switcher (⌘K).
- **Deploy paths**: Git HTTP server with per-app push URLs; GitHub webhooks
  (HMAC-signed receiver, per-app repository/branch/build-mode/secret);
  Sync from git (`git:sync`); Deploy from image; Deploy from archive.
- **Platform**: REST JSON:API under `/@api/` with JWT auth, Swagger UI +
  OpenAPI, atomic `/operations` batches with command coalescing and
  collection replacement (app-scoped endpoints cover env vars, domains,
  ports, formations, buildpacks, resources, certificates, letsencrypt,
  http auth, maintenance mode, the webhook config, and builder/scheduler
  settings); per-app port management; durable Activity/Job logs (90-day
  activity TTL, 7-day log TTL, secret redaction, lineage,
  per-app/service/user views, live-tailing over WebSocket); background job
  queue with retries.
- **Identity**: multi-user accounts with passwords, reset tokens/links, SSH
  keys; teams (owners/members) with command/app/service grants; internal
  per-app/per-service teams; reverse-proxy header authentication with trusted
  proxy CIDRs and auto-registration; multi-server (peer list, CORS, custom
  servers).
- **Ops**: `/etc/default/dokku-pro` configuration (JWT secrets, `ROOT_TOKEN`,
  TTLs, public URL, CORS, timeouts, login banner, app filter, license),
  systemd service, documented backup/restore and plugin guidance.

## Gap matrix

### Apps, lifecycle, configuration

| Capability | dokku-ui | Dokku Pro | Notes |
|---|---|---|---|
| Create / destroy / start / stop / restart / rebuild | ✅ | ✅ | Ours streams via SSE; mutating actions have no timeout |
| Scale processes | ✅ | ✅ | Ours capped at 100; Pro also edits resources |
| Rename app | ✅ | ✅ | `apps:rename`; Pro's optional skip-rebuild needs a newer dokku than 0.38.4 — capability-gated |
| Deploy lock (`apps:lock`/`unlock`) | ✅ | ✅ | P1; real UI state |
| Maintenance mode | ✅ | ✅ | `maintenance` plugin; `Plugin`-gated explanatory state when absent |
| Env var view | ✅ | ✅ reveal/copy | Masked by default; reveal under re-auth with `no-store` (P0) |
| Env var edit (batched, `.env`, unset, commit bar) | ✅ | ✅ | `config:set/unset` batch as one queued run (P0) |
| Domains view | ✅ | ✅ | Ours includes a DNS pre-check Pro hints at |
| Domains add/remove/set | ✅ | ✅ | `domains:add/remove/set` (P1) |
| Resource limit/reserve/CPU editing | ✅ | ✅ | `resource:limit/reserve` + clears (P1); rebuild applies |
| Build configuration (buildpacks order, builder, build dir, scheduler and scheduler props) | ✅ buildpacks + builder + scheduler | ✅ | Buildpacks order + builder/build-dir + scheduler selection landed (P1/P4); per-app scheduler *properties* (docker-local init process, parallel count; k3s namespaces) do not exist on 0.38.4 — the Build tab says so |
| Cron tab (list, run-now, suspend/resume individual & app-wide) | ✅ | ✅ | `cron:list --format json` + run-now + per-task suspend/resume (P1); app-wide suspend/resume fans out one command per task in a single queued run (0.38.4 has no app-wide command) |
| HTTP basic auth (users + allowed IPs) | ✅ | ✅ | `http-auth` plugin, `Plugin`-gated; enable/disable/add-user/remove-user + allowed-IP bypasses (`add/remove-allowed-ip`, `set-allowed-ips`, blank clears) landed; passwords redacted from run lines |
| Multi-user accounts + roles | ✅ | ✅ | Own SQLite RBAC: `admin`/`operator`/`viewer`, enforced per request in the auth middleware; Users screen (P3 increment) |
| Self-service password change | ✅ | ✅ | Current-password verification + policy |
| Password reset links | ✅ | ✅ | Admin-generated one-time link (24h, single-use) built from the instance public URL; only the SHA-256 hash is stored, link pages are `no-store`, and both a reset and a self-service change revoke the user's other sessions |
| Queued-job toasts / "deleting" states | ✅ | ✅ | Durable per-user toasts (P0) + dashboard "deleting…" badge from in-flight destroy runs |
| Advanced service create (image/version, env, extra args) | ✅ | ✅ | 1.x plugin flags `--image`/`--image-version`/`--custom-env`/`--config-options`; env values redacted |
| Instance settings (public URL, login banner, app filter, TTLs) | ✅ | ✅ | Admin `/settings` page persisted in shared SQLite; TTL overrides applied at prune time |

### TLS, proxy, logs, builds, activity

| Capability | dokku-ui | Dokku Pro | Notes |
|---|---|---|---|
| Let's Encrypt (email, staging/prod, status/expiry, disable) | ✅ | ✅ | Status/expiry, enable/disable/revoke/cleanup, server-wide auto-renew, and `letsencrypt:set` email/staging on the TLS tab |
| Manual certificate upload / activate | ❌ | ✅ | Deferred: PEM cannot survive the SSH argv re-split; design note in `PLAN.md` §20 covers a stdin-tarball client capability |
| Server-wide auto-renew cron toggle | ✅ | ✅ | `letsencrypt:cron-job --add/--remove`; no status query on plugin 0.20.4, so the card is action-only |
| k3s cert-manager integration | ❌ | ✅ | Environment-specific; divergent unless host runs k3s |
| Proxy/port management (`ports:add/set/remove/report`, proxy status) | ✅ | ✅ | Ports tab with `ports:report/add/set/remove/clear` + read-only `proxy:report` status; native commands on 0.38.4 (source-verified), no companion helper needed |
| Live log tail + pause/follow | ✅ | ✅ | Pro uses WebSocket; our SSE streams over SSH |
| Log source selector (app / nginx access / nginx error) | ✅ | ✅ | Capability-gated per source |
| Process filtering of application logs | ✅ | ✅ | `logs --ps <process>` via the live panel and bounded viewer |
| Last build/image status | ✅ | ✅ | `builds:report` |
| Build logs + deploy history | ✅ | ✅ | Activity records operations; failed deploy logs card + a per-app deploy-history table fed from `action_runs` (`git.*` runs) on the Deploy tab |
| Per-service live logs | ✅ | ✅ | `GET /services/{plugin}/{svc}/logs/stream` SSE (`--tail` as the follow flag); the bounded viewer still re-tails client-side |
| App live CPU/memory | ❌ | not documented | Not a Pro gap; no native dokku stats command |
| Activity/audit tab (app / service / user) | ✅ | ✅ | `/activity`, per-app, per-service; actor attribution |
| Durable audit, secret redaction, lineage, retention | ✅ | ✅ | `action_runs` + `action_run_lines`; TTL settings; `JobPayload.redactions` |
| Background job queue + retries | ✅ | ✅ | Persisted `action_jobs`; transient retries; cross-container lease reclaim |

### Services, data, plugins, storage

| Capability | dokku-ui | Dokku Pro | Notes |
|---|---|---|---|
| Service lifecycle + link/unlink | ✅ | ✅ | Destroy is blocked while apps are linked at enqueue time **and** re-checked in the executor (fail-closed when the check errors) |
| Service create advanced options (image/tag, env, extra args) | ✅ | ✅ | `--image`/`--image-version`/`--custom-env`/`--config-options`, source-verified against the installed 1.x plugin generation |
| Connection string / credential reveal | ✅ | ✅ | Policy reversed (P4): DSN parsed, masked by default, re-auth-gated reveal with `no-store` + audit; never in run lines |
| Service Activity tab | ✅ | ✅ | `/services/{plugin}/{service}/activity` |
| Datastore plugin coverage | ✅ 12 | 18 official plugins | Full dokku-org set (postgres/mysql/redis/mongo/mariadb/memcached/rabbitmq/clickhouse/couchdb/elasticsearch/nats/solr); sidebar is capability-driven. Pro also names community plugins (influxdb, neo4j, rsync, vault) — catalog additions when needed |
| Expose/unexpose service ports | ✅ | not documented | We exceed the documented Pro surface |
| Service live stats (mem/CPU/data/disk) | ✅ | not documented | Our fixed read-only scripts; cgroup v2 only |
| Volumes page (mounts, mount/unmount, usage, host disk) | ✅ | not documented | We exceed the documented Pro surface |
| Named storage entries (`storage:create/destroy/info`) | ❌ | ❌ | Not a Pro gap either |
| Plugin management (list/install/enable/disable/update/uninstall) | ❌ | ✅ | Root-only on host; needs companion helper |

### Identity, API, transport, multi-server

| Capability | dokku-ui | Dokku Pro | Notes |
|---|---|---|---|
| Multi-user accounts | ✅ | ✅ | Admin creates users with an initial password; roles admin/operator/viewer |
| Password set / reset token & link | ✅ | ✅ | Self-service change and admin-generated one-time reset links (24h, single-use, public-URL based) |
| SSH key management | ✅ | ✅ | Admin `/keys` screen over `ssh-keys:list/add/remove`; the public key travels on stdin (`DokkuClient::exec_with_stdin`), add/remove are audited as `ssh-key.*` |
| Teams (owners/members) | ❌ | ✅ | Pro ships a `teams` host plugin; own RBAC covers the app-level need |
| Command/app/service scoped grants | ❌ | ✅ | Enforced via host plugin triggers; own RBAC is role-level today |
| Internal per-app / per-service teams | ❌ | ✅ | Auto-managed `dokku@app--…`/`dokku@service--…` |
| Per-user activity audit | ✅ | ✅ | `/activity?user=`, actor attribution on every run |
| Reverse-proxy authentication (trusted header + CIDR, auto-registration) | ✅ | ✅ | Env-only: `TRUSTED_PROXY_CIDRS`, `PROXY_AUTH_HEADER`, `PROXY_AUTH_DEFAULT_ROLE`; header honored only for trusted peer addresses; auto-registered users cannot password-login |
| REST JSON:API + JWT access/refresh tokens | ❌ | ✅ | |
| Swagger UI + OpenAPI spec | ❌ | ✅ | |
| Atomic batch operations (`/operations`, coalescing, collection replacement) | ❌ | ✅ | Coalescing is the complex half |
| Git HTTP server + push URL on app page | ◐ push URL | ✅ | SSH push URL card on the Deploy tab; HTTP server needs the companion helper (spike in PLAN.md §20) |
| GitHub webhooks (HMAC receiver, per-app config) | ✅ | ✅ | Public `/webhooks/github/{app}`, HMAC-SHA256, per-app repo/branch/build-mode config; secret reveal under re-auth |
| Deploy from git / image / archive | ✅ | ✅ | `git:sync`, `git:from-image`, `git:from-archive` on the Deploy tab |
| Multi-server (peer list, CORS, custom servers, selector) | ❌ | ✅ | Our "multi" is containers of one server |
| Theme light/dark, command palette, login banner, app filter | ✅ | ✅ | Light/dark variable flip, ⌘K palette (`/palette.json`), banner + app filter in instance settings |
| Ops config parity (JWT secrets, root token, TTLs, public URL, CORS, timeouts) | ◐ | ✅ | Public URL + activity/run-log TTLs are admin-editable; JWT/root-token/CORS remain N/A |
| `/healthz` healthcheck | ✅ | not documented | Our deploy requirement |
| Horizontal multi-container scaling | ✅ | not documented | Horizontal test suite guards it |

## Constraints and risks

1. **Dokku version skew.** Pro requires dokku 0.38.26+ and newer datastore
   plugins; the target host is 0.38.4. Commands such as
   `config:show --revealed`, `ports:set`, `ps:scale --skip-deploy`,
   `apps:rename --skip-deploy`, and parts of `git:*` may simply not exist.
   The capability framework must gate these, and each newly used command needs
   verification against the host before it is wired in.
2. **Root vs SSH.** Pro runs as a root host binary and writes
   `/var/lib/dokku/plugins` and the `teams`/`users` property namespaces. We
   connect as the dokku SSH user. A small companion helper (installed and
   invoked over SSH) is the agreed bridge for root-only operations.
3. **Git HTTP server.** Pro implements git smart-HTTP itself. With SSH-only
   access this needs a host-side helper (e.g. a shim around
   `git-http-backend`/the dokku git receiver) plus auth. Spike before
   scheduling.
4. **DSN policy.** The fixture/test invariant "DSNs never surface" must be
   explicitly reversed (or consciously broken) before service credential
   reveal can ship.
5. **Closed-source moving target.** Dokku Pro is commercial and evolving
   (1.5.0 at time of writing); parity means independent reimplementation, and
   the surface will keep moving.
6. **Multi-container safety.** Any new request-path state introduced for
   parity (job queue, capability cache, audit store) lives in shared SQLite
   and must keep `tests/horizontal.rs` green.
7. **Service logs on 0.38.4.** The installed plugin generation ignores the
   tail count and prints a filesystem-banner; parsers must keep skipping it.

## Closure paths under the locked decisions

| Gap area | Path | Gate |
|---|---|---|
| Env edit, domains edit, rename, lock/unlock, maintenance | Direct dokku commands | Capability-gated |
| Resource limit/reserve/CPU editing | `resource:limit/reserve` | Capability-gated |
| Cron tab, build config, HTTP basic auth | Direct / plugin commands | Capability + plugin detection |
| TLS: letsencrypt, manual certs, renew toggle | Direct if plugins installed | Plugin detection, explanatory fallback |
| Deploy from git/image/archive, push URL | Direct commands | Capability-gated |
| Live app/service logs, source selector | `logs --tail` + nginx logs streamed over SSH into SSE | Capability-gated |
| Durable activity/audit, redaction, retention | Extend `action_runs` storage; no host dependency | — |
| Background job queue + retries + toasts | Our own persisted queue | — |
| Multi-user, password reset, SSH keys | Dokku `users`/`ssh-keys` + our sessions | Design decision: own RBAC vs host `teams` |
| Teams/RBAC, internal teams | Host `teams` plugin via companion | Companion helper |
| Plugin management | Host helper (root) | Companion helper |
| Git HTTP server | Host helper, spike first | Companion helper |
| GitHub webhooks | Our own public receiver + HMAC + secret store | Public URL available |
| REST API + JWT + Swagger + batch ops | Our own JSON:API layer | Multi-user/token model |
| Multi-server, reverse-proxy SSO | Our own peer config/CORS; trusted-header auth | Token model; env-only SSO |
| Credential/DSN reveal | Policy reversal + parsers | **Open decision** |
| Datastore coverage 4 → 18 | Generic pages + parsers + fixtures | Plugin detection |
| k3s cert-manager / scheduler props | Environment-specific | Divergent unless host runs k3s |

## Capability detection framework (implemented — see P0 status)

One reusable mechanism so we stop hand-rolling version checks:

- A pure `Capabilities` value in `src/domain/` holding the dokku version,
  enabled plugin inventory, and per-command-family support.
- A single metadata table binding each `DokkuCommand` variant to its
  requirements (`min_version`, `requires_plugin`), so support is declared once
  next to the argv definition.
- A read-only probe behind the existing `DokkuClient` seam (`dokku version`,
  `plugin:list`, targeted `<family>:help` checks — the nginx log families
  share the `nginx:help` probe), with output parsed and fixture-tested like
  every other parser.
- The probe result cached in the shared SQLite snapshot (carrying the dokku
  version and `updated_at`) so all containers share one answer and cold starts
  cost zero SSH; refreshed with the same lifecycle as the app snapshot.
- Handlers consult capabilities to register tabs/forms or render a
  "requires dokku ≥ X / plugin Y" state, never to fail at command time.
  HTMX error fragments stay HTTP 200 as usual.

## True divergences (keep documenting, don't chase)

- k3s scheduler features (cert-manager TLS, scheduler-k3s properties) unless
  the target host moves to the k3s scheduler.
- Pro's license service, JWT-secret/`/etc/default` operational surface, and
  systemd packaging — we run as a container via `git push dokku main`.
- Pro is closed-source; every parity item is an independent implementation.
- We intentionally exceed Pro (documented surface) on: volumes UI, service
  stats, expose/unexpose, vhost DNS pre-check, `/healthz`, horizontal scaling.
- DSN reveal now matches Pro (masked by default, re-auth-gated reveal); the
  fixtures keep `XXXXXX` redactions.

## Suggested sequencing (when implementation starts)

- **P0 — Foundations**: durable activity/audit store (actor, operation,
  target, lineage, redaction, configurable TTL) on top of `action_runs`;
  background job queue + retries + toasts; capability detection framework;
  multi-user auth/RBAC design decision; config edit with reveal under re-auth;
  live log streaming.

## P0 status (implemented)

All P0 workstreams are landed and green (`make test`, `make lint`,
`make coverage-gate`):

- **Capability detection** (`src/domain/capabilities.rs`, `src/dokku/capabilities.rs`,
  migration `0007`): `dokku version` + `plugin:list` + `<cmd>:help` probes
  published to a shared SQLite row; `DokkuCommand::requirement()` declares each
  command's gate; the logs page shows a live-tail badge straight from the row.
- **Durable audit** (migration `0005`): `action_runs` carries actor, operation,
  target kind, and parent run id; every handler routes through audit-aware
  `RunRequest`s; no-JS create paths record via `run_synchronously`. Lines are
  redacted before persisting (`src/domain/redact.rs`). Retention is
  configurable (`ACTIVITY_TTL_SECS` 90d / `RUN_LOG_TTL_SECS` 7d). Activity
  pages: global (`/activity`), per-app, per-service.
- **Job queue + retries** (migration `0006`, `src/domain/job.rs`,
  `src/storage/jobs.rs`, `src/dokku/workers.rs`): every mutation enqueues a
  validated `JobSpec` plan; immediate executors claim *their own* job by id
  (same-second races can never strand a job); transient `Connect`/`Timeout`
  errors retry with exponential backoff, `Exit` never, destructive ops once;
  the worker pool reclaims expired leases cross-container.
- **Toasts** (migration `0008`, `src/web/toasts.rs`): a global tray in
  `base.html` polls `/actions/toasts`; completions are dismissed via
  per-user durable acks (`POST /actions/runs/{id}/ack`).
- **RBAC decision + re-auth**: locked in this document (§Locked decisions item
  5); `src/auth/rbac.rs` + `src/auth/reauth.rs`; `users.role` column
  (migration `0009`); `/reauth` page + modal.
- **Config reveal + edit** (`src/domain/env_file.rs`, `src/web/apps.rs`):
  `.env` parse/diff/validate (single-line, quote-free values), reveal under
  re-auth (`no-store`), batch set/unset as one queued run.
- **Live logs** (`src/web/logs.rs`, `static/js/logs.js`): SSE stream for app /
  nginx access / nginx error sources, per-source capability-gated; dropping
  the stream flips the mpsc sender's `is_closed()`, and `RusshClient` closes
  the SSH channel without harming the shared session (fake-SSH-server test
  proves the session survives).

## P1 status (implemented)

- **Settings tab**: rename (`apps:rename`), deploy lock/unlock
  (`apps:lock`/`unlock`), maintenance mode and HTTP basic auth (both
  `Plugin`-gated; the target host does not install `maintenance`/`http-auth`,
  so they render explanatory states). Basic-auth passwords travel in argv and
  are redacted from run lines via `JobPayload.redactions`.
- **Domains tab**: vhost list with per-domain DNS pre-check, add/remove/set
  (`domains:add/remove/set`), `DomainName` validation (wildcards allowed).
- **Resource limits**: per-process-type limit/reserve editing
  (`resource:limit/reserve` + `-clear`) in the Processes tab.
- **Cron tab**: `cron:list --format json`, run-now/suspend/resume
  (`cron:run/suspend/resume`).
- **Build tab**: buildpack order editing (`buildpacks:list/set/add/remove/clear`)
  and builder selection/build-dir (`builder:report/set`).
- **Probe corrections from the live host**: `dokku version` (not `--version`,
  which the SSH wrapper mangles) and `<cmd>:help` (not `<cmd> --help`, which
  some plugins parse as an app name); the nginx log commands are subcommands
  of the `nginx` plugin, so both log families share the `nginx:help` probe.
- **P1 — App configuration UI**: domains edit; batched env editing; rename;
  lock/unlock; maintenance; resource limit editing; cron tab; build config;
  HTTP basic auth.
- **P2 — Deploy paths**: git sync / from-image / from-archive; push URL;
  GitHub webhooks; git HTTP server spike (companion helper).
- **P3 — Platform**: JSON:API + JWT + Swagger + batch operations; plugin
  screens via companion; service advanced create, credential reveal (policy
  permitting), service activity, datastore expansion; multi-server;
  reverse-proxy auth.
- **P4 — Polish/ops**: theming, command palette, login banner, app filter,
  public URL, TTL configuration, "deleting" states.

## P2 status (implemented)

- **Deploy tab**: SSH push URL card, deploy public key card (guidance state —
  the host has no generated deploy key), `git:report` summary with deploy
  branch/commit/source image/last-updated, deploy-branch set/clear
  (`git:set`).
- **Deploy actions**: `git:sync` (build modes, optional ref), `git:from-image`,
  `git:from-archive` as indefinite-timeout jobs; remotes carrying userinfo are
  redacted from run lines; audit operations `git.*`.
- **GitHub webhooks**: migration `0010`, per-app repo/branch/build-mode/secret
  config, public HMAC-SHA256 receiver (`/webhooks/github/{app}`; 256 KiB cap,
  constant-time compare via `ring` + `subtle`, host-aware repo matching),
  system-actor attribution, secret reveal under re-auth (`no-store`).
- **TLS tab**: `letsencrypt:list`/`active` status and expiry/renewal,
  enable/disable/revoke/cleanup, server-wide `letsencrypt:cron-job` toggle
  (all plugin-gated with explanatory fallback), plus a read-only `certs:report`
  certificate card. Manual certificate uploads remain deferred (PEM cannot
  travel as SSH argv; design note in `PLAN.md` §20).
- **Log quick wins**: `logs --ps <process>` filtering on the bounded viewer
  and the SSE live panel (`?process=`, app source only), and a lazy failed
  deploy logs card fed by `logs:failed`.
- **Git HTTP server**: paper spike only (`PLAN.md` §20) — companion-helper
  shape, auth options, constraints; no code until companion infrastructure is
  scheduled.
- **Fixture provenance**: deploy/TLS/logs-failed fixtures captured from the
  live host on 2026-10-06; `logs_failed_populated.txt` is synthetic,
  source-verified against dokku `v0.38.4`.

## P3 status (partially implemented)

- **Multi-user + RBAC** (migration `0009` roles already existed): `UsersRepo`
  gained list/update-role/set-password/delete; the auth middleware now resolves
  the user per request and enforces a pure `permission_for(method, path)` map
  (`src/auth/rbac.rs`) — admin/operator/viewer, with self-service exceptions
  (logout, own password, re-auth, toast acks) and 403s for the rest. Sessions
  for deleted users are purged. `/users` admin screen (create with role, set
  role, delete) with fail-closed role parsing and a pure last-admin guard
  (cannot delete or demote the final admin; cannot self-delete). Self-service
  `/password` verifies the current password. `DokkuCommand` argv/parsers
  unchanged; the UI does not (yet) hide every mutation control for viewers —
  the server-side 403 is authoritative.
- **Advanced service create**: `ServiceCreateOptions` (pure,
  `src/domain/service_create.rs`) with source-verified 1.x plugin flags —
  `--image`, `--image-version`, `--custom-env "K=V;K=V"`,
  `--config-options "--flag value"` — validated for the SSH argv re-split
  (single-line, quote-free) and re-checked at job rehydration. Custom-env
  values join `JobPayload.redactions`.
- **Datastore coverage 4 → 8**: catalog adds `mariadb`, `memcached`,
  `rabbitmq`, `clickhouse` (display names + data dirs); the sidebar and
  `/partials/service-nav` render only plugins present in the capabilities
  `plugin:list` inventory, degrading to the full catalog when the probe has not
  run. Remaining official plugins are catalog additions.
- **Instance settings** (migration `0011`, shared SQLite, admin `/settings`):
  public URL, login banner (rendered on the sign-in page), app filter (globs
  hidden from the dashboard while staying manageable by URL), and
  activity/run-log TTL overrides applied by the run-prune path with the env
  value as fallback.
- **Polish**: light/dark theme via CSS-variable inversion + cookie-backed
  toggle (`static/js/theme.js`, no template sweep); ⌘K command palette
  (`/palette.json` + `static/js/palette.js`, role-gated entries); dashboard
  "deleting…" badge from unfinished destroy runs.
- **Still open in P3**: REST JSON:API + JWT + Swagger + batch operations,
  multi-server, teams/scoped grants, plugin management (companion helper),
  git HTTP server, manual certificate upload (deferred — design note in
  `PLAN.md` §20).
- **P4 closed the remaining SSH-only gaps**: proxy/port management, per-app
  scheduler selection, app-wide cron fan-out, http-auth allowed-IP bypasses,
  service live logs, datastore coverage 8 → 12 (full dokku-org set), viewer UI
  hiding, execution-time destroy precondition, deploy history, SSH keys (with
  a stdin client capability), reverse-proxy header auth, and the DSN reveal
  policy flip. See "P4 status" below.

## P3 follow-up (implemented)

- **Password reset links**: migration `0012_password_resets.sql`; bearers are
  64-hex tokens shown once and stored as SHA-256 hashes, single-use with a
  24-hour expiry, and invalidated when a newer link is generated for the same
  user. Admins generate links from `/users` (requires the instance public
  URL); `/reset/{token}` is a public route using the normal session CSRF flow,
  a successful reset flashes on the sign-in page, link pages are `no-store`,
  and both a reset and a self-service change purge the user's other sessions
  (killing a copied cookie).
- **Destroy-while-linked guard**: `service.destroy` checks `<plugin>:links`
  first and refuses with the linked app list; a failed link check fails closed
  (never destroy when links are unknown). The check runs when the job is
  enqueued **and** again in the executor (`workers::precondition`), closing the
  millisecond TOCTOU window; links unknown ⇒ the job fails closed.
- **`letsencrypt:set`**: email and staging are editable from the TLS tab as
  audited runs (`letsencrypt:set <app> <property> [value]`), re-validated at
  job rehydration; blank fields are left untouched. The email must also be
  free of single quotes because it travels as SSH argv (the `xargs` re-split
  cannot carry them).

## P4 status (implemented)

All remaining SSH-only gaps plus the two open policy decisions landed:

- **Ports tab** (`src/web/ports.rs`, migration-free): `ports:report/add/set/remove/clear`
  and read-only `proxy:report`; mappings are pure-validated
  (`src/domain/port.rs`) and re-validated at job rehydration. Commands and
  report formats source-verified against dokku `v0.38.4`; fixtures are
  synthetic/source-verified until the next host capture.
- **Scheduler selection** (Build tab): `scheduler:report/set <app> selected`
  only — 0.38.4's `scheduler:set` has no per-scheduler property namespaces
  (`plugins/scheduler/scheduler.go`), so the per-scheduler property gap is
  documented as a version divergence rather than chased.
- **App-wide cron toggles**: `cron:suspend/resume` need a task id on 0.38.4,
  so the UI's "Suspend all"/"Resume all" fan out one command per matching task
  in a single queued run.
- **http-auth allowed IPs**: `add-allowed-ip`, `remove-allowed-ip`,
  `set-allowed-ips` (blank clears); `is_valid_allowed_ip` shape-validates
  nginx-style addresses (IPv4/IPv6 CIDR, `all`, `unix:`) and the plugin
  re-validates at command time. Plugin remains absent on the target host →
  capability-gated explanatory state.
- **Per-service live logs**: `/services/{plugin}/{svc}/logs/stream` SSE,
  reusing the app-stream plumbing; the follow flag rides the plugin's `$2`
  (`--tail`) with the tail count as `$3`.
- **Datastore coverage 8 → 12**: adds couchdb, elasticsearch, nats, solr —
  the full dokku-org datastore set; data dirs follow the official images
  (nats keeps none, like memcached).
- **Viewer UI hiding completed**: `can_manage` flows through every
  mutation-bearing page/partial; create pages redirect; the middleware 403 is
  unchanged and authoritative. The palette hides `New app` from viewers.
- **Destroy execution-time precondition** (`workers::precondition`): the
  `<plugin>:links` check is repeated at claim time and fails closed.
- **Deploy history**: the Deploy tab lists recent `git.sync`/`git.from-image`/
  `git.from-archive` runs (actor + outcome) from `action_runs`, with a link to
  the app's activity page.
- **SSH keys + stdin client capability**: `DokkuClient::exec_with_stdin`
  (russh channel data + EOF; mock records payloads) backs the admin `/keys`
  screen (`ssh-keys:list/add/remove`); `ssh-key.*` runs are audited without
  lines and keys never travel in argv.
- **Reverse-proxy header auth**: env-only `TRUSTED_PROXY_CIDRS` (empty ⇒ off),
  `PROXY_AUTH_HEADER` (default `x-forwarded-user`), and
  `PROXY_AUTH_DEFAULT_ROLE` (default `viewer`); the header is honored only
  when the direct peer address is inside a trusted CIDR, users auto-register
  with the unusable `!proxy-auth` password hash, and the session is renewed
  before the user is bound.
- **DSN reveal**: locked decision 4 reversed — DSNs are parsed and masked by
  default; `POST /services/{plugin}/{svc}/dsn/reveal` is re-auth-gated,
  `no-store`, and audited as `service.dsn.reveal`.

## Refreshing this document

- Re-inventory routes from `src/web/mod.rs` and commands from
  `src/domain/command.rs` first; both are exhaustive and single-source.
- Re-read the Dokku Pro doc nav at `https://pro.dokku.com/docs/` — new
  top-level nav entries are the signal that the parity target moved.
- Update the "Locked decisions" only with an explicit call; everything else
  is descriptive.

Last re-check 2026-10-07: the pro.dokku.com/docs nav is unchanged from the
audit above (`features/commands` documents the host `dokku-pro` binary CLI —
a documented divergence). This review corrected the build-configuration,
cron, and http-auth rows to ◐, added the proxy/ports row, clarified the
rename skip-rebuild note, and enumerated the missing official datastore
plugins.

P4 implementation pass (2026-10-07): all remaining SSH-only rows are ✅; the
two open decisions (DSN reveal, reverse-proxy auth) are resolved as above.
What remains is companion-helper work (plugin management, teams/scoped
grants, git HTTP), the JSON:API layer, multi-server, and manual certificate
upload (stdin-tarball design, `PLAN.md` §20).
