# agent-harness — agent-agnostic middleware design

Status: draft for review · Date: 2026-09-28 · Supersedes the Codex-only scope of `codex-system-harness`.

Legend: **MUST / MUST NOT / SHOULD / MAY** per RFC 2119. *Agent* = an AI coding CLI (Codex CLI, Claude Code, …).
*Instance* = one configured home of an agent (e.g. `codex/personal`). *Common event* = the normalized hook event
defined in §6. *Consumer* = a core component that reacts to common events. *Installed runtime* = the machine-local
copy under `%USERPROFILE%\.agent-harness`. *Repo* = this distributable repository.

## 1. Purpose and scope

One portable middleware that every agent connects to, so policy, lifecycle hooks, observability, token usage,
notifications, and background-task tracking are implemented once instead of copied per agent.

- **Compile (build time):** one source of policy/config → each agent's native files.
- **Aggregate (runtime):** every agent's hook events and telemetry → one router → shared consumers → one database.

Goals (v1):
1. Codex CLI (instances `personal`, `cy`, `feu`) and Claude Code (`default`) run through the same middleware with
   behavior parity to today's `.codex-shared` + `~/.claude` setup.
2. The repo is publishable: no hardcoded user paths, no secrets; machine specifics live in `.env`.
3. Adding an agent = one adapter + passing the adapter conformance kit (§12), no copied logic.

Non-goals (v1): adapters for Copilot/Gemini/Cursor/OpenCode (contract and docs only), dashboard UI,
multi-machine/remote operation, replacing the observability hub service.

## 2. Current state (as-is)

| Area | Codex | Claude Code |
|---|---|---|
| Policy | `.codex-shared/AGENTS.md`, hard-linked into 3 homes | `~/.claude/CLAUDE.md` `@import`s the same file |
| Hooks | `hooks.json` → `global-hook-router` → orb / obs / herdr-bridge / legacy notifier | per-event shims: `obs-hook.ps1`, `notify-gate.ps1`, `background_tasks.py hook`, OTel `ensure` |
| Observability | obs `hook.py` (`CODEX_OBS_INSTANCE`) | same `hook.py` via `obs-hook.ps1` (`CODEX_OBS_INSTANCE=claude`) |
| Token usage | herdr-bridge `turn-usage.record` (payload-based) | OTel receiver (`claude_otel`) + transcript parser (`claude_hook`) |
| Notifications | legacy `codex_cli_notify.py`, or Herdr when inside Herdr | `notify-gate.ps1` (background-task aware) |
| Config sync | `sync-global.cmd` → `config/sync_config.py` | manual edits |

Repo today: `harness/templates` (`AGENTS.md.tmpl`, `hooks.json.tmpl`, `config/`), `harness/tools`
(`global-hook-router`, `codex-status-orb`, `observability-hub`), `scripts/` (`install.ps1`, `doctor.ps1`,
`uninstall.ps1`, `verify-secrets.ps1`), `tests/`, `benchmarks/`, CI `verify.yml`. Not yet a Git repository.

## 3. Requirements

- R1 Every hook of every connected agent MUST enter through one command, `harness-hook <agent> <native-event>`.
- R2 Consumers MUST depend only on common events, never on agent-native payloads.
- R3 A hook MUST NOT block or fail its agent: every error path returns the adapter's safe default response.
- R4 Policy MUST have one owner-editable source per machine; agents MUST receive it through compiled native files.
- R5 The repo MUST NOT contain user paths, credentials, prompts, logs, or personal policy text (`verify-secrets.ps1`).
- R6 Telemetry mapping MUST whitelist fields; personal data (e.g. `user.email`, `user.id`, `organization.id`) MUST NOT be stored.
- R7 Compile MUST be idempotent, MUST support `--dry-run` (diff only), and MUST NOT modify keys outside managed sections.
- R8 Log reading follows the shared policy: bounded tails, query the database, interval supervisor for noisy commands.
- R9 Existing Codex observability history MUST stay continuous (same agent-instance IDs for Codex instances).
- R10 Migration MUST be staged, backed up, reversible, and verified in fresh sessions per stage.

## 4. Architecture

```
agent CLI ──native hook──► harness-hook <agent> <event>
                              │ adapters/<agent>.normalize()
                              ▼
                         core/router ──parallel, hidden, per-consumer timeout──►
                              │   obs · notify · tasks · context · herdr · orb
                              ▼
                         adapters/<agent>.respond(results) ──stdout──► agent

agent CLI ──OTLP/HTTP JSON──► core/usage receiver (127.0.0.1:${OTLP_PORT}) ──► obs turn-usage.record ──► hub DB
```

