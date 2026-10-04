# Dokku output fixtures

Golden outputs from the commands `dokku-ui` executes over SSH, used by the
domain parsers' unit tests and the `MockClient` integration suite.

## Source

- **Host:** dokku.re-cycledair.com
- **Dokku version:** 0.38.4
- **Captured:** 2026-10-03
- **Validation:** `tests/dokku_smoke.rs` (`-- --ignored`) runs `apps:list`
  through the real russh client against the live host and parses it with
  `parse_apps_list`. `tests/russh_integration.rs` runs the client against an
  in-process fake SSH server.

> **Installed plugin/dokku versions matter.** The App Detail fixtures were
> captured from the host above (dokku 0.38.4, redis 1.42.1, postgres 1.36.4).
> These versions do **not** support `--format json` on `ps:scale` or on the
> service `<plugin>:info` commands (the current dokku.com docs describe a newer
> release), so both are parsed from their **plain-text** reports. Commands and
> argv live in `src/domain/command.rs`, in lockstep with the parsers.
>
> Service fixture DSNs contain credentials on the live host and are **redacted
> to `XXXXXX`** here. The parser never reads the `Dsn` line and the UI never
> renders it; a test asserts the DSN does not appear in `ServiceInfo`.
>
> **Synthetic fixtures:** the service-list, service-log and storage-report
> fixtures (`*_list.txt`, `redis_logs.txt`, `storage_report*.txt`) are **not
> live captures yet** — no host access was available when the service/volume
> pages landed. Their formats were verified against the installed plugin
> sources (dokku-service `service_list`/`service_logs` in dokku-redis 1.42.1;
> `plugins/storage/report.go` + `subcommands.go` at dokku tag `v0.38.4`) and
> should be replaced by real captures on the next host session.

## Formats

