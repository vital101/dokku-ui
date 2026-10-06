# Dokku output fixtures

Golden outputs from the commands `dokku-ui` executes over SSH, used by the
domain parsers' unit tests and the `MockClient` integration suite.

## Source

- **Host:** dokku.re-cycledair.com
- **Dokku version:** 0.38.4
- **Captured:** 2026-10-03, with deploy/TLS/logs-failed captures added 2026-10-06
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
> **Synthetic fixtures:** the service-list, service-log and
> `storage_report_empty.txt` fixtures (`*_list.txt`, `redis_logs.txt`,
> `storage_report_empty.txt`) are **not live captures yet** — no host access was
> available when the service/volume pages landed. Their formats were verified
> against the installed plugin sources (dokku-service
> `service_list`/`service_logs` in dokku-redis 1.42.1; `plugins/storage/report.go`
> + `subcommands.go` at dokku tag `v0.38.4`) and should be replaced by real
> captures on the next host session. The stats/entries fixtures below are real
> captures from 2026-10-04.
>
> **Added service plugins (mariadb, memcached, rabbitmq, clickhouse):** no new
> fixtures — the list/info/log commands share the generic plain-text shapes
> above. Plugin `display_name`/`data_dir` mappings were source-verified against
> the plugins' official images (`/var/lib/mysql`, container root for
> memcached's volume-less layout, `/var/lib/rabbitmq`, `/var/lib/clickhouse`).
> The sidebar only shows plugins the `plugin:list` capability probe reports as
> installed.

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
| `storage_report.txt` | `dokku storage:report` (no app) | Real capture (2026-10-04, 14 apps): one `=====> <app> storage information` section per app with `Storage build/deploy/run mounts: -v host:container[:opts]` lines. Only these three keys exist on 0.38.4 (the dotted `attachment.N.*` keys are newer). `parse_storage_report` merges phases per mount. |
| `storage_report_empty.txt` | `dokku storage:report` | Synthetic: an app with the three mount lines all empty. |
| `redis_stats.txt` | `dokku redis:enter candid sh -c '<stats script>'` | Real capture (2026-10-04). The plugin prints a `-----> Filesystem changes may not persist…` banner first; then the fixed script's `key=value` lines: host `/proc/meminfo`, cgroup v2 `memory.current`/`memory.max` (`max` = unlimited), `memory.stat` inactive_file, `cpu.stat` sampled twice over ~1s, `nproc`, `du`/`df` of the data dir. |
| `volume_usage.txt` | `dokku storage:exec legacy-90db719326 -- sh -c '<usage script>'` | Real capture (stdout only; docker pull progress goes to stderr): `du`/`df` of the entry mounted at `/data` in the throwaway `alpine:3` container. |
| `list_entries.json` | `dokku storage:list-entries --format json` | Real capture (2026-10-04): `name`/`scheduler`/`host_path`/`schema_version`. Maps a mount's host path onto its `legacy-<hash>` entry name, which `storage:exec` needs. |
| `dokku_version.txt` | `dokku version` | `dokku version 0.38.4`. **Not `--version`** — the flag does not survive the SSH wrapper (the host rejects a coreutils banner as the command name). |
| `logs_help.txt` | `dokku logs:help` | Real capture (2026-10-05). `<cmd>:help` is dokku's help convention over SSH; `<cmd> --help` is parsed as an app name by the logs plugin. The `logs` line names `-t|--tail`, which is how live-tail support is probed. |
| `nginx_help.txt` | `dokku nginx:help` | Real capture (2026-10-05). The `nginx` plugin's help lists `nginx:access-logs <app> [-t]` and `nginx:error-logs <app> [-t]` — the capability probe scopes the `-t` match to each command's line. |
| `apps_report_locked.json` | `dokku apps:report <app> --format json` | Real capture (2026-10-05) of a locked app (`app-locked: "true"`). |
| `cron_list_empty.json` | `dokku cron:list <app> --format json` | Real capture (2026-10-05): `[]` for an app with no cron tasks. Populated entries (id/schedule/concurrency/maintenance/command) are parsed tolerantly. |
| `buildpacks_list_empty.txt` | `dokku buildpacks:list <app>` | Real capture (2026-10-05): the `-----> <app> buildpack urls` header with no URLs. |
| `builder_report.txt` | `dokku builder:report <app>` | Real capture (2026-10-05): `Builder selected:`, `Builder computed selected:`, etc. `builder:set <app> selected <builder>` sets; no value clears. |
| `git_report.txt` | `dokku git:report dokku-ui` | Real capture (2026-10-06). Header `=====> <app> git information` + space-padded `Git <key>: value` lines. `Git sha` is `git rev-parse HEAD` on the app's bare repo — it literally prints `HEAD` on unborn refs, so the parser only accepts commit-hash-shaped values. `Git last updated at` is the deploy-branch ref mtime (unix seconds) and is empty when the branch ref does not exist yet. |
| `git_report_not_deployed.txt` | `dokku git:report starwars` | Real capture: no explicit deploy branch (computed falls back to the `--global` value, `master`), real sha. |
| `git_report_fresh.txt` | `dokku git:report scratch-m20` | Real capture of a just-created app (scratch app, destroyed after): empty deploy branch and last-updated, `Git sha: HEAD`. |
| `git_report_synced.txt` | `dokku git:report scratch-m20` (after `git:sync`) | Real capture: `git:sync` auto-set `deploy-branch` to the detected branch, real sha, last-updated set. |
| `git_public_key_missing.txt` | `dokku git:public-key` | Real capture (2026-10-06): the host has no deploy key, so the plugin prints a three-line warning on stdout and exits 1. The key-present form (`ssh-ed25519 AAAA...`) cannot be captured without generating a key and is fixture-free; the parser is unit-tested for both shapes. |
| `git_sync.txt` | `dokku git:sync scratch-m20 https://github.com/octocat/Hello-World.git` | Real capture (2026-10-06): clone banner plus `Detected branch, setting deploy-branch to master`. No flags means no build/deploy. Flags are `--build` / `--build-if-changes` / `--skip-deploy-branch`, and they precede the app. |
| `letsencrypt_list.txt` | `dokku letsencrypt:list` | Real capture (2026-10-06): `-----> App name ...` banner + fixed-width rows (app, absolute expiry `YYYY-MM-DD HH:MM:SS` UTC, time before expiry, time before renewal). The countdown columns drift; parsers should key on app + absolute expiry. |
| `letsencrypt_active_true.txt`, `letsencrypt_active_false.txt` | `dokku letsencrypt:active <app>` | Real captures: the literal string `true`/`false`, exit 0 either way. |
| `letsencrypt_help.txt` | `dokku letsencrypt:help` | Real capture. Confirms `cron-job [--add --remove]` (no status query on 0.20.4 — no-args prints `Specify --add or --remove to modify the cron-job`), plus `auto-renew`, `cleanup`, `disable`, `enable`, `revoke`, `set`. |
| `certs_report.txt` | `dokku certs:report` (no app) | Real capture (2026-10-06): one `=====> <app> ssl information` section per app; keys `Ssl dir/enabled/hostnames/expires at/issuer/starts at/subject/verified` (no `--format json` on 0.38.4). |
| `certs_report_app.txt` | `dokku certs:report dokku-ui` | Real capture: single-app form of the same section shape. |
| `certs_report_disabled.txt` | `dokku certs:report scratch-m20` | Real capture of an app without TLS: `Ssl enabled: false`, every other value empty. |
| `logs_failed.txt` | `dokku logs:failed dokku-ui` | Real capture (2026-10-06): the `=====> <app> failed deploy logs` header only. No app on the host currently has failed deploy logs, and 0.38.4 prints **no** `!` warning for the empty case (unlike older docs); parser drops the header, keeps the rest. |
| `logs_failed_populated.txt` | synthetic | Source-verified against dokku v0.38.4 `plugins/logs/logs.go` (`GetFailedLogs`: header + streamed `scheduler-logs-failed` output). Covers the populated branch, including `remote: !` error lines the parser must keep. |

## Maintenance

When dokku output drifts (after a host upgrade), re-capture with the exact
argv built by `src/domain/command.rs` and refresh the fixtures; the parsers
are tolerant (unknown keys ignored, empty/`-` values treated as unset) so minor
additions should not break anything. If a plugin upgrade adds `--format json`,
prefer switching the command + parser together and re-capturing.
