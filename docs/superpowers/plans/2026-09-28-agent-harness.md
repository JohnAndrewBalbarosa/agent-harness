# agent-harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the Codex-only shared runtime into `agent-harness`, one portable middleware that Codex CLI and Claude Code connect to through thin adapters, with compile (one source → native agent files) and aggregation (one router → shared consumers → one DB).

**Architecture:** Every agent hook calls `harness-hook <agent> [<native-event>]`; the agent's adapter normalizes the native payload into an `agent-harness.event/v1` common event; `core/router` fans it out (parallel, hidden, per-consumer timeout) to consumers and returns the adapter's native response. Token usage arrives through one OTLP receiver fed by each agent's built-in OpenTelemetry. `compile` renders one policy/config source into each agent's files (rulesync, with an in-repo fallback).

**Tech Stack:** Python 3.14 stdlib only (pinned uv interpreter, `unittest`), PowerShell 5.1 shims, `rulesync@22.0.0` via `npx` (compile only), existing observability hub (Node + Postgres, separate repo), OpenTelemetry OTLP/HTTP JSON.

**Spec:** `docs/superpowers/specs/2026-09-28-agent-harness-design.md`

## Global Constraints

- Windows-first; every path in generated files MUST be quoted (user profiles may contain spaces).
- Core code: Python stdlib only; interpreter from `HARNESS_PYTHON` (default `%APPDATA%\uv\python\cpython-3.14.7-windows-x86_64-none\python.exe`).
- Tests: `python -m unittest discover -s tests -t .` from the repo root; coverage target ≥ 80% for `core/` and `adapters/`.
- Hooks MUST NOT block agents: every failure path returns the adapter's `safe_default()` and exit code 0 (R3).
- Telemetry mapping MUST whitelist fields; never persist `user.*`, `organization.*`, account ids, emails (R6).
- Repo MUST NOT contain owner paths, credentials, prompts, logs, or personal policy (R5); `scripts/verify-secrets.ps1` MUST pass.
- Logs: bounded JSONL (≤ 1 MiB per file, rotated) or bounded record counts; no full-log reads.
- Ports: obs hub `4319`, OTLP receiver `4320`, both bound to `127.0.0.1`.
- Codex agent-instance IDs MUST stay `uuid5(URL, "codex-observability:<instance>")` (R9).
- Git: conventional commits, local only; creating/pushing a GitHub repo needs explicit owner confirmation.
- High-risk steps (Tasks 16–17: live hook switch-over, receiver takeover) need an explicit owner approval of a bounded verification request before execution.

## Review Focus

- Non-ASCII hook payloads (Taglish accents, emoji) through Windows stdin/stdout MUST round-trip as UTF-8 — Task 8 test `test_utf8_payload_round_trip`.
- Empty or malformed stdin JSON MUST yield the adapter safe default and exit 0 — Task 8 test `test_malformed_stdin_returns_safe_default`.
- Unknown/new native events (agent upgrades add events) MUST normalize to `None` and get the safe default — Tasks 5/6 test `test_unknown_event_is_ignored`.
- Parallel hooks (parallel tool calls) MUST NOT corrupt lifecycle logs or state files — Task 8 test `test_concurrent_log_appends_are_line_atomic`, Task 11 test `test_concurrent_state_updates`.
- `USERPROFILE` containing spaces MUST produce working quoted commands — Task 14 test `test_hook_commands_quote_paths_with_spaces`.

---

## File Structure

