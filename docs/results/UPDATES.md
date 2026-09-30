# Harness updates and results

Newest first. Each entry states what changed, how it was verified, and what was measured. Results from before the
Rust migration are in [`LEGACY.md`](LEGACY.md).

## 2026-09-30 - Hook path rewritten in Rust (sub-project #2)

**Change.** `harness-hook.exe` (crates/harness-hook) replaces the Python router and its per-consumer Python
subprocesses (up to 5 per hook event; `obs` alone chained 3). All nine consumers run in-process on threads:
obs, notify, tasks, usage-fallback, context-guard, context, services, orb (hook side), herdr-bridge (hook side).
`core/router/harness-hook.cmd` runs the exe when `<harness>\bin\harness-hook.exe` exists, else the Python router.
Spec: `docs/superpowers/specs/2026-09-30-harness-hook-rust-design.md`. Code: `ab3e522`, `39d799d`.

**Verification.**

| Check | Result |
|---|---|
| clippy -D warnings, `cargo test --workspace` | pass, 421 tests |
| Windows cross-build (Windows-only code paths) | pass |
| Parity vs the Python router (`tests/system/hook_parity.py`, 26 scenarios / 59 hooks, Claude + Codex) | 0 unexpected mismatches, 22 expected |
| Code review before cutover | 0 critical; 3 high (unbounded lock wait, log mutex deadlock, unbounded stdin) fixed |
| Live cutover smoke test | Codex `Stop` stdout `{"continue": true}`, exit 0; 0 failed dispatches since cutover |

**Bugs found on the way (fixed in Rust).**
- The Python router drops a hook entirely when the payload contains a lone UTF-16 surrogate (`\ud800`); Rust
  processes it (U+FFFD).
- First-turn token usage of a new session was attributed to the global project: the Python `usage-fallback`
  raced the `obs` consumer for the session row. Rust serialises spool access and falls back to the cwd marker.
- herdr-bridge never delivered `herdr.binding` / `agent-state.record` / `turn-usage.record` (it imported a
  non-existent `obs.py`); these now reach the spool.
- Concurrent consumers in one process could hit `SQLITE_BUSY` on the spool (found by the parity test).

**Measured (sandbox parity run, median; same machine, same run).**

| Event | Python | Rust |
|---|---:|---:|
| `PostToolUse` (every tool call) | 527-813 ms | 93-148 ms |
| `UserPromptSubmit` | 546-771 ms | 89-121 ms |
| `Stop` | 670-1179 ms | 168-378 ms |
| `PermissionRequest` / `Notification` | 320-492 ms | 25-106 ms |
| `SubagentStart/Stop` | 243-339 ms | 16-26 ms |

**Interpretation.**
- The target of 30 ms per tool call is not met. Inside Herdr, the `herdr-bridge` consumer now dominates: it runs
  the `herdr` CLI on every hook (`pane report-agent`, plus `pane read`/`agent explain` on state changes), 155-1267 ms
  in the first live samples. Next optimisation: report to Herdr only when the projected state changes.
- First live samples right after installing a new exe are noisy (Defender first-run scans, 93% RAM in use);
  re-measure after a day of use.

**Rollback.** Delete `~\.agent-harness\bin\harness-hook.exe`; the shim falls back to the Python router.

## 2026-09-30 - Legacy hub removed; Docker only for the first install

**Change.**
- Removed the Node (Fastify) + PostgreSQL + MongoDB hub (`hub/src`, `hub/migrations`, `package*.json`,
  `docker-compose.yml`, `start.ps1`, `run-server.ps1`) and the bootstrap's `start.ps1` fallback and `HUB_DIR` key.
  `hub/` now holds only the Python client tools (hooks, `obs.py`, spool, SDKs).
- Removed the Node-vs-Rust parity test (`tests/system/`). It was a one-time migration check (0 mismatches, see the
  entry below); it needs the removed Node hub and a Postgres container. History: commit `726feb0`.