### 4.1 Repo layout (evolved from `codex-system-harness`)

```
agent-harness/
  .env.example
  policy/AGENTS.template.md        # generic policy shipped to others (was harness/templates/AGENTS.md.tmpl)
  .rulesync/                        # rulesync source (rendered by compile from templates + .env)
  core/
    router/                         # generalized global-hook-router
    events.py                       # common event schema + validation
    consumers/obs.py                # common events → obs operations
    notify/                         # decision engine + delivery backends (herdr, windows)
    tasks/                          # background-task state (subagent events + adapter shell scan)
    usage/                          # OTLP receiver + mapping registry
    context/                        # session-start context injection (from herdr-bridge build_context)
  adapters/
    codex/  adapter.py  manifest.toml  otel.py  transcript.py  fixtures/
    claude/ adapter.py  manifest.toml  otel.py  transcript.py  fixtures/
  tools/                            # moved unchanged: interval-supervisor, codex-status-orb, herdr-bridge
  scripts/  install.ps1  compile.ps1  doctor.ps1  uninstall.ps1  verify-secrets.ps1
  tests/    unit/  conformance/  integration/  e2e/
  docs/     adding-an-agent.md  architecture.md  superpowers/specs/
```

The observability hub remains a separate service/repository (`codex-observability-hub`), addressed by
`OBS_ENDPOINT`; `install.ps1` MAY set it up.

### 4.2 Installed runtime

