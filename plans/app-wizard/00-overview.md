# App creation wizard - overview

Read this file, then exactly one numbered file. Do not load the rest unless the current file says to.

## Goal

Replace the current `/apps/new` form with a five-step wizard:

1. Name
2. Build (builder + optional buildpack URLs)
3. Services (attach existing or create + attach)
4. Volumes (create named entry, attach registered entry, or attach host path)
5. Review the exact command plan, then execute it as one streamed run

## Locked decisions

- Sequential pages; draft stored in the existing SQLite-backed session.
- No SPA, no inline JS/CSS. Plain form posts; only the final execute uses the existing `data-action-form` + run modal.
- Service creation is name-only (`ServiceCreateOptions::default()`). Advanced options stay on the service Overview page.
- Volume creation uses dokku 0.38.4 `storage:create <entry>` plus a named mount `storage:mount <app> <entry> --container-dir <container>`.
- Attaching an arbitrary host path reuses the existing colon-form `storage:mount` (`MountSpec`).
- One combined persisted job. Order: app create, builder/buildpacks, service create/link, volume create/mount.
- No rollback. A later failure leaves a partially configured app; the run log shows the stopping point.
- Buildpacks require the `herokuish` builder. Auto + buildpacks inserts `builder:set <app> selected herokuish`; an explicit non-herokuish builder with buildpacks is rejected.
- The combined run is audited under the app (`TargetKind::App`, operation `app.create`). It does not appear on individual service/volume activity pages.

## Plan commands (dokku 0.38.4)

```text
apps:create <app>
builder:set <app> selected <builder>
buildpacks:add <app> <url>                         # one per URL, input order
<plugin>:create <service>                          # default options
<plugin>:link <service> <app>
storage:create <entry>                             # creates/registers the host dir
storage:mount <app> <entry> --container-dir <path>
storage:mount <app> <host>:<container>             # arbitrary host path
```

`storage:create` and the named `storage:mount` form are source-verified against dokku `v0.38.4` (`plugins/storage/subcommands.go`, `commands_entries.go`, `entry.go`).

## Route map

Register every wizard route **before** `/apps/{name}` and `/apps/{name}/...` so static segments win.

```text
GET  /apps/new
POST /apps/new
POST /apps/new/reset

GET  /apps/new/build
POST /apps/new/build

GET  /apps/new/services
POST /apps/new/services/attach
POST /apps/new/services/create
POST /apps/new/services/remove

GET  /apps/new/volumes
POST /apps/new/volumes/create
POST /apps/new/volumes/attach-entry
POST /apps/new/volumes/attach-path
POST /apps/new/volumes/remove

GET  /apps/new/review
POST /apps/new/execute
```

## Shared invariants

- Every POST handler takes `CsrfForm<T>`.
- Every step GET/POST checks `can_manage_apps`; the auth middleware remains authoritative.
- Session draft key: `app_wizard_draft`. Corrupt/old drafts fall back to default.
- Draft limits: 20 buildpacks, 10 services, 10 volumes.
- All values are revalidated by `AppWizardDraft::to_plan()` and again by `JobSpec::to_commands()`.
- No secrets are stored in the draft or shown in the plan preview.
- No new migration: the session store is already shared SQLite.

## Implementation order

```text
00-overview.md
01-domain-storage-entry.md
02-commands-and-job-specs.md
03-wizard-draft-and-plan.md
04-session-routes-and-name-step.md
05-build-step.md
06-services-step.md
07-volumes-step.md
08-review-and-plan-preview.md
09-execute-and-completion.md
10-remove-old-flow-and-docs.md
11-integration-tests-and-verification.md
```

Each step must leave the crate compiling and its focused tests green. Do not start the next file until the current one is done.

## Definition of done

- `make lint` clean.
- `make test` green (Tailwind runs first).
- `make coverage-gate` at least 90% lines.
- `tests/horizontal.rs` unchanged and green.