- `install.ps1 -WithHub` now builds the Rust hub **once** in Docker (runs `cargo test`, then the Windows build),
  copies `obs-hub.exe`/`obs-rs.exe` to `<harness>\bin`, stops Docker if it started it, and skips all of this when the
  binaries are already installed. The running harness never uses Docker.
- `tools/observability-client/obs.cmd` (repo copy) now carries the `obs-rs` dispatch with a relative `..\..\bin`
  path, so reinstalling no longer overwrites it; `uninstall.ps1` also removes `bin\`.
- Still kept for the rollback window: the one-time importer (`obs-hub import-postgres`, `scripts/obs-export-postgres.*`)
  and, on the owner machine, the Postgres/Mongo Docker volumes plus a backup of the legacy hub files.

**Verification.** Python unit suite and installer parse check; see commit message.

## 2026-09-30 - Observability hub rewritten in Rust + SQLite (sub-project #1)

**Change.** `obs-hub` (crates/obs-hub) replaced the Node + PostgreSQL + MongoDB hub on the same URL
(`127.0.0.1:4319`); data lives in `~/.agent-harness/var/obs/hub.sqlite3`. `obs-rs` (crates/obs-cli) serves
`errors`, `diagnose`, `trace`, `prompt show`, `analytics`, `change plan|finish|package`, `project register`, `flush`,
and the new read-only `query`; other `obs.cmd` subcommands still run `obs.py`. The services bootstrap starts the
native hub (`e598577`); Docker is now needed only for Rust builds. Spec:
`docs/superpowers/specs/2026-09-30-obs-hub-rust-sqlite-design.md`. Code: `726feb0`.

**Verification.**

| Check | Result |
|---|---|
| `cargo fmt --check`, `cargo clippy --all-targets -D warnings` | pass |
| `cargo test --workspace` | 145 passed |
| Parity vs legacy Node hub, synthetic stream (all 28 operation kinds, failures, edge probes) | 0 mismatches / 113 routes |
| Parity vs legacy Node hub, frozen snapshot of real data | 0 mismatches / 103 routes |
| `obs.py` vs `obs-rs` `change plan|finish` on spool copies (counter + `PACKAGE_NOW`) | 12/12 identical outputs and rows |
| Live migration | 28,824 rows / 34 tables, 0 count mismatches; 7 outage dead letters requeued, 0 lost |

**Measured results (Windows 11, 7.7 GB RAM).**

| Metric | Legacy | Rust hub | Change |
|---|---:|---:|---:|
| Observability stack RAM | ~750 MB | 11 MB working set (5.7 MB private) | -98.5% |
| Hub `/health` latency (x50, warm) | not recorded | p50 0.50 ms, p95 0.90 ms | - |
| Free RAM after also stopping Docker Desktop + WSL | 0.5-0.8 GB | 1.8 GB | +1.0-1.3 GB |
| Hook dispatch p50 / p95 / max | 667 / 2,023 / 18,343 ms (n=945) | 535 / 1,965 / 4,425 ms (n=40) | -20% / -3% / -76% |
| `obs` consumer p50 / p95 / max | 699 / 2,137 / 18,310 ms (n=817) | 502 / 1,958 / 4,308 ms (n=40) | -28% / -8% / -76% |

**Interpretation.**
- The hub itself answers in under 1 ms; the remaining ~500 ms per hook is Python process startup in the hook
  router and its consumers. That is the target of sub-project #2 (hook router in Rust).
- The post-cutover hook sample is small (n=40, ~10 minutes) and coincided with stopping Docker Desktop and WSL, so the
  latency gains cannot be attributed to the hub alone. Re-measure after a full day of use.
- Deliberate deviations from the legacy hub: no per-project Postgres schemas (use `obs query --project`), no
  MongoDB technology memory (it was unused).

**Rollback.** Stop `obs-hub` and remove `~/.agent-harness/bin/obs-hub.exe`; restore the legacy hub files from
`~/.agent-harness/var/backups/legacy-node-hub-*` into `~/.codex-shared/tools/observability-hub`, then run its
`start.ps1` (Postgres/Mongo volumes were kept). Since the next entry, the bootstrap no longer falls back to it.