`%USERPROFILE%\.agent-harness\` holds code, `.env`, `policy/AGENTS.md` (owner's personal policy, created from the
template on first install and never overwritten afterwards), `var/` (spool, lifecycle logs, state).
During migration `%USERPROFILE%\.codex-shared` becomes a directory junction to it.

### 4.3 Identity

`agent/instance` replaces Codex profiles and `CODEX_OBS_INSTANCE`. Agent-instance ID =
`uuid5(URL, "codex-observability:<instance>")` for `codex/*` (legacy continuity, R9) and
`uuid5(URL, "agent-harness:<agent>/<instance>")` for all others. Display name: `"<Agent> <instance>"`.

## 5. Configuration (`.env`)

| Key | Example | Purpose |
|---|---|---|
| `HARNESS_HOME` | `%USERPROFILE%\.agent-harness` | installed runtime root |
| `HARNESS_PYTHON` | path to pinned interpreter | router, consumers, receiver |
| `INSTANCES` | `codex:personal=%USERPROFILE%\.codex;codex:cy=…;claude:default=%USERPROFILE%\.claude` | agent homes |
| `OBS_ENDPOINT` | `http://127.0.0.1:4319` | observability hub |
| `OTLP_PORT` | `4320` | usage receiver |
| `NOTIFY_RATE_LIMIT` | `5/60` | toasts per window (seconds) |
| `HERDR_BIN` | optional | Herdr integration; absent = disabled |
| `RULESYNC_VERSION` | `22.0.0` | pinned compiler |

`.env.example` documents every key; secrets are generated locally by `install.ps1` and never committed.

## 6. Common event contract (`agent-harness.event/v1`)

| Common event | Codex native | Claude native |
|---|---|---|
| `session.start` / `session.end` | SessionStart / SessionEnd | SessionStart / SessionEnd |
| `prompt.submit` | UserPromptSubmit | UserPromptSubmit |
| `tool.completed` | PostToolUse | PostToolUse |
| `turn.stop` | Stop | Stop |
| `attention.needed` (`kind`: `permission` \| `idle`) | PermissionRequest | Notification (`permission_prompt` \| `idle_prompt`) |
| `subagent.start` / `subagent.stop` | — / SubagentStop | SubagentStart / SubagentStop |

Envelope (JSON, UTF-8):

```json
{ "schema": "agent-harness.event/v1", "event": "turn.stop",
  "agent": "claude", "instance": "default", "session_id": "…", "turn_id": null,
  "cwd": "…", "transcript_path": "…", "ts": "2026-09-28T00:00:00.000Z",
  "data": { "prompt": null, "tool": null, "attention": null, "subagent": null, "last_message": "…" },
  "native": { "event": "Stop", "payload": { } } }
```

`native.payload` is bounded (≤ 8 KiB) and not persisted by default. Unknown native events normalize to `None`
and receive the safe default response.

## 7. Adapter contract

Each `adapters/<agent>/` MUST provide:
- `normalize(native_event: str, payload: dict) -> Event | None`
- `respond(event: Event, results: list[ConsumerResult]) -> str` — agent-specific stdout (e.g. Codex Stop
  `{"continue":true}`; `hookSpecificOutput.additionalContext` on session start)
- `safe_default(native_event) -> str` — used on any failure (R3)
- `manifest.toml` — agent id, supported native events, rulesync target, tested agent versions, OTel setup,
  transcript locator
- `otel.map(attrs) -> TurnUsage | None` — whitelist mapping (R6)
- `transcript.usage(path, turn) -> TurnUsage` — fallback parser
- `patches(env) -> list[ManagedPatch]` — native config not covered by rulesync (e.g. Claude `env` OTel block,
  Codex `[otel]`, Codex `status_line`)

## 8. Compile flow (`scripts/compile.ps1`)

1. Load `.env`; resolve instances.
2. Render `.rulesync/` from templates: rules ← `policy/AGENTS.md`; hooks ← every manifest event mapped to
   `harness-hook <agent> <event>`.
3. Run `npx -y rulesync@${RULESYNC_VERSION} generate --global` for targets `codexcli`, `claudecode`.
4. Apply adapter `patches()` inside managed markers (`# >>> agent-harness managed >>>` … `<<<`, or a
   top-level `agentHarness`-owned key set for JSON); preserve all other keys.
5. Verify: JSON/TOML parse, hard-link identity and SHA-256 (reuses `sync_config.py verify`), managed-section
   integrity; print a change summary. Abort before writing on any parse failure; back up every file before write.

Open item (Phase 0 spike): whether rulesync global mode can target three Codex homes; fallback is generate once
and link into each home (current hard-link model). If rulesync fails the spike, compile falls back to generalizing
`sync_config.py` (Approach B) for the compile half only.

## 9. Consumers

- **obs** — maps common events to existing obs operations (`session.start`, `prompt.begin`, `execution.start`,
  `event.emit` for tools, `execution.finish`/`prompt.finish` on `turn.stop`, `session.end`). Instruction-load
  check reads `policy/AGENTS.md`.
- **usage** — one OTLP/HTTP JSON receiver (`/v1/logs`, `/health`, localhost only) started by `session.start`
  (`ensure`). Dispatch by `service.name` to the adapter's `otel.map`; one row per API request,
  `source = '<agent>_otel'`, `native_turn` = the agent's prompt/turn id, `usageId = uuid5(request id)`.
  Fallback on `turn.stop`: adapter transcript parser, `source = '<agent>_hook'` (carries reasoning tokens).
- **notify** — decision engine (ported from `notify-gate.ps1`): `turn.stop` with pending prompt and no
  outstanding background tasks → done; `attention.needed/permission` → always; `/idle` → defer while tasks run;
  rate limit. Delivery backends: Herdr (`herdr notification show --sound done|request`) + silent Windows toast
  when Herdr is available, Windows toast with sound otherwise; fallback on backend failure. Every decision is
  logged (`var/notify-events.jsonl`, bounded) and `notify_report.py` cross-references it. Replaces
  `codex_cli_notify.py` and `notify-gate.ps1`.
- **tasks** — per-session agent state from `subagent.start/stop`; background shells from the adapter's
  transcript scan; entries older than 3 h are ignored.
- **context** — session-start context from the hub (`/v1/projects/<id>/context/bootstrap`), returned through
  `adapter.respond` for every agent (Claude gains hub context).
- **herdr / orb** — moved with minimal change; consume common events. Herdr 0.9.1 ignores
  `pane report-agent` for `claude`/`codex` panes (documented limitation); session identity reporting remains.

## 10. Error handling and safety

- Router: per-consumer timeout, isolated failures, bounded lifecycle log, adapter `safe_default` on error (R3).
- Hub down → spool (existing); receiver down → exporters drop silently (verified for Claude), restarted on next
  `session.start`.
- Version drift: `session.start` compares the running agent version with `manifest.toml` tested versions and
  logs `agent.version.untested` (no blocking).
- PII: whitelist mapping only (R6); telemetry prompt/tool content stays redacted (agent defaults).
- Security: receiver binds `127.0.0.1` only; unauthenticated local ingestion is an accepted, documented risk.

## 11. Reuse decisions

| Need | Choice | Status (technology memory) |
|---|---|---|
| Compile to native agent files | `rulesync` 22.0.0 (MIT) | `rulesync`: untested (Phase 0 spike gates use) |
| Token usage | Claude Code / Codex built-in OpenTelemetry | `claude-code-opentelemetry`: untested |
| Agent lifecycle | Claude `SubagentStart/Stop` hooks | `claude-code-subagent-hooks`: untested |
| Ad-hoc usage/cost reports | `ccusage` 20.0.26 (MIT) | `ccusage`: untested |
| Notification logs | PowerShell/Python JSONL | `powershell-jsonl`: tested |
| Rejected | `ruler` (weaker hooks coverage), custom compiler (Approach B, fallback only) | — |

## 12. Testing

- Unit: adapters (fixture → event, `respond`, `safe_default`), notify decision table, tasks, usage mapping,
  compile renderer (golden files), router fan-out with fake consumers. Coverage ≥ 80%.
- Adapter conformance kit (`tests/conformance/`): one parametrized suite every adapter MUST pass, driven by
  sanitized real payload fixtures.
- Integration: install into a temporary `HOME`, compile, assert generated files and managed-section integrity.
- E2E: headless `claude -p` and `codex exec` → assert obs rows (events, `turn_usage`) and notify decisions.
- Regression: existing hub, notify, usage suites keep passing; CI `verify.yml` runs unit + conformance.

## 13. Migration (owner machine) and rollback

1. Back up `.codex-shared`, `~/.codex*` (`hooks.json`, `config.toml`, `AGENTS.md`), `~/.claude/settings.json`,
   `~/.claude/CLAUDE.md`.
2. Install alongside; `compile --dry-run`; compare with current configs.
3. Codex: move `.codex-shared` → `.agent-harness`, junction the old path, regenerate hooks to `harness-hook`;
   verify in a fresh Codex session.
4. Claude: replace ad-hoc shims with generated entries; `@import` → `~/.agent-harness/policy/AGENTS.md`;
   verify in a fresh Claude session and a subagent.
5. Rollback at any stage: restore backups, remove junction.

## 14. Rollout phases and exit criteria

| Phase | Deliverable | Exit criteria |
|---|---|---|
| 0 Spike | rulesync global × 3 Codex homes; Codex hooks on Windows; Codex OTel token events | written go/no-go per item |
| 1 Repo | `git init`, rename to `agent-harness` (GitHub creation/rename needs owner confirmation), scaffold, move tools unchanged | existing tests pass |
| 2 Router + obs (Codex) | `harness-hook`, events, Codex adapter, obs consumer | Codex parity in fresh session; conformance green |
| 3 Claude adapter | Claude adapter; shims retired | parity in fresh session + subagent |
| 4 Usage / notify / tasks | core consumers for both agents | `turn_usage` rows per agent; notify tests; Codex receives accurate done toasts |
| 5 Compile / install / migrate | `compile.ps1`, `install.ps1`, owner migration | temp-HOME install passes; owner machine migrated |
| 6 Docs | README, `adding-an-agent.md`, `.env.example` | a new adapter can be added from docs alone |

All phases follow the shared Global GitHub Save Policy and High-risk gate (router entry, receiver, and
security-boundary changes require explicit approval).

## 15. Risks and open questions

- rulesync coverage of Codex hooks / multiple homes on Windows (Phase 0).
- Codex OTel event names and token fields not yet verified (Phase 0).
- `ccusage` session totals ran ~3.5% above transcript/OTel output tokens (2026-09-28); unresolved, reports only.
- Hub gaps to fix or document: `obs.cmd tech|reuse` missing from the CLI (policy requires them);
  `obs.cmd change plan` resolves the active prompt by cwd; spool `dead_letter` at its 1000 cap.
- Herdr cannot take hook-reported state for Claude/Codex panes; banners stay screen-detected.

## 16. Sources

Claude Code hooks: https://code.claude.com/docs/en/hooks · Claude Code monitoring (OpenTelemetry):
https://code.claude.com/docs/en/monitoring-usage · Codex OTel crate: https://github.com/openai/codex/tree/main/codex-rs/otel ·
rulesync: https://github.com/dyoshikawa/rulesync · ruler: https://github.com/intellectronica/ruler ·
ccusage: https://github.com/ccusage/ccusage · Herdr docs: https://herdr.dev/llms.txt
