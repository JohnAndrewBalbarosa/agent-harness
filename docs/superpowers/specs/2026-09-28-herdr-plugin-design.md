# agent-harness Herdr plugin — design

Status: draft for owner review · 2026-09-28 · supersedes the `tools/herdr-bridge` plugin and the status orb.

## 1. Goal

Herdr is the owner's single UI for every agent. The harness (hooks router, notifications, local DB, profiles,
policy) MUST be operable and visible from Herdr as a plugin. Decisions already made by the owner:

- **Thin plugin.** Code, `.env`, DB/spool and logs stay in `HARNESS_HOME` (`~\.agent-harness`); the plugin is a
  manifest plus small entrypoints that call it. The harness MUST keep working without Herdr (e.g. `codex exec` in
  another terminal).
- **Retire the status orb.** Herdr is the status and sound authority. Orb-only behaviors that still matter are
  ported to plugin event handlers (§5).

Non-goals: moving hook execution or system-prompt loading into Herdr (impossible: agents run their own hooks and
read `CLAUDE.md` / `AGENTS.md` themselves); supporting Herdr < 0.9.1; Linux/macOS in v1.

## 2. Herdr capabilities used (0.9.1, from official plugin + socket API docs)

| Capability | Use |
|---|---|
| `[[startup]]` (one-shot per server start, async) | ensure harness services |
| `[[events]]` with `HERDR_PLUGIN_EVENT_JSON` | status/recovery handlers, obs event recording |
| `[[actions]]` (workspace/pane contexts) | start agent with profile, compile, doctor, notify report |
| `[[panes]]` (tab/overlay/popup) | harness dashboard |
| `HERDR_PLUGIN_CONFIG_DIR` / `HERDR_PLUGIN_STATE_DIR` | plugin-local pointer to `HARNESS_HOME`; handler cursors |
| `notification.show` (`herdr notification show --sound done\|request`) | the only notification delivery inside Herdr |
| `pane.report_metadata` / `agent.view.set` | show profile + last-turn tokens in the agent panel |
| `agent.start` | launch Codex/Claude in a pane with a profile |
| official integrations (`herdr integration install <agent>`) | Herdr-native agent session/state reporting |

## 3. Architecture

```
agent (Claude / Codex)
  └─ native hook ─► harness-hook <agent>  (unchanged: router + consumers, local DB, usage, notify decisions)
                        └─ herdr-bridge consumer (inside Herdr): official Herdr integration call + pane metadata
Herdr server
  ├─ plugin [[startup]]  ─► harness services bootstrap (receiver, hub, console monitor)
  ├─ plugin [[events]]   ─► harness herdr-events handler (status-driven recovery, obs recording)
  ├─ plugin [[actions]]  ─► start-agent(profile), compile, doctor, notify-report
  └─ plugin [[panes]]    ─► harness dashboard (tokens, notify decisions, tasks, obs errors)
```

Plugin id `agent-harness` (replaces `agent-harness.herdr-bridge`). Manifest lives in the repo at
`herdr-plugin/herdr-plugin.toml`; install = `herdr plugin link <HARNESS_HOME>\herdr-plugin`. Entrypoints resolve
`HARNESS_HOME` from `HERDR_PLUGIN_CONFIG_DIR\harness.env` (written by the installer), falling back to
`%USERPROFILE%\.agent-harness`.

## 4. Components

### 4.1 Notifications
- Decision logic stays in `core.notify` (defer while background tasks run, rate limit, dedupe).
- Inside Herdr, delivery is **Herdr only**: `herdr notification show --sound done|request`; no Windows toast.
  Outside Herdr, unchanged (Windows toast with sound).
- `.env` `NOTIFY_DELIVERY=auto|herdr|windows` (default `auto`) makes this explicit.

### 4.2 Status authority and sounds
- Herdr owns per-pane agent status (screen detection + official integrations). The harness never reports a
  competing status label (Herdr ignores custom labels for claude/codex anyway).
- Sounds come from Herdr (`ui.sound.*` + notification `--sound`). The orb's done bell/attention ping retire.

### 4.3 Official Codex integration without breaking shared hooks
`herdr integration install codex` edits Codex config files; the owner's profile `hooks.json` files are hard links
and are compile-managed. Therefore the harness MUST NOT let Herdr edit `hooks.json`. Instead:
1. Inspect the integration in a throwaway `CODEX_HOME` to capture its hook script and event set.
2. The `herdr-bridge` consumer invokes that script (same payload, same env) for Codex events, exactly like the
   Claude integration is invoked today. Version drift is detected by comparing the script's
   `HERDR_INTEGRATION_VERSION` header with `herdr integration status`.

### 4.4 Event handlers (orb logic port)
`[[events]]` handlers for `pane.agent_status_changed`, `pane.agent_detected`, `pane.exited`:
- **Dead session:** `pane.exited` for a pane bound to an agent session → clear that session's pending notify state
  and task records (orb `validate_processes` equivalent).
