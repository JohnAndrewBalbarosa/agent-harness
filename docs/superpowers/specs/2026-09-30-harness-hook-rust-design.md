# Hook path in Rust: one `harness-hook.exe` with in-process consumers (sub-project #2)

Status: approved direction 2026-09-30 ("lahat ay migrate mo na sa rust"); compiler = Docker (user choice).
Contract map: research of 2026-09-30 (router, adapters, 9 consumers, shared state) summarized below.

## Purpose

Every agent hook currently starts 2-5 Python processes (router, then one per consumer; `obs` alone chains three) and
the `obs` consumer does a synchronous HTTP flush per call. Measured cost: hook dispatch p50 535 ms, p95 ~2 s.
Replace the whole hook path with one Rust binary whose consumers run in-process on threads.

- Target: `tool.completed` dispatch p50 <= 30 ms, p95 <= 150 ms (excluding `turn.stop` git/toast work).
- No behaviour change visible to Claude Code / Codex: identical stdout bytes, exit code 0, fail-open.
- Language floor: Rust. External CLIs stay external (`git`, `herdr`); nothing new in Python.

## Scope

| In | Out (later sub-projects) |
|---|---|
| Router, `.env` runtime merge, lifecycle log, adapters (claude, codex) incl. respond/safe_default | Services daemon: usage receiver, console monitor, Herdr subscriber, bootstrap (#3) |
| Consumers: obs, orb (hook side), herdr-bridge (hook side), notify, tasks, usage-fallback, context-guard, context, services (launch only) | Status orb GUI `orb.py` (#5) |
| Shared crates extracted from `obs-cli`: Python-compat JSON/redaction/uuid/time, spool access | Remaining `obs.py` subcommands (#4), compile/sync (#6) |

## Compatibility contract

Source of truth: `core/router/*`, `core/events.py`, `adapters/*`, the consumer modules, and their tests.

- Stdout: `SessionStart` → `{"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": ...}}`
  (Python default separators, `ensure_ascii=False`) from the context consumer, else herdr-bridge stdout stripped;
  Codex `Stop` → `{"continue": true}`; otherwise empty. No trailing newline. Always exit 0.
- Event JSON (`agent-harness.event/v1`), normalize table, instance naming, 8 KiB native-payload drop.
- Every file another process reads keeps its exact format: spool SQLite schema and operation JSON, `hmac.key`,
  orb `sessions/<sid>.json` (+ `.dedupe`, `.lock`), herdr-bridge session JSON, notify state/rate files, tasks files,
  context-guard files, lifecycle JSONL (compact, locked, 1 MiB rotation x2), usage JSONL, hook-invocations audit.
- Locks: sidecar `<path>.lock`, `LockFileEx` on byte 0 length 1 (interoperates with `msvcrt.locking`).
- The orb dedupe fingerprint must match Python's `json.dumps(..., sort_keys=True, ensure_ascii=True,
  separators=(",", ":"), default=str)` byte for byte.

Deliberate deviations (documented in UPDATES.md):
- `obs`: no synchronous flush inside the hook. Operations are enqueued, then one detached `obs-rs flush` runs
  (single-instance). Removes up to 15 s blocking when the hub is slow.
- herdr-bridge: enqueue into the real spool (the Python version imports a non-existent `obs.py`, so its
  `herdr.binding` / `agent-state.record` / `turn-usage.record` never reached the hub).
- PermissionRequest 590 s consumer timeout dropped (the orb never decides permissions; its stdout is unused).
- Consumers are threads: a consumer exceeding its timeout is abandoned (logged `consumer.timeout`, exit 124
  semantics) and the process exits after writing stdout.

## Architecture

```
crates/
├─ py-compat/        Python-compatible text: json.dumps variants, redaction, uuid5/uuid7, isoformat (from obs-cli)
├─ obs-spool/        spool open/queue/context/hmac + enqueue (from obs-cli), shared by obs-rs and harness-hook
└─ harness-hook/     binary `harness-hook`
   ├─ main.rs              argv → run → stdout; never panics outward (catch_unwind), exit 0
   ├─ router/              payload parse, dispatch, per-consumer timeouts, lifecycle records
   ├─ runtime_env.rs       .env runtime keys merge (%VAR% expansion)
   ├─ events.rs            Event + normalize table + 8 KiB rule
   ├─ adapters/            claude.rs, codex.rs (instance naming, respond, safe_default)
   ├─ shared_state/        file_lock.rs (LockFileEx), lifecycle_log.rs, atomic_write.rs
   └─ consumers/           obs/, orb/, herdr_bridge/, notify/, tasks.rs, usage_fallback/, context_guard.rs,
                           context.rs, services.rs
```

- `harness-hook.cmd` keeps its name and arguments; it execs `bin\harness-hook.exe` when present, else Python.
- Windows APIs via the `windows` crate (process snapshot for orb ancestry, LockFileEx,
  SHQueryUserNotificationState). Toasts keep the existing `toast.ps1` invocation for identical AUMID behaviour.
- Git calls stay `git` CLI (same commands as obs.py) to keep identical fingerprints.

## Verification (global "Implementation and verification order")

1. Implement everything; unit tests per module ported from the Python tests listed in the contract map.
2. fmt, clippy -D warnings, cargo test (Docker).
3. Whole-system parity (`tests/system/hook_parity.py`, one-time, removed after cutover): for every fixture payload
   of both agents and scripted multi-event sessions, run the Python router and the Rust binary in two sandboxed
   harness roots (temp `AH_*` dirs, dry-run delivery, fake `herdr`/`git`-free cwd), then compare stdout bytes and
   every written file (masking uuid7/timestamps/pids).
4. Measure dispatch latency before/after from `hook-router.lifecycle.jsonl`.

## Cutover (needs explicit approval)

Install `bin\harness-hook.exe`; `harness-hook.cmd` prefers it. Rollback: delete the exe (Python path remains).
