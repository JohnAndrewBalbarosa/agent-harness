# Background services in Rust: `harness-services.exe` + `interval-supervisor.exe` (sub-project #3)

Status: approved direction 2026-09-30 ("oo" to #3). Compiler: Docker. Contract map: research of 2026-09-30.

## Purpose

Replace the Python/Node background processes (bootstrap, OTLP usage receiver, console window monitor, Herdr
subscriber + Node pipe helper, flush worker) with one Rust binary, and replace the policy-mandated
`interval_supervisor.py` (currently broken on import) with a Rust one-shot CLI.

## Binaries and modes

| Command | Lifetime | Replaces |
|---|---|---|
| `harness-services ensure` | one-shot, per `session.start` (launched by the hook's services consumer) | `python -m core.services.bootstrap` |
| `harness-services serve` | daemon, single instance (`Local\AgentHarnessServices` mutex + `var/services.lock`) | usage receiver + console monitor + flush worker |
| `harness-services herdr-subscribe` | one per Herdr socket (same `subscriber-<sha256[:16]>.lock/.pid` files), launched by the hook | `subscriber.py` + `socket-stream.mjs` |
| `interval-supervisor -- <cmd>` (separate crate) | one-shot CLI | `interval_supervisor.py` |

`ensure`: start `serve` detached when its mutex is free; start `bin/obs-hub.exe serve` when `/health` fails;
start the orb when `health.json` is not healthy or older than 15 s (orb stays Python until #5); log each step to
`var/logs/services.lifecycle.jsonl` exactly as bootstrap.py (`receiver.ensure`, `orb.ensure`, `hub.ensure`,
`console-monitor.ensure`, `bootstrap skipped/already_running`).

## Contracts kept (byte-compatible)

- OTLP receiver on `127.0.0.1:{OTLP_PORT|4320}`: `GET /health` → 200 `{"status":"ok"}`; `POST /v1/logs` JSON only,
  10 MiB limit (413), always 200 `{}` otherwise; 404 `{}` elsewhere; exclusive bind (exit quietly if taken).
  Claude/Codex field whitelists, uuid5 inputs, token math, `redact`, `turn-usage.record` operation shape,
  project lookup from the spool `context` table with the global-project fallback.
- Console monitor: mutex `Local\CodexSharedConsoleWindowMonitor`, WinEvent hooks (SHOW, FOREGROUND), filters,
  dedupe, codex-ancestor/profile detection, log `~/.codex-shared/var/logs/console-window.lifecycle.jsonl` record
  shape and rotation.
- Herdr subscriber: named pipe `\\.\pipe\<HERDR_SOCKET_PATH>`, NDJSON (1 MiB line cap), `events.subscribe` request
  and types, sanitize rules, backoff 1 s → 30 s, log records in `var/logs/herdr-bridge.lifecycle.jsonl`.
- `interval-supervisor`: same CLI flags, log files (`<label>.stdout.log`, `.stderr.log`, `.state.json`), final
  one-line JSON completion record, tail/summary rules, exit codes.

## Deliberate deviations (documented)

- Receiver: no process spawn per POST; the daemon runs one internal drain loop (flush_spool.py rules: batches of
  50, bisect on 400/413, dead-letter after 4 network attempts) triggered by enqueues, debounced. Writes honour
  `AH_OBS_SPOOL` like the lookup (Python ignored it for writes). Requests are handled concurrently.
- Herdr subscriber: `herdr-event.record` is enqueued into the real spool (Python loaded a non-existent `obs.py`).
  Buffered reads instead of one syscall per byte; no Node helper.
- Console monitor: filter before taking a process snapshot.
- `interval-supervisor`: observability events go through the spool (`obs_cli` operations) instead of the missing
  `sdk.python` import, so the tool works again.
- Each daemon task is isolated (`catch_unwind` + restart with backoff); one failing task never stops the others.

## Integration

- Hook `services` consumer: launch `bin/harness-services.exe ensure` when present, else the Python bootstrap.
- Hook herdr-bridge: launch `bin/harness-services.exe herdr-subscribe` when present, else `subscriber.py`.
- Python services remain as fallback until the cutover is approved; uninstall removes `bin\`.

## Verification

fmt, clippy -D warnings, cargo test (Docker); unit tests ported from `tests/unit/test_services.py`,
`test_usage_otel.py`, `test_usage_receiver_http.py`, `test_lifecycle.py`; a one-time parity script comparing the
Python receiver and the Rust receiver on recorded OTLP payloads (spool rows) and the Python vs Rust
interval-supervisor completion records; RSS measurement of the daemon.

## Cutover (needs approval)

Stop Python receiver/monitor/subscriber processes, install binaries, let the next `session.start` run `ensure`.
Rollback: delete the two exes; the hook falls back to Python.
