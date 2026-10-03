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

## Formats

| Fixture | Command | Notes |
|---|---|---|
| `apps_list.txt` | `dokku apps:list` | Plain text: `=====> My Apps` header, one app per line. **`apps:list` has no `--format` flag** (exit 2 on 0.38.4); the parser strips ANSI, trims, and skips blank/header lines. |
| `apps_report.json` | `dokku apps:report <app> --format json` | Flat map with **hyphenated keys** (`app-created-at`, `app-locked`). `created_at` is a unix-epoch string, formatted to UTC at parse time. Spaced keys from older dokku are accepted as a fallback. |
| `ps_report.json` | `dokku ps:report <app> --format json` | Captured from a running app. `status-<type>.<n>` keys (e.g. `status-web.1`) hold `<state> (CID: ...)`. |
| `ps_report_not_deployed.json` | `dokku ps:report <app> --format json` | Captured from a created-but-never-deployed app: `deployed=false`, `processes=0`, no `status-*` keys. |
| `ps_report_missing.json` | synthetic | Covers the `missing` process state for a removed container. |
| `config_show.txt` | `dokku config:show <app>` | `=====> <app> env vars` header + `KEY:  value` lines. Values are fetched **unmasked**; the UI masks client-side with a fixed-length glyph string (deviation from plan.md §8's `--masked`, same visible result). |
| `config_show_empty.txt` | `dokku config:show <app>` | No env vars set. |
| `logs.txt` | `dokku logs <app> --num 200` | Docker log lines **with ANSI color codes**; the parser strips them. |

## Maintenance

When dokku output drifts (after a host upgrade), re-capture with the exact
argv built by `src/domain/command.rs` and refresh the fixtures; the parsers
are tolerant (unknown JSON fields ignored, `#[serde(default)]`-style
defaults) so minor additions should not break anything.
