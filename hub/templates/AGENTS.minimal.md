# Codex Observability

Classify every prompt with `obs.cmd change plan`. Finalize successful code changes
with actual line/file counts through `obs.cmd change finish`. It derives one
cross-profile counter per project and delivery stream and returns `PACKAGE_NOW`
at five un-packaged code-changing prompts. The current agent must then apply the
active global→project→nested `AGENTS.md` cascade. The directive never authorizes
branch creation, push, PR, approval, or merge. Never create a branch by default;
never approve or merge a PR without a separate explicit instruction. After an
authorized packaging action succeeds, close the batch with `obs.cmd change package`.

Use `obs.cmd` for persistent project repositories. Before the first prompt that
changes a repository, ensure it has `observability.project.toml`; if missing, run
`obs.cmd project register --init`. Do not enroll dependency, cache, generated,
vendor, temporary, or throwaway directories. Read-only work must not create files.

For diagnosis, run `obs.cmd errors`, then `obs.cmd diagnose ERROR_GROUP_ID` and
`obs.cmd trace TRACE_ID` before broad searches. Import structured logs with
`obs.cmd runtime ingest --file PATH --match '"event"'`; do not print raw logs.

Emit bounded, redacted structured events for meaningful decisions, state changes,
external calls, retries, tests, builds, failures, commits, pushes, and runtime
outcomes. Include component, operation, correlation ID, severity, outcome,
duration, code version, and source locator when useful. Never log credentials,
tokens, cookies, private keys, unnecessary personal data, or unrestricted payloads.

Run long-lived commands detached with bounded log files and one local completion
notification. Do not repeatedly poll healthy work; use one bounded readiness check
when proof is required. Keep short, destructive, and interactive commands in the
foreground.

Observability must never block requested work. If the hub is unavailable, start it
once and flush the spool; otherwise continue with the bounded SQLite WAL spool.