```
agent-harness/                      (renamed from codex-system-harness)
  .env.example                      Task 14
  .gitignore                        Task 2
  policy/AGENTS.template.md         Task 3 (moved from harness/templates/AGENTS.md.tmpl)
  core/__init__.py                  Task 3
  core/config.py                    Task 14  .env loading, instances
  core/events.py                    Task 4   Event dataclass, validate, (de)serialize
  core/lifecycle.py                 Task 8   bounded, line-atomic JSONL logger
  core/router/harness_hook.py       Task 8   entry point: normalize → fan-out → respond
  core/router/consumers.py          Task 8   consumer registry (event → consumers)
  core/router/harness-hook.cmd      Task 9   Windows shim
  core/notify/decide.py             Task 10  pure decision engine
  core/notify/deliver.py            Task 10  herdr + windows backends
  core/notify/toast.ps1             Task 10  (moved from ~/.claude/scripts/ai-notify.ps1)
  core/notify/consumer.py           Task 10  consumer entry
  core/notify/report.py             Task 10  (moved from ~/.claude/scripts/notify_report.py)
  core/tasks/state.py               Task 11  per-session subagent state
  core/tasks/consumer.py            Task 11
  core/usage/receiver.py            Task 12  OTLP receiver (generalized claude_otel_receiver)
  core/usage/fallback.py            Task 12  turn.stop transcript fallback consumer
  core/consumers/obs.py             Task 13  common event → obs hook.py (legacy consumer bridge)
  core/context/consumer.py          Task 13  session-start context injection
  adapters/base.py                  Task 5   Adapter protocol + TurnUsage
  adapters/codex/{adapter.py,manifest.toml,otel.py,transcript.py,patches.py}   Tasks 5, 12, 15
  adapters/claude/{adapter.py,manifest.toml,otel.py,transcript.py,patches.py}  Tasks 6, 12, 15
  adapters/*/fixtures/*.json        Tasks 5, 6
  compile/render.py                 Task 15  .rulesync + fallback native rendering
  compile/patch.py                  Task 15  managed-section merge (JSON / TOML text)
  compile/run.py                    Task 15  orchestrates, --dry-run, verify
  scripts/compile.ps1               Task 15
  scripts/install.ps1               Task 16 (modified)
  scripts/migrate.ps1               Task 17
  tools/                            Task 2   vendored, unchanged: interval-supervisor, codex-status-orb, herdr-bridge, observability-hub CLI
  tests/unit/, tests/conformance/, tests/integration/, tests/e2e/
  docs/adding-an-agent.md, docs/architecture.md    Task 18
```

---

### Task 1: Phase 0 spike (go/no-go)

**Files:**
- Create: `docs/superpowers/spikes/2026-09-28-phase0.md`

**Interfaces:**
- Produces: decisions consumed by Tasks 5, 12, 15 — (a) rulesync usable for hooks of `codexcli` + `claudecode` in global mode and for 3 Codex homes (yes/no + method), (b) Codex hook payload field names per event, (c) Codex OTel token event name + attribute names.

- [ ] **Step 1: rulesync global generation in a throwaway HOME**

```bash
T="$TEMP/ah-spike-rulesync"; rm -rf "$T"; mkdir -p "$T/home" "$T/src/.rulesync/rules"
printf -- "---\nroot: true\ntargets: [\"*\"]\n---\n# Policy\nTest rule.\n" > "$T/src/.rulesync/rules/overview.md"
cd "$T/src" && HOME="$T/home" USERPROFILE="$T/home" npx -y rulesync@22.0.0 init >/dev/null 2>&1
cd "$T/src" && HOME="$T/home" USERPROFILE="$T/home" npx -y rulesync@22.0.0 generate --global --targets codexcli,claudecode --features rules,hooks 2>&1 | tail -20
find "$T/home" -maxdepth 3 -type f | head -30
```
Expected: files under `$T/home/.codex` and `$T/home/.claude`. Record paths, and how a hooks source is expressed (read `npx -y rulesync@22.0.0 --help` and the generated `.rulesync` layout after `init`).

- [ ] **Step 2: Hooks format + multi-home**

Add a hook source per rulesync docs (e.g. `.rulesync/hooks.json`) with `SessionStart` → `harness-hook codex`; regenerate; inspect generated `~/.codex/hooks.json` and `~/.claude/settings.json`. Test a second Codex home by running with a different `HOME` (or rulesync output-dir option if one exists). Record: supported, needs link-copy, or unsupported.

- [ ] **Step 3: Codex hook payload schema (read-only, from source)**

```bash
gh search code "hook_event_name" --repo openai/codex --limit 20 --json path --jq '.[].path'
```
Open the Rust serde structs for hook inputs; record exact field names for SessionStart, UserPromptSubmit, PostToolUse, Stop, SubagentStop, SessionEnd, PermissionRequest, and the documented stdout contract (e.g. `{"continue":true}`, additionalContext envelope).

- [ ] **Step 4: Codex OTel token events (read-only, from source)**