- **Missing Stop:** status change to idle/done while notify has a pending prompt for that session and no
  `turn.stop` arrived within a grace window → emit a synthetic `turn.stop` into `core.notify` (orb
  `recover_missing_stop_hooks` equivalent). Logged as `herdr.recovered_stop`.
- **Needs input:** Herdr status `needs_input`/blocked → no harness action (Herdr already alerts); recorded only.
- Every event is recorded to obs (replaces the `subscriber.py` daemon and its socket loop).
Pane ↔ session binding comes from `report-agent-session` data in the Herdr snapshot (`agent_session_id`).

### 4.5 Profiles
- Actions `start-codex-personal|cy|feu`, `start-claude`: `agent.start` in the current workspace with the profile env
  (`CODEX_HOME` for Codex). Profiles are read from `.env` `INSTANCES` (no hardcoded list).
- `pane.report_metadata` shows `profile` and last-turn tokens (from the usage fallback record) per agent pane.

### 4.5a Agent registry and launch preflight (owner requirement, 2026-09-28)
The middleware keeps a registry of agents: **supported** = an adapter exists (`adapters/<agent>/manifest.toml`);
**subscribed** = listed in `.env` `INSTANCES`. `harness agents` prints every supported/subscribed agent with its
status. `harness preflight <agent> [--instance <name>] [--init]` runs, in order, and logs each step to
`var/logs/preflight.lifecycle.jsonl`:

| Step | Check | Failure verdict |
|---|---|---|
| supported | adapter manifest exists | `unsupported` — print "agent-harness: <agent> is not supported" |
| installed | manifest `binary` resolves on PATH | `not_installed` |
| subscribed | instance in `INSTANCES` (`--init`: add it with the manifest's default home) | `not_subscribed` |
| configured | the agent's hook config contains `harness-hook <agent>` (`--init`: run compile) | `not_configured` |
| working | adapter round-trip on its own fixtures + shim/python resolve + no failed dispatch in the last 24 h of router logs | `broken: <reason>` |

Verdict `ready` or a failure is printed as one line and returned as an exit code (0 ready; 10–14 failures).
Herdr trigger: the plugin `[[events]]` handler for `pane.agent_detected` runs `preflight --init` for the detected
agent and shows the verdict as a Herdr notification (failures always; `ready` only on first detection per pane).
"Supported but not working" is always logged and printed, never silent.

### 4.6 Dashboard pane
A read-only TUI (Python, stdlib only) refreshing every 2 s: per-session last turn tokens (both sources, labelled),
notify decisions (last 20), outstanding background tasks, hub health, obs spool depth and recent errors. Bounded
reads only (tails / indexed queries); never prints prompts or tool payloads.

### 4.7 Services
`[[startup]]` runs `core.services.bootstrap` (receiver, hub via `HUB_DIR`, console monitor). The session-start
`services` consumer stays for sessions outside Herdr. The orb step is removed from the bootstrap.

## 5. Retirements
| Retired | Replacement |
|---|---|
| status orb (process, `ORB_UI`, orb hook consumer, orb venv step) | Herdr status + §4.4 handlers |
| `tools/herdr-bridge/subscriber.py` daemon + `ensure_subscriber` | plugin `[[events]]` |
| `agent-harness.herdr-bridge` plugin (dashboard.ps1, focus-priority.ps1) | `agent-harness` plugin |
The orb code is deleted from the repo (history keeps it); the vendored `codex-cli-notify` goes with it.

## 6. Error handling and observability
- Plugin entrypoints never fail Herdr: exit 0, log to `HARNESS_HOME\var\logs\herdr-plugin.lifecycle.jsonl`
  (bounded, rotated) with event name, pane id, outcome, duration.
- Herdr's own plugin command log (`herdr plugin log list`) stays the first place to look for launch failures.

## 7. Testing
- Unit: manifest validity (ids, contexts, commands exist), event handler decisions from recorded
  `HERDR_PLUGIN_EVENT_JSON` fixtures, notify delivery selection, profile action env, dashboard renderers.
- Integration (owner machine, High-risk gate): `herdr plugin link`, trigger each action, observe events via
  `herdr plugin log list`, verify one toast + bell per finished turn and zero Windows pop-ups inside Herdr.

## 8. Migration (owner)
1. Unlink `agent-harness.herdr-bridge`; link `agent-harness`.
2. Stop orb processes; remove orb consumer; delete `ORB_UI` from `.env`.
3. Codex integration via §4.3 (no `hooks.json` edits by Herdr).
4. Rollback: relink the old plugin; `git revert` the retirement commit; orb venv is recreated by the installer.

## 9. Open risks
- Herdr 0.9.1 plugin API is young; event payload shapes are validated against `herdr api schema --json` at link
  time and fixtures are regenerated from the live schema.
- Screen-detected status can lag; the missing-Stop grace window (default 20 s) trades latency for false positives.
