# Observability hub: Rust + SQLite rewrite (sub-project #1)

Status: approved 2026-09-30 (brainstorming answers: migrate all data = A; per-project schemas → `obs query --project` temp views = A; Cargo workspace in this repo + `obs-cli` subset = A).

## Purpose

Replace the Node (Fastify) hub + PostgreSQL 17 + MongoDB 7 containers (≈750 MB RSS incl. Docker Desktop and WSL) with one Rust process over one SQLite file, without changing what clients observe.

- Target idle RSS: ≤ 15 MB for `obs-hub`; 0 MB when stopped (SQLite is embedded).
- Portable: Windows, Linux, macOS; `git clone` + `cargo build` — no Docker at runtime.
- Language floor: Rust. Only bundled third-party C (SQLite via `rusqlite/bundled`) is permitted; no project-owned code below Rust.

## Scope

| In | Out (later sub-projects) |
|---|---|
| `obs-hub`: HTTP API on `127.0.0.1:4319`, 28 operation kinds, 12 routes, storage maintenance | Hook router in Rust (#2) |
| `obs-hub import-postgres`: one-time migration of all live data | `harness sync` cascade (#3) |
| `obs-cli` subset: `errors`, `diagnose`, `trace`, `prompt show`, `analytics`, `change plan\|finish\|package`, `project register`, `flush`, new `query` | `benchmark`, `runtime ingest`, `git` subcommands stay in `obs.py` |
| Drop MongoDB (dead code: `technology-memory.mjs` is never imported) | Cutover of the live hub (requires explicit user approval; High-risk gate) |

## Compatibility contract (source of truth)

The live implementation `~/.codex-shared/tools/observability-hub/src/server.mjs` and `migrations/001–007` define behavior. The Rust hub MUST reproduce:

- Routes, status codes, and JSON shapes of `server.mjs:379-537`, including 404 bodies (`{"error": "..."}`) and the batch error shapes (`{"error":"invalid operation batch","issues":[{path,code}]}`, `{"error": msg≤300}`).
- Batch semantics: 1–500 ops, whole batch in one transaction, any failure rolls back everything; idempotent replays (`ON CONFLICT` behavior per kind).
- Validation parity with zod (`operationId` UUID, `kind` 3–80 chars, `occurredAt` ISO-8601 UTC `Z`, `data` object) and with Postgres constraints that make a batch fail (FKs, CHECKs, regex CHECKs on `projects.slug` and `event_definitions.code`, UUID syntax, hex byte lengths, non-finite benchmark values).
- node-pg wire types: `bigint`/`count(*)`/`SUM` → JSON string; `integer`/`smallint`/`double` → number; `boolean` → bool; `timestamptz` → ISO-8601 UTC with milliseconds (`2026-09-30T01:02:03.456Z`); `jsonb` → parsed JSON; `bytea` selected via `*` → `{"type":"Buffer","data":[..]}`; `uuid` → lowercase text.
- Fastify defaults: 1 MiB body limit → 413; unknown route → 404 `{"message":"Route <M>:<path> not found","error":"Not Found","statusCode":404}`; handler exception → 500 `{"statusCode":500,"error":"Internal Server Error","message":...}`.

Intentional deviations (documented, not observable by current clients):

- No per-project schemas/views, no `project_schemas` table, no advisory lock, no partitions.
- `one_day_log_rollups` duplicate view dropped; `obs query` scopes `herdr_runtime_instances` to nothing project-specific (global table, exposed unfiltered only via `--global`).

## Architecture

```
crates/
├─ obs-common/     paths (DB, spool), UTC timestamp normalization, UUID/hex validation
├─ obs-hub/        binary `obs-hub`
│  ├─ main.rs            serve | import-postgres | maintain
│  ├─ config.rs          env → Config (OBS_HOST, OBS_PORT, OBS_DB_PATH, OBS_LOG_LEVEL)
│  ├─ store/             SQLite: open + pragmas, schema.sql, writer/reader connections
│  ├─ operations/        batch validation + one module per concept (projects, sessions, prompts,
│  │                     executions, events, errors, usage, herdr, handoffs, changes, benchmarks)
│  ├─ routes/            one module per route family; wire encoding in routes/wire.rs
│  ├─ maintenance.rs     30-day event retention, 2-day rollups, payload expiry (daily)
│  └─ import_postgres/   JSONL dump → SQLite, row-count verification
└─ obs-cli/        binary `obs-rs` (dispatched from `obs.cmd` for the ported subcommands)
   ├─ spool/        same SQLite spool schema as obs.py (shared file, WAL)
   ├─ commands/     one module per subcommand
   └─ query/        read-only SQLite + TEMP views reproducing the 35 project views
```

Runtime choices (low RAM first):

- `tokio` current-thread runtime; `axum 0.8` (matches `website/apps/api`), `tower-http` body limit.
- `rusqlite` (bundled SQLite ≥ 3.45): one writer connection (batches) + one reader connection, WAL, `synchronous=NORMAL`, `foreign_keys=ON`, `busy_timeout=5000`, `cache_size=-2048` (2 MiB). Blocking DB work runs in `spawn_blocking`.
- Timestamps stored as normalized UTC text (lexicographically ordered = chronologically ordered); interval math done in Rust.
- `DISTINCT ON` → `ROW_NUMBER()`; `LATERAL` → correlated subquery; `EXTRACT(EPOCH)` → Rust; regex CHECKs → Rust validators.

## Data migration

1. `scripts/obs-export-postgres.{ps1,sh}`: `docker exec … psql` exports each table as JSON lines (`row_to_json`) into a dump directory (read-only on Postgres).
2. `obs-hub import-postgres --dump <dir> --db <new.sqlite3>`: creates schema, imports tables in FK order in one transaction, normalizes types (bytea `\x..` → BLOB, timestamptz → UTC ms, jsonb → text), verifies per-table row counts, prints a JSON report. Refuses to write into a non-empty DB.
3. Postgres and its volume remain untouched until the user approves cutover.

## Verification (per global "Implementation and verification order")

1. Write all code + tests (unit tests per module where logic is non-trivial; no build between files).
2. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` (Docker `rust` image).
3. `cargo test --workspace` + whole-system test `tests/system/obs_hub_parity`:
   - Start old hub (Node, existing Postgres container, throwaway database) and new hub on free ports.
   - Replay one generated batch stream covering all 28 kinds + replays + failing batches to both.
   - GET every route on both; diff JSON after masking server-generated fields (`created_at` defaults, identity ids).
   - Migration check: export live Postgres → import → compare row counts and route outputs for sampled ids against the live hub.
4. Measure idle RSS of `obs-hub`.

## Cutover (separate approval)

Stop Node hub, point `OBS_ENDPOINT` users (unchanged URL) at `obs-hub`, route ported `obs.cmd` subcommands to `obs-rs`, `docker compose down` (volumes kept), update global policy text ("PostgreSQL" → "SQLite", `docker exec … psql` → `obs query`). Rollback: stop `obs-hub`, `docker compose up -d`, restart Node hub.