| Fixture | Command | Notes |
|---|---|---|
| `apps_list.txt` | `dokku apps:list` | Plain text: `=====> My Apps` header, one app per line. **`apps:list` has no `--format` flag** (exit 2 on 0.38.4); the parser strips ANSI, trims, and skips blank/header lines. |
| `apps_report.json` | `dokku apps:report <app> --format json` | Flat map with **hyphenated keys** (`app-created-at`, `app-locked`). `created_at` is a unix-epoch string, formatted to UTC at parse time. Spaced keys from older dokku are accepted as a fallback. |
| `ps_report.json` | `dokku ps:report <app> --format json` | Captured from a running app. `status-<type>.<n>` keys (e.g. `status-web.1`) hold `<state> (CID: ...)`. |
| `ps_report_all.txt` | `dokku ps:report <app1> <app2> ...` | **Dashboard uses this** (JSON mode ignores extra apps). One command reports every app: `=====> <app> ps information` sections with indented `Key: value` lines; `Status web 1` maps to process `web.1`. |
| `ps_report_not_deployed.json` | `dokku ps:report <app> --format json` | Captured from a created-but-never-deployed app: `deployed=false`, `processes=0`, no `status-*` keys. |
| `ps_report_missing.json` | synthetic | Covers the `missing` process state for a removed container. |
| `builds_report.json` | `dokku builds:report <app> --format json` | Latest build record for a succeeded build (`build-status: succeeded`, exit 0). Drives the "Image status" detail. |
| `builds_report_failed.json` | synthetic | A `build-status: failed` record with exit code 1, for the error branch. |
| `builds_report_empty.json` | `dokku builds:report <app> --format json` | Captured from a created-but-never-built app (`starwars`): every `build-*` value is an empty string. |
| `domains_report.json` | `dokku domains:report <app> --format json` | App with a custom vhost (`app-vhosts: dokku.re-cycledair.com`). Drives the "DNS record" detail. |
| `domains_report_default.json` | `dokku domains:report <app> --format json` | App relying on the auto-assigned `<app>.<global-vhost>` (`starwars.re-cycledair.com`). |
| `plugin_list.txt` | `dokku plugin:list` | Full plugin listing; the parser keeps enabled plugins whose description ends in `service plugin` (mongo, mysql, postgres, redis here). |
| `app_links.txt` | `dokku postgres:app-links <app>` | Names of services linked to the app (`roboswarm-db`). Drives the "Link exists" detail. |
| `config_show.txt` | `dokku config:show <app>` | `=====> <app> env vars` header + `KEY:  value` lines. Values are fetched **unmasked**; the UI masks client-side with a fixed-length glyph string (deviation from plan.md §8's `--masked`, same visible result). |
| `config_show_empty.txt` | `dokku config:show <app>` | No env vars set. |
| `logs.txt` | `dokku logs <app> --num 200` | Docker log lines **with ANSI color codes**; the parser strips them. |
| `ps_scale.txt` | `dokku ps:scale <app>` | Plain text: `-----> Scaling for <app>`, `proctype: qty` / `--------: ---` header, then `<type>: <qty>` rows (real capture: release 0, web 1, worker 2). **No `--format json` on 0.38.4.** |
| `ps_inspect.json` | `dokku ps:inspect <app>` | Sanitized `docker inspect` array. Real-host keys confirmed: `Id`, `Name`, `Config.Image`, `State.{Status,StartedAt,RestartCount,OOMKilled,ExitCode}`. Fixture body is representative (trimmed). |
| `resource_report.txt` | `dokku resource:report <app>` | Indented `web limit memory: 1024` / `web reservation cpu:` lines (synthetic populated case). Parser keeps cpu/memory/memory-swap limits + cpu/memory reservations; ignores network/GPU. |
| `resource_report_empty.txt` | `dokku resource:report <app>` | Real capture for an app with no limits: the header line only. |
| `redis_info.txt` | `dokku redis:info <service>` | Plain-text report (real capture, DSN redacted): `Status`, `Version` (`redis:7.2.4`), `Exposed ports` (`-` when unset), `Internal ip` (often empty), `Id`, `Links` (apps). **No `--format json` on 1.42.1.** |
| `postgres_info.txt` | `dokku postgres:info <service>` | Same format (real capture, DSN redacted); shows `Exposed ports: 5432->15432`. |
| `redis_list.txt` | `dokku redis:list` | Synthetic (source-verified): `=====> Redis services` banner + one bare service name per line. The installed plugin generation prints **no status/version columns**; `parse_service_list` skips banners and `!` warnings and validates the name charset. |
| `postgres_list.txt`, `mysql_list.txt`, `mongo_list.txt` | `dokku <plugin>:list` | Same shape for the other service plugins (synthetic). |
| `service_list_empty.txt` | `dokku redis:list` | Synthetic empty listing: ` !     There are no Redis services` warning line only. |
| `redis_logs.txt` | `dokku redis:logs candid '' 200` | Synthetic log tail. The installed plugin takes the follow flag as `$2` and the line count as `$3`, so the argv carries an explicit empty string that `russh_client::shell_command` quotes to preserve it. |
| `storage_report.txt` | `dokku storage:report` (no app) | Synthetic (source-verified at `v0.38.4`): one `=====> <app> storage information` section per app with `Storage build/deploy/run mounts: -v host:container[:opts]` lines. Only these three keys exist on 0.38.4 (the dotted `attachment.N.*` keys are newer). `parse_storage_report` merges phases per mount. |
| `storage_report_empty.txt` | `dokku storage:report` | Synthetic: an app with the three mount lines all empty. |

## Maintenance

When dokku output drifts (after a host upgrade), re-capture with the exact
argv built by `src/domain/command.rs` and refresh the fixtures; the parsers
are tolerant (unknown keys ignored, empty/`-` values treated as unset) so minor
additions should not break anything. If a plugin upgrade adds `--format json`,
prefer switching the command + parser together and re-capturing.
