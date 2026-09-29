# Harness updates and results

Newest first. Each entry states what changed, how it was verified, and what was measured. Results from before the
Rust migration are in [`LEGACY.md`](LEGACY.md).

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

**Rollback.** Stop `obs-hub`, remove `~/.agent-harness/bin/obs-hub.exe`, run `docker compose up -d` in
`~/.codex-shared/tools/observability-hub` (volumes kept); the bootstrap falls back to `start.ps1`.
