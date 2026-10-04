# Markdown discovery and explicit reads

Purpose: agents choose documents from project-relative paths; Rust performs deterministic traversal and bounded reads.

## Flow

1. Resolve an enclosing Git or registered project below `AH_MARKDOWN_PROJECTS_DIR`.
2. Apply `AH_MARKDOWN_ALLOWED_ROOT_REGEX` to its normalized canonical root. Canonical containment remains mandatory.
3. On prompt submit, supply a bounded filename inventory once per changed inventory/session/context epoch. Contents are never injected.
4. The agent selects paths from their folder structure and filenames, then calls the packaged command.
5. Record the content hash, source, supplied interval, outcome and supplied character count. Unchanged observed intervals are skipped.

```powershell
& "$env:USERPROFILE/.agent-harness/core/harness.cmd" markdown index --cwd "$PWD" --prefix docs --limit 80
& "$env:USERPROFILE/.agent-harness/core/harness.cmd" markdown read --cwd "$PWD" --session SESSION_ID --file docs/design.md --file README.md
```

`index --offset N` requests another page explicitly. It returns at most 200 paths and 12,000 path characters.
Paths omit the project root name. No contents or later filename pages are automatically drained on tool calls.
`read --offset N --max-chars N` selects a Unicode-character interval; the default content budget is 12,000 characters across all selected files.
Files are UTF-8 Markdown, at most 2 MiB each; a command accepts at most 32 explicitly indexed files.
`--agent` and `--instance` MUST match the hook identity when overriding the Codex/personal defaults.
`--session` is required for reads unless `AH_SESSION_ID` or `CODEX_THREAD_ID` supplies it.
`--force` permits an explicitly requested reread, within the remaining session budget.

## Activation and configuration

Machine-owned values stay in the installed `.env`; environment values override them.

| Key | Default | Meaning |
|---|---|---|
| `AH_MARKDOWN_CONTEXT_ENABLED` | `1` | Enable discovery and commands |
| `AH_MARKDOWN_PROJECTS_DIR` | user Desktop/Projects | Mandatory containment boundary |
| `AH_MARKDOWN_ALLOWED_ROOT_REGEX` | all descendants | Optional repository allowlist regex; forward-slash canonical paths |
| `AH_MARKDOWN_INDEX_CHARS` | `6000` | Automatic filename inventory budget |
| `AH_MARKDOWN_SESSION_CHARS` | `64000` | Explicit-read content budget per agent/instance/session/epoch |
| `AH_MARKDOWN_TELEMETRY_ENABLED` | `1` | Enqueue normalized metadata through the shared spool |
| `AH_MARKDOWN_CONTEXT_EPOCH` | `0` | Explicit context-generation identity |

Regex example after expanding the configured Projects path: `(?i)^C:/path/to/Projects/(repo-a|repo-b)$`.
Missing boundaries, invalid regexes and disallowed roots produce empty fail-open hook context; explicit commands return a bounded error and nonzero status.
Umbrella Projects sessions MUST select a target using `--cwd` before discovery. No sibling-repository search is inferred from prompt keywords.
Nested repositories, caches, backups, generated/dependency directories, ignored files and symlink directory traversal are excluded.

## Coverage and recovery

State is keyed by canonical root, agent, instance, native session and context epoch. Coverage is `full`, `partial`, or `unknown`.
Explicit reads track and merge up to 64 disjoint character intervals; a content hash change invalidates prior-version coverage.
Successful native `Read` results establish full coverage only when returned contents exactly equal the current file. Matching excerpts establish partial coverage; other shapes remain unknown.
Shell path mentions, failed tools and filesystem presence never prove model-context loading. `AGENTS.md`/`CLAUDE.md` appear as native instruction candidates with loading `unverified`; there is no native consumption acknowledgment.
Emission state does not prove delivery or model comprehension. After a failed output delivery, use `--force`; after verified compaction/reset, use a new `--epoch`/`AH_MARKDOWN_CONTEXT_EPOCH`.

No full walk occurs on ordinary `tool.completed`. On prompt/index/read, cached directory/file/ignore-rule metadata is checked; changed metadata triggers a deterministic rebuild under a sidecar lock.
External creates/deletes/renames are discovered through indexed parent directories. Use `index --refresh` when global ignore rules change or metadata timestamps are unreliable/restored.
This uses existing `ignore`, sidecar-lock and atomic-write utilities rather than another daemon. Rust `regex` provides bounded regex compilation through its public API; no filesystem watcher dependency is added.
Technology-memory lookup was attempted; the installed CLI has no `tech`/`reuse` command. No previous tested status was inferred.
Sources: [Rust RegexBuilder size limit](https://docs.rs/regex/latest/regex/struct.RegexBuilder.html), [SQLite foreign keys](https://www.sqlite.org/foreignkeys.html).

## Normalized metadata

The existing hub stores `markdown_documents` (one project/path), `markdown_document_versions` (one hash/version),
`markdown_context_batches` (event/session identity, reason, duration), and `markdown_context_observations` (version FK, source, coverage, interval, outcome, characters).
`markdown-context.record` is transactional and replay-idempotent; `obs query --project UUID` scopes all four tables.
Document bodies are not duplicated into these tables. Existing canonical prompt/tool records remain unchanged.
Telemetry failures are logged and isolated from successful reads. Unregistered projects are not enrolled by discovery.
Normalization reduces repeated metadata storage; RAM and model token savings require separate measurement.

## Installation and rollback

Repository verification uses sandboxed state/SQLite only. Install the matching hook and hub binaries together before enabling normalized telemetry;
an older hub does not support the new operation. Existing schema changes are additive. Roll back to prior binaries and disable Markdown telemetry;
retain added tables for recovery instead of deleting historical data. Live installation, DB migration and restart require owner approval.
