# Architecture

```
agent CLI ──native hook──► core\router\harness-hook.cmd <agent> [<native-event>]
                             │  adapters\<agent>\adapter.py  normalize() → agent-harness.event/v1
                             ▼
                        core\router (parallel, CREATE_NO_WINDOW, per-consumer timeout, fail-open)
                             ├─ obs            (native payload) → tools\observability-client\hook.py → hub / spool
                             ├─ notify         turn.stop · attention.needed · prompt.submit
                             ├─ tasks          subagent.start · subagent.stop
                             ├─ usage-fallback turn.stop → adapters\<agent>\transcript.py
                             ├─ context        session.start → hub context bootstrap
                             ├─ orb            (native, when its venv exists)
                             └─ herdr-bridge   (native, only inside Herdr)
                             ▼
                        adapter.respond() ──stdout──► agent

agent CLI ──OpenTelemetry OTLP/HTTP JSON──► core\usage\receiver (127.0.0.1:OTLP_PORT)
            dispatch by service.name → adapters\<agent>\otel.py (whitelist) → turn-usage.record → hub
```

## Units

| Unit | Responsibility | Depends on |
|---|---|---|
| `core/events.py` | immutable common event, JSON round trip, 8 KiB native payload bound | — |
| `adapters/base.py` | adapter protocol, shared snake_case payload mapping, loader | events |
| `adapters/<agent>/` | `adapter.py` (normalize/respond/safe_default), `manifest.toml`, `otel.py`, `transcript.py`, `patches.py`, fixtures | base |
| `core/router/` | entry point, consumer registry, fan-out | adapters, lifecycle |
| `core/lifecycle.py`, `core/locking.py` | bounded line-atomic JSONL logs, cross-process locks | — |
| `core/notify/` | pure `decide()`, delivery backends, consumer, cross-reference report | tasks, display |
| `core/tasks/` | per-session subagent state; adapter shell scan | locking |
| `core/usage/` | OTLP receiver, transcript fallback, obs client bridge | adapters |
| `core/consumers/obs.py`, `core/context/` | observability and session context bridges | vendored tools |
| `core/config.py` | `.env` loading, quoted hook commands | — |
| `compile/` | render rulesync hooks, merge helpers, simulated compile + backups | config, adapters |
| `tools/` | vendored runtime tools (observability client, status orb, herdr-bridge, legacy router) | — |
| `hub/` | observability hub distribution (Node + Postgres) deployed by `install.ps1 -WithHub` | Docker |

## Guarantees

- A hook never blocks its agent: every failure returns the adapter's safe default and exit code 0.
- Consumers see only common events (except native-payload consumers that need full tool I/O).
- Compile never runs rulesync on real homes; it simulates on copies and applies diffs with backups.
- Codex agent-instance IDs keep their legacy values, so existing observability history stays continuous.
