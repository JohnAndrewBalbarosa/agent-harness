# agent-harness

Portable, Windows-first middleware that every AI coding agent connects to. One policy, one hook router, one set of
consumers (observability, token usage, notifications, background-task tracking, session context) and one database,
instead of per-agent copies. Ships adapters for **Codex CLI** and **Claude Code**; other agents plug in through a
small adapter that passes the conformance kit ([docs/adding-an-agent.md](docs/adding-an-agent.md)).

- **Compile (build time):** one source → each agent's native files. Hooks are generated with
  [rulesync](https://github.com/dyoshikawa/rulesync) (pinned, simulated before writing); the policy reaches Codex by
  hard link and Claude Code by a managed `@import` block; OpenTelemetry settings are merged into managed sections.
- **Aggregate (runtime):** every agent hook runs `harness-hook <agent>`; the agent's adapter normalizes it into an
  `agent-harness.event/v1` common event; consumers run in parallel (hidden, timed out, fail-open).

Architecture: [docs/architecture.md](docs/architecture.md) · Design spec:
[docs/superpowers/specs/2026-09-28-agent-harness-design.md](docs/superpowers/specs/2026-09-28-agent-harness-design.md)

## Safety boundary

The repository contains no credentials, sessions, prompts, histories, logs, database rows or personal policy.
Machine specifics live in `%USERPROFILE%\.agent-harness\.env`. Installing never overwrites owner-owned files
(`.env`, `policy\AGENTS.md`, `var\`); `harness compile` backs up every agent file before changing it
(`var\backups\compile-<timestamp>\`) and only touches managed hook entries, managed env keys and marked blocks.
Telemetry mapping reads whitelisted fields only; personal data carried in OpenTelemetry events is never stored.

## Install

Requirements: Python 3.11+ (uv-managed 3.14 preferred), Node.js 22+ (`npx` for rulesync), optionally Docker for the
observability hub. Missing tools are installed with `winget` unless `-Prerequisites ValidateOnly`.

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\install.ps1                 # install + compile + start receiver
.\scripts\install.ps1 -DryRun                                                   # plan only, writes nothing
.\scripts\install.ps1 -CompileDryRun                                            # install code, show config diff only
.\scripts\install.ps1 -WithHub                                                  # also deploy the observability hub (Docker)
.\scripts\doctor.ps1 -CheckCompile                                              # read-only health report
.\scripts\uninstall.ps1 [-Purge]
```

Edit `%USERPROFILE%\.agent-harness\.env` (see [.env.example](.env.example)) to list agent instances, e.g.
`INSTANCES="codex:personal=%USERPROFILE%\.codex;claude:default=%USERPROFILE%\.claude"`, then re-run compile:

```powershell
$env:PYTHONPATH="$env:USERPROFILE\.agent-harness"; python -m compile.run --dry-run   # review
$env:PYTHONPATH="$env:USERPROFILE\.agent-harness"; python -m compile.run             # apply (with backups)
```

Restart each agent afterwards. Authentication stays per agent home and is never shared.

## What each agent gets

| Capability | How |
|---|---|
| Shared policy | `policy\AGENTS.md` (owner-edited) → Codex hard link, Claude `@import` |
| Observability | `obs` consumer → observability client → hub (`OBS_ENDPOINT`), offline spool |
| Token usage | built-in OpenTelemetry → local receiver (`127.0.0.1:OTLP_PORT`) → `turn_usage`; transcript fallback per turn |
| Notifications | done / needs-you toasts, deferred while background agents or shells still run; Herdr bell when inside Herdr |
| Background tasks | subagent start/stop hooks + transcript scan for background shells |
| Session context | hub project context injected at session start |
| Services | session start detaches a bootstrap that keeps the receiver, status orb, console monitor and hub running |

> **Token totals:** every turn is recorded twice on purpose — `source = '<agent>_otel'` (per API request, from
> OpenTelemetry) and `source = '<agent>_hook'` (per turn, from the transcript). Filter `turn_usage` by one source
> when summing, or you will double count. `_otel` is authoritative; `_hook` covers sessions without OTel.

Diagnostics: `python -m core.notify.report --minutes 120` cross-references notifications with Windows toast
history and transcripts; router and consumer lifecycle logs live in `var\logs\`.

## Performance hypotheses

Validated tasks default to non-inferiority. Fewer than 30 valid samples always reports `insufficient_data`; only a
statistically confirmed regression can block packaging or push.

```powershell
& "$env:USERPROFILE\.agent-harness\tools\observability-client\obs.cmd" benchmark catalog verify
& "$env:USERPROFILE\.agent-harness\tools\observability-client\obs.cmd" benchmark evaluate --profile personal --window 30 --format table
```

## Verification

```powershell
uv run --with scipy==1.18.1 python -m unittest discover -s tests -t . -v   # unit, conformance, integration (needs npx)
.\tests\test_installer.ps1
.\scripts\verify-secrets.ps1
```

Secret verification downloads the official Gitleaks v8.30.1 archive, verifies its checksum, runs a synthetic
detection canary, then scans the repository.

## Sources

- [Claude Code hooks](https://code.claude.com/docs/en/hooks) · [Claude Code monitoring (OpenTelemetry)](https://code.claude.com/docs/en/monitoring-usage)
- [Codex hooks input schemas](https://github.com/openai/codex/tree/main/codex-rs/hooks/schema/generated) · [Codex OpenTelemetry](https://github.com/openai/codex/tree/main/codex-rs/otel)
- [rulesync](https://github.com/dyoshikawa/rulesync) · [Custom instructions with AGENTS.md](https://agents.md/)
- [DORA software delivery performance metrics](https://dora.dev/guides/dora-metrics/)

## License

See [LICENSE](LICENSE).