```bash
gh search code "token" --repo openai/codex --limit 30 --json path --jq '.[].path' | grep -i otel
```
Record the event name carrying per-request token counts and its attribute names; record the `[otel]` config.toml shape (exporter `otlp-http`, `protocol = "json"`, endpoint).

- [ ] **Step 5: Write the spike report with go/no-go per item and the chosen method; delete `$T`.**

---

### Task 2: Repository bootstrap

**Files:**
- Rename directory: `Projects/codex-system-harness` → `Projects/agent-harness`
- Create: `.gitignore`
- Create: `tools/` (vendored copy of live runtime tools)
- Modify: `harness/` → removed after its contents move (Task 3)

**Interfaces:**
- Produces: a Git repository whose existing tests pass; vendored tools at `tools/<name>/`.

- [ ] **Step 1: Rename and initialize**

```bash
cd $HOME/Desktop/Projects && mv codex-system-harness agent-harness && cd agent-harness && git init -b main
```

- [ ] **Step 2: `.gitignore`**

```gitignore
.env
var/
**/__pycache__/
**/.venv/
*.bak-*
*.log
*.sqlite3*
node_modules/
```

- [ ] **Step 3: Vendor the live tools (latest versions; repo copies are stale)**

```bash
S=$HOME/.codex-shared/tools
for t in codex-status-orb herdr-bridge global-hook-router observability-hub; do
  mkdir -p tools/$t
  (cd $S/$t && find . -type f ! -path "*/var/*" ! -path "*/.venv/*" ! -path "*/__pycache__/*" ! -name "*.bak-*" ! -name "*.log") | while read f; do mkdir -p "tools/$t/$(dirname "$f")"; cp "$S/$t/$f" "tools/$t/$f"; done
done
rm -rf harness/tools
```

- [ ] **Step 4: Verify no secrets / owner paths, run existing tests**

```bash
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify-secrets.ps1
grep -rIl "C:\|C:\\\\Users\\\\" --exclude-dir=.git . | grep -v "docs/superpowers" || echo "no owner paths"
python -m unittest discover -s tests -t . 2>&1 | tail -3
```
Expected: verify-secrets passes; owner-path hits only in docs (fix any others by parameterizing on `Path.home()` / `%USERPROFILE%`); tests OK.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "chore: import codex-system-harness and latest shared runtime tools"
```

---

### Task 3: Layout skeleton + policy template

**Files:**
- Create: `core/__init__.py`, `core/router/__init__.py`, `core/notify/__init__.py`, `core/tasks/__init__.py`, `core/usage/__init__.py`, `core/consumers/__init__.py`, `core/context/__init__.py`, `adapters/__init__.py`, `adapters/codex/__init__.py`, `adapters/claude/__init__.py`, `compile/__init__.py`, `tests/__init__.py`, `tests/unit/__init__.py`, `tests/conformance/__init__.py`, `tests/integration/__init__.py`
- Move: `harness/templates/AGENTS.md.tmpl` → `policy/AGENTS.template.md`; `harness/templates/hooks.json.tmpl`, `harness/templates/config/` → `compile/templates/`

- [ ] **Step 1: Create packages and move templates (git mv), replacing "Codex" wording in `policy/AGENTS.template.md` with agent-neutral wording ("the agent"), keeping every rule.**
- [ ] **Step 2: Run `python -m unittest discover -s tests -t .` → OK.**
- [ ] **Step 3: Commit `refactor: agent-neutral layout and policy template`.**

---

### Task 4: Common event contract

**Files:**
- Create: `core/events.py`
- Test: `tests/unit/test_events.py`

**Interfaces:**
- Produces:
  - `SCHEMA = "agent-harness.event/v1"`
  - `EVENT_TYPES = frozenset({"session.start","session.end","prompt.submit","tool.completed","turn.stop","attention.needed","subagent.start","subagent.stop"})`
  - `@dataclass(frozen=True) class Event: event: str; agent: str; instance: str; session_id: str; cwd: str | None; transcript_path: str | None; ts: str; turn_id: str | None = None; data: Mapping[str, Any] = field(default_factory=dict); native_event: str = ""; native_payload: Mapping[str, Any] = field(default_factory=dict)`
  - `def make_event(event: str, agent: str, instance: str, payload: Mapping[str, Any], native_event: str, data: Mapping[str, Any] | None = None) -> Event` — fills `session_id`, `cwd`, `transcript_path` from snake_case payload keys, `ts` = UTC now, bounds `native_payload` to 8 KiB (drops it when larger and sets `data["native_truncated"]=True`)
  - `def to_json(event: Event) -> str` / `def from_json(text: str) -> Event` (round-trip; raises `ValueError` on schema mismatch or unknown event)

- [ ] **Step 1: Write the failing tests**

```python
import json, unittest
from core import events as ev

