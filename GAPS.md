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
4. **DSN/credential reveal is a policy decision still open.** The current
   codebase deliberately never parses or renders DSNs and a test enforces it;
   Pro's service detail page reveals connection strings. Parity requires
   reversing that policy explicitly (or accepting a documented divergence).
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
  collection replacement; durable Activity/Job logs (90-day activity TTL,
  7-day log TTL, secret redaction, lineage, per-app/service/user views,
  live-tailing over WebSocket); background job queue with retries.
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
| Rename app | ❌ | ✅ | `apps:rename`, optional skip-rebuild |
| Deploy lock (`apps:lock`/`unlock`) | ❌ | ✅ | Trivial command, real UI state |
| Maintenance mode | ❌ | ✅ | `maintenance` plugin |
| Env var view | ◐ masked | ✅ reveal/copy | Ours fixed `••••`; no re-auth unmask |
| Env var edit (batched, `.env`, unset, commit bar) | ❌ | ✅ | `config:export/set/unset` + rebuild job |
| Domains view | ✅ (+DNS check) | ✅ | Ours includes a DNS pre-check Pro hints at |
| Domains add/remove/set | ❌ | ✅ | `domains:add/remove/set` |
| Resource limit/reserve/CPU editing | ❌ | ✅ | `resource:limit/reserve`; ours read-only |
| Build configuration (buildpacks order, builder, build dir, scheduler and scheduler props) | ❌ | ✅ | Several families of commands |
| Cron tab (list, run-now, suspend/resume individual & app-wide) | ❌ | ✅ | `cron:*`; verify presence on 0.38.4 |
| HTTP basic auth (users + allowed IPs) | ❌ | ✅ | `http-auth` plugin |
| Queued-job toasts / "deleting" states | ❌ | ✅ | Falls out of the job queue |

### TLS, proxy, logs, builds, activity

| Capability | dokku-ui | Dokku Pro | Notes |
|---|---|---|---|
| Let's Encrypt (email, staging/prod, status/expiry, disable) | ❌ | ✅ | `letsencrypt` plugin, background job |
| Manual certificate upload / activate | ❌ | ✅ | `certs:*` |
| Server-wide auto-renew cron toggle | ❌ | ✅ | Admin setting |
| k3s cert-manager integration | ❌ | ✅ | Environment-specific; divergent unless host runs k3s |
| Live log tail + pause/follow | ❌ | ✅ | Pro uses WebSocket; our SSE infra can stream the same |
| Log source selector (app / nginx access / nginx error) | ❌ | ✅ | |
| Process filtering of application logs | ❌ | ✅ | |
| Last build/image status | ✅ | ✅ | `builds:report` |
| Build logs + deploy history | ❌ | ✅ | Via Activity |
| Per-service live logs | ◐ static tail | ✅ | Plugin ignores tail count on 0.38.4; we re-tail client-side |
| App live CPU/memory | ❌ | not documented | Not a Pro gap; no native dokku stats command |
| Activity/audit tab (app / service / user) | ❌ | ✅ | Foundation exists in `action_runs` |
| Durable audit, secret redaction, lineage, retention | ❌ | ✅ | Ours: runs pruned after 300s, no actor/attribution |
| Background job queue + retries | ❌ | ✅ | Ours: in-process runs, no queue |

### Services, data, plugins, storage

| Capability | dokku-ui | Dokku Pro | Notes |
|---|---|---|---|
| Service lifecycle + link/unlink | ✅ | ✅ | Pro blocks destroy while linked; ours does not |
| Service create advanced options (image/tag, env, extra args) | ❌ | ✅ | |
| Connection string / credential reveal | ❌ deliberate | ✅ | **Open policy decision** — test asserts DSNs never surface |
| Service Activity tab | ❌ | ✅ | |
| Datastore plugin coverage | 4 (pg/mysql/redis/mongo) | 18 official plugins | Our pages are generic; expansion is config + parsers |
| Expose/unexpose service ports | ✅ | not documented | We exceed the documented Pro surface |
| Service live stats (mem/CPU/data/disk) | ✅ | not documented | Our fixed read-only scripts; cgroup v2 only |
| Volumes page (mounts, mount/unmount, usage, host disk) | ✅ | not documented | We exceed the documented Pro surface |
| Named storage entries (`storage:create/destroy/info`) | ❌ | ❌ | Not a Pro gap either |
| Plugin management (list/install/enable/disable/update/uninstall) | ❌ | ✅ | Root-only on host; needs companion helper |

### Identity, API, transport, multi-server

| Capability | dokku-ui | Dokku Pro | Notes |
|---|---|---|---|
| Multi-user accounts | ❌ | ✅ | Ours: single admin from setup |
| Password set / reset token & link | ❌ | ✅ | `users:set-password`, reset URLs |
| SSH key management | ❌ | ✅ | Maps to dokku `ssh-keys` |
| Teams (owners/members) | ❌ | ✅ | Pro ships a `teams` host plugin |
| Command/app/service scoped grants | ❌ | ✅ | Enforced via host plugin triggers |
| Internal per-app / per-service teams | ❌ | ✅ | Auto-managed `dokku@app--…`/`dokku@service--…` |
| Per-user activity audit | ❌ | ✅ | |
| Reverse-proxy authentication (trusted header + CIDR, auto-registration) | ❌ | ✅ | Env-only; small once multi-user exists |
| REST JSON:API + JWT access/refresh tokens | ❌ | ✅ | |
| Swagger UI + OpenAPI spec | ❌ | ✅ | |
| Atomic batch operations (`/operations`, coalescing, collection replacement) | ❌ | ✅ | Coalescing is the complex half |
| Git HTTP server + push URL on app page | ❌ | ✅ | Hardest gap; needs host helper in SSH-only model |
| GitHub webhooks (HMAC receiver, per-app config) | ❌ | ✅ | Needs public route + secret storage |
| Deploy from git / image / archive | ❌ | ✅ | `git:sync`, `git:from-image`, `git:from-archive` |
| Multi-server (peer list, CORS, custom servers, selector) | ❌ | ✅ | Our "multi" is containers of one server |
| Theme light/dark, command palette, login banner, app filter | ❌ | ✅ | Polish/later |
| Ops config parity (JWT secrets, root token, TTLs, public URL, CORS, timeouts) | ◐ subset | ✅ | Public URL is a prerequisite for future reset links |
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

## Capability detection framework (prerequisite, not yet designed in code)

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
- DSN non-exposure remains a divergence until the policy decision flips.

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
  migration `0007`): `dokku --version` + `plugin:list` + `<family> --help` probes
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

## Refreshing this document

- Re-inventory routes from `src/web/mod.rs` and commands from
  `src/domain/command.rs` first; both are exhaustive and single-source.
- Re-read the Dokku Pro doc nav at `https://pro.dokku.com/docs/` — new
  top-level nav entries are the signal that the parity target moved.
- Update the "Locked decisions" only with an explicit call; everything else
  is descriptive.