class EventTests(unittest.TestCase):
    def test_make_event_fills_common_fields(self):
        e = ev.make_event("turn.stop", "claude", "default",
                          {"session_id": "s1", "cwd": "grep -rIl "C:/p", "transcript_path": "C:/t.jsonl"}, "Stop")
        self.assertEqual((e.session_id, e.cwd, e.transcript_path, e.native_event), ("s1", "grep -rIl "C:/p", "C:/t.jsonl", "Stop"))
        self.assertTrue(e.ts.endswith("Z"))

    def test_round_trip(self):
        e = ev.make_event("prompt.submit", "codex", "personal", {"session_id": "s"}, "UserPromptSubmit", {"prompt": "ñ 🙂"})
        self.assertEqual(ev.from_json(ev.to_json(e)), e)
        self.assertEqual(json.loads(ev.to_json(e))["schema"], ev.SCHEMA)

    def test_unknown_event_rejected(self):
        with self.assertRaises(ValueError):
            ev.make_event("bogus", "codex", "personal", {}, "X")

    def test_large_native_payload_is_dropped(self):
        e = ev.make_event("tool.completed", "codex", "personal", {"session_id": "s", "blob": "x" * 20000}, "PostToolUse")
        self.assertEqual(e.native_payload, {})
        self.assertTrue(e.data["native_truncated"])

    def test_event_is_immutable(self):
        e = ev.make_event("turn.stop", "codex", "personal", {"session_id": "s"}, "Stop")
        with self.assertRaises(Exception):
            e.agent = "x"  # type: ignore[misc]
```

- [ ] **Step 2: `python -m unittest tests.unit.test_events -v` → FAIL (module missing).**
- [ ] **Step 3: Implement `core/events.py` per Interfaces (json.dumps with `ensure_ascii=False`; `MappingProxyType` for data/native_payload).**
- [ ] **Step 4: Re-run → PASS.**
- [ ] **Step 5: Commit `feat(core): common event contract v1`.**

---

### Task 5: Adapter protocol + Codex adapter

**Files:**
- Create: `adapters/base.py`, `adapters/codex/adapter.py`, `adapters/codex/manifest.toml`, `adapters/codex/fixtures/*.json` (one per native event, field names from Task 1 Step 3; no real prompts/ids)
- Test: `tests/unit/test_codex_adapter.py`

**Interfaces:**
- Produces (`adapters/base.py`):
  - `@dataclass(frozen=True) class TurnUsage: input_tokens: int; output_tokens: int; cached_input_tokens: int; reasoning_tokens: int | None; api_calls: int; turn_id: str | None`
  - `class Adapter(Protocol): name: str; def instance(self, env: Mapping[str,str]) -> str; def normalize(self, native_event: str, payload: Mapping[str, Any]) -> Event | None; def respond(self, event: Event, results: Sequence[ConsumerResult]) -> str; def safe_default(self, native_event: str) -> str`
  - `@dataclass(frozen=True) class ConsumerResult: name: str; exit_code: int; stdout: bytes`
  - `def load_adapter(name: str) -> Adapter` (imports `adapters.<name>.adapter.ADAPTER`; raises `KeyError` for unknown)
- Codex specifics: native event read from `payload["hook_event_name"]` when argv omits it; instance from `CODEX_HOME` leaf (`.codex`→`personal`, `.codex-cy`→`cy`, `.codex-feu`→`feu`, else leaf name); `PermissionRequest` → `attention.needed` `{"kind":"permission"}`; `respond`: first non-empty stdout of consumer `context` or `herdr-bridge`, else `{"continue":true}` for Stop, else `""`; `safe_default("Stop") == '{"continue":true}'`.

- [ ] **Step 1: Failing tests** — for every fixture: `normalize` returns the mapped common event with `agent="codex"`; `test_unknown_event_is_ignored` (`"FutureEvent"` → `None`); `test_instance_from_codex_home` (three homes + fallback); `test_respond_prefers_context_output`; `test_safe_default_stop_continues`.
- [ ] **Step 2: Run → FAIL.**  **Step 3: Implement.**  **Step 4: Run → PASS.**
- [ ] **Step 5: Commit `feat(adapters): codex adapter and adapter protocol`.**

---

### Task 6: Claude adapter

**Files:**
- Create: `adapters/claude/adapter.py`, `adapters/claude/manifest.toml`, `adapters/claude/fixtures/*.json`
- Test: `tests/unit/test_claude_adapter.py`

**Interfaces:**
- Consumes: `adapters.base` (Task 5), `core.events.make_event` (Task 4).
- Claude specifics: instance `"default"` (overridable by `AH_INSTANCE`); `Notification` → `attention.needed` with `kind` = `permission` for `notification_type == "permission_prompt"`, `idle` for `idle_prompt`, else ignored (`None`); `SubagentStart/Stop` → `subagent.start/stop` with `data["subagent"] = {"id": agent_id, "type": agent_type}`; `Stop` data `last_message` from `last_assistant_message`; `respond`: `session.start` with context output → `{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext": <text>}}`, else `""`; `safe_default` always `""`.

- [ ] **Step 1: Failing tests** — fixtures per event incl. both notification types and an unrecognized type; `test_unknown_event_is_ignored` (`"PostToolBatch"` → `None`); `test_session_start_context_envelope`.
- [ ] **Steps 2–4: FAIL → implement → PASS.**
- [ ] **Step 5: Commit `feat(adapters): claude adapter`.**

---

### Task 7: Adapter conformance kit

**Files:**
- Create: `tests/conformance/test_adapter_contract.py`

**Interfaces:**
- Consumes: `load_adapter`, adapter `fixtures/` dirs, `core.events`.

- [ ] **Step 1: Parametrized suite over `["codex", "claude"]` (subTest):** every fixture normalizes to `None` or an `Event` whose `event ∈ EVENT_TYPES`, `agent == name`, JSON round-trips; `safe_default` returns `str` for every native event in `manifest.toml` and for `"FutureEvent"`; `respond` never raises with empty results; manifest has keys `agent`, `events`, `rulesync_target`, `tested_versions`.
- [ ] **Step 2: Run → PASS (both adapters).**
- [ ] **Step 3: Commit `test: adapter conformance kit`.**

---

### Task 8: Router (`harness-hook`)

**Files:**
- Create: `core/lifecycle.py`, `core/router/consumers.py`, `core/router/harness_hook.py`
- Test: `tests/unit/test_lifecycle.py`, `tests/unit/test_router.py`

**Interfaces:**
- Consumes: `load_adapter` (Task 5), `Event`/`to_json` (Task 4).
- Produces:
  - `core.lifecycle.emit(path: Path, component: str, operation: str, outcome: str, **details) -> None` — one `os.write` per record on an `O_APPEND` fd (line-atomic), rotate at 1 MiB keeping 2 backups (reuse the router/monitor pattern).
  - `core.router.consumers.Consumer(name: str, events: frozenset[str], command: list[str], stdin: Literal["event","native"], timeout_s: int)`; `def consumers_for(event: Event, env: Mapping[str,str]) -> list[Consumer]` — registry: `obs` (native, all but subagent.*), `orb` (native), `herdr-bridge` (native; only when `HERDR_BIN` set), `notify` (event: turn.stop, attention.needed, prompt.submit), `tasks` (event: subagent.*), `usage-fallback` (event: turn.stop), `context` (event: session.start).
  - `def run(agent: str, native_event: str | None, stdin: bytes, env: Mapping[str,str]) -> tuple[str, int]` — returns (stdout text, exit code 0); parallel `subprocess.Popen(..., creationflags=CREATE_NO_WINDOW)`; per-consumer timeout; every exception → `adapter.safe_default`.
  - `main(argv) -> int` — `harness-hook <agent> [<native-event>]`, stdin/stdout as UTF-8 bytes.
  - On `session.start`, compare the payload/agent version (when present, e.g. Claude `version`, Codex `CODEX_VERSION` env or payload field from Task 1) with `manifest.toml` `tested_versions`; mismatch → `emit(..., "agent.version.untested", "observed", agent=…, version=…)` (spec §10, never blocks). Test: `test_untested_version_is_logged_not_blocked`.

- [ ] **Step 1: Failing tests (fake consumers = small Python scripts under `tests/unit/fakes/`)**

```python
class RouterTests(unittest.TestCase):
    def test_malformed_stdin_returns_safe_default(self):
        out, code = router.run("codex", "Stop", b"{not json", {})
        self.assertEqual((out.strip(), code), ('{"continue":true}', 0))

    def test_utf8_payload_round_trip(self):
        payload = json.dumps({"hook_event_name": "UserPromptSubmit", "session_id": "s", "prompt": "ñá 🙂"}).encode()
        with fake_consumer("echo-event") as seen:
            router.run("claude", "UserPromptSubmit", payload, {})
        self.assertEqual(json.loads(seen.read_text(encoding="utf-8"))["data"]["prompt"], "ñá 🙂")

    def test_consumer_timeout_does_not_block(self):
        with fake_consumer("sleep-60", timeout_s=1):
            started = time.perf_counter(); router.run("claude", "Stop", b'{"session_id":"s"}', {})
        self.assertLess(time.perf_counter() - started, 5)

    def test_unknown_agent_exits_zero(self):
        self.assertEqual(router.main(["nope", "Stop"]), 0)
```
Plus `test_concurrent_log_appends_are_line_atomic` in `test_lifecycle.py` (20 threads × 50 emits → every line parses as JSON).
- [ ] **Steps 2–4: FAIL → implement → PASS.**
- [ ] **Step 5: Commit `feat(core): harness-hook router with fail-open fan-out`.**

---

### Task 9: Windows entry shim

**Files:**
- Create: `core/router/harness-hook.cmd`

```bat
@echo off
setlocal
if not defined HARNESS_PYTHON set "HARNESS_PYTHON=%APPDATA%\uv\python\cpython-3.14.7-windows-x86_64-none\python.exe"
set "PYTHONUTF8=1"
"%HARNESS_PYTHON%" -m core.router.harness_hook %* 2>nul
exit /b 0
```
(`cd /d "%~dp0\..\.."` before the call so `core` is importable.)

- [ ] **Step 1: Smoke test** — `printf '{"hook_event_name":"Stop","session_id":"s"}' | core/router/harness-hook.cmd codex` prints `{"continue":true}`.
- [ ] **Step 2: Commit `feat(core): windows harness-hook shim`.**

---

### Task 10: Notify consumer (port of notify-gate)

**Files:**
- Create: `core/notify/decide.py`, `core/notify/deliver.py`, `core/notify/consumer.py`, `core/notify/report.py` (moved), `core/notify/toast.ps1` (moved `ai-notify.ps1`)
- Test: `tests/unit/test_notify_decide.py`, `tests/unit/test_notify_report.py` (moved + adapted)

**Interfaces:**
- Produces:
  - `@dataclass(frozen=True) class NotifyState: pending: frozenset[str]; recent_toasts: tuple[int, ...]`
  - `def decide(event: Event, state: NotifyState, outstanding_tasks: Sequence[str], now_s: int, rate: tuple[int,int]) -> tuple[Decision, NotifyState]` where `Decision(action: Literal["toast","defer","skip","mark"], reason: str, sound: Literal["done","request"] | None, message: str | None)`
  - `def deliver(decision: Decision, herdr_available: bool) -> str` — Herdr `notification show` + silent Windows toast, else Windows toast with sound; fallback on failure; returns result string as today.
- Rules (from spec §9): prompt.submit → mark pending; turn.stop with pending & no outstanding → toast done; turn.stop without pending → skip:no-pending-prompt; turn.stop with outstanding → defer (keep pending); attention permission → toast request; attention idle with outstanding → defer; rate limit → skip:rate-limited.

- [ ] **Step 1: Failing table-driven tests covering each rule above (port the 4 end-to-end scenarios verified on 2026-09-28).**
- [ ] **Steps 2–4: FAIL → implement (state file `var/notify/<agent>-<session>.json`, atomic replace; events log `var/notify-events.jsonl` bounded 1000) → PASS.**
- [ ] **Step 5: Commit `feat(notify): agent-neutral notification consumer`.**

---

### Task 11: Tasks consumer (background work)

**Files:**
- Create: `core/tasks/state.py` (from `~/.claude/scripts/background_tasks.py`: `apply_hook_event`, `outstanding_combined`, `scan`), `core/tasks/consumer.py`
- Test: `tests/unit/test_tasks.py` (port 14 existing tests + `test_concurrent_state_updates`: 10 processes start/stop distinct agents → final state consistent)

**Interfaces:**
- Produces: `def outstanding(agent: str, session_id: str, transcript_path: str | None, now: datetime) -> list[str]` (used by notify); state dir `var/tasks/`; writes under a per-session lock file (`msvcrt.locking`) + atomic replace.

- [ ] **Steps: failing tests → implement → PASS → commit `feat(tasks): agent-neutral background task tracking`.**

---

### Task 12: Usage (OTLP receiver + fallbacks)

**Files:**
- Create: `core/usage/receiver.py` (from `claude_otel_receiver.py`, dispatch by resource `service.name` → adapter `otel.map`), `core/usage/fallback.py`, `adapters/claude/otel.py`, `adapters/claude/transcript.py` (from `claude_turn_usage.py`), `adapters/codex/otel.py` (event/attrs from Task 1 Step 4), `adapters/codex/transcript.py` (rollout `token_count.info.last_token_usage` summed since the last user turn; `reasoning_output_tokens` → reasoning)
- Test: `tests/unit/test_usage_receiver.py` (port 9), `tests/unit/test_claude_transcript.py` (port 14), `tests/unit/test_codex_transcript.py`, `tests/unit/test_codex_otel.py`

**Interfaces:**
- Produces: `otel.map(attrs: Mapping[str, Any]) -> dict | None` (whitelisted `turn-usage.record` data, `source="<agent>_otel"`); `transcript.usage(path: Path, session_id: str) -> TurnUsage`; receiver `ensure()`/`serve()` unchanged in behavior; fallback consumer enqueues `source="<agent>_hook"`.
- Codex transcript test fixture: two `token_count` events after a user turn with `last_token_usage` `{input_tokens:100,cached_input_tokens:60,output_tokens:10,reasoning_output_tokens:4}` and `{…200,120,20,6}` → `TurnUsage(input_tokens=300, cached_input_tokens=180, output_tokens=30, reasoning_tokens=10, api_calls=2)`.

- [ ] **Steps: failing tests → implement → PASS → commit `feat(usage): multi-agent OTLP receiver and transcript fallbacks`.**

---

### Task 13: obs + context consumers

**Files:**
- Create: `core/consumers/obs.py` (sets `CODEX_OBS_INSTANCE` = instance for codex / `claude` for claude, `CODEX_OBS_HOME` = `HARNESS_HOME/policy`, `AH_AGENT_DISPLAY`, then pipes the native payload to `tools/observability-hub/hook.py`), `core/context/consumer.py` (imports `build_context` from `tools/herdr-bridge/hook.py`; prints context text)
- Modify: `tools/observability-hub/hook.py` — display name from `AH_AGENT_DISPLAY` when set (default unchanged), instruction check file name from `AH_POLICY_FILE` (default `AGENTS.md`)
- Test: `tests/unit/test_obs_consumer.py` (env mapping; Codex IDs unchanged — `uuid5(URL,"codex-observability:personal")`), `tests/unit/test_context_consumer.py` (hub unreachable → empty output, exit 0)

- [ ] **Steps: failing tests → implement → PASS → commit `feat(core): obs and context consumers`.**

---

### Task 14: Configuration

**Files:**
- Create: `core/config.py`, `.env.example`
- Test: `tests/unit/test_config.py`

**Interfaces:**
- Produces: `@dataclass(frozen=True) class Config: harness_home: Path; python: Path; instances: tuple[Instance, ...]; obs_endpoint: str; otlp_port: int; notify_rate: tuple[int,int]; herdr_bin: Path | None; rulesync_version: str`; `Instance(agent: str, name: str, home: Path)`; `def load(env_file: Path, environ: Mapping[str,str]) -> Config` (expands `%VAR%`; environment overrides file; missing required keys → `ValueError` naming the key); `def hook_command(cfg: Config, agent: str, native_event: str | None) -> str` (always quoted).
- Tests include `test_hook_commands_quote_paths_with_spaces` (`HARNESS_HOME=grep -rIl "C:\Users\Juan dela Cruz\.agent-harness`).

- [ ] **Steps: failing tests → implement → PASS → commit `feat(config): .env-driven configuration`.**

---

### Task 15: Compile

**Files:**
- Create: `compile/render.py`, `compile/patch.py`, `compile/run.py`, `adapters/codex/patches.py`, `adapters/claude/patches.py`, `scripts/compile.ps1`
- Test: `tests/unit/test_patch.py`, `tests/integration/test_compile_temp_home.py`

**Interfaces:**
- Produces: `patch.merge_json(existing: dict, managed: dict, owner_key: str = "agentHarness") -> dict` (returns new dict; only keys listed in `existing[owner_key]["managedKeys"]` or new managed keys change; hooks arrays: managed entries identified by command containing `harness-hook`); `patch.merge_text(existing: str, block: str, begin: str, end: str) -> str` (replace between markers or append); `run.compile(cfg: Config, dry_run: bool) -> list[Change]` (`Change(path: Path, before: str, after: str)`), writes backups `*.bak-agent-harness-<ts>` before any write, aborts on parse errors.
- Rendering: if Task 1 says rulesync is GO → render `.rulesync/` (rules from `HARNESS_HOME/policy/AGENTS.md`, hooks → `hook_command`) and run `npx -y rulesync@<ver> generate --global …`; else → render natively (Codex `hooks.json` from `compile/templates/hooks.json.tmpl` pointing to `harness-hook codex`, Claude `settings.json` hooks via `merge_json`, Claude `CLAUDE.md` managed block with `@<HARNESS_HOME>/policy/AGENTS.md`, Codex `AGENTS.md` hard links as today). Adapter `patches()` always apply (Claude `env` OTel block, Codex `[otel]` block).
- Integration test: temp HOME with pre-existing unrelated keys in `settings.json`/`config.toml` → after compile the unrelated keys are byte-identical; `--dry-run` writes nothing; second run produces zero changes (idempotent).

- [ ] **Steps: failing tests → implement → PASS → commit `feat(compile): render agent-native config from one source`.**

---

### Task 16: Installer (HIGH-RISK GATE before running against the owner's real homes)

**Files:**
- Modify: `scripts/install.ps1` (install to `HARNESS_HOME`, create `.env` from `.env.example` interactively or via `-EnvFile`, copy `policy/AGENTS.template.md` → `policy/AGENTS.md` only if absent, run compile, start obs hub (`OBS_START_SCRIPT` if set), `usage.receiver ensure`, health checks)
- Modify: `scripts/doctor.ps1` (health of router shim, receiver, hub, generated files)
- Test: `tests/test_installer.ps1` (extend: temp HOME install, idempotent re-install)

- [ ] **Steps: extend installer test → implement → run in temp HOME → commit `feat(install): agent-harness installer`.**

---

### Task 17: Owner migration (HIGH-RISK GATE — present verification request, wait for approval)

**Files:**
- Create: `scripts/migrate.ps1` (stages per spec §13, each stage idempotent, `-Stage backup|install|codex|claude|rollback`)

- [ ] **Step 1: Present bounded verification request (scope, files, risk flags, writes, checks, rollback) and wait.**
- [ ] **Step 2: `-Stage backup`, `-Stage install` (side-by-side), `compile --dry-run` diff review.**
- [ ] **Step 3: `-Stage codex` → verify fresh `codex exec` session: obs rows, notify decision, `{"continue":true}` on Stop.**
- [ ] **Step 4: `-Stage claude` → retire `~/.claude` shims (obs-hook.ps1, notify-gate.ps1, background_tasks hook, claude_otel_receiver ensure) in favor of generated entries → verify fresh `claude -p` session and a subagent.**
- [ ] **Step 5: Commit `feat(migrate): staged owner migration with rollback`.**

---

### Task 18: Docs + CI

**Files:**
- Modify: `README.md` (agent-harness), `.github/workflows/verify.yml` (run unittest discover + conformance + verify-secrets)
- Create: `docs/architecture.md`, `docs/adding-an-agent.md` (adapter checklist = conformance kit + manifest + rulesync target + OTel map)

- [ ] **Steps: write docs → run full suite + verify-secrets → commit `docs: agent-harness architecture and adapter guide`.**
