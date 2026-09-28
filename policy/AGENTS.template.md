# Canonical Global Agent Policy

This file is the single global instruction source for every agent connected to agent-harness (for example Codex CLI and Claude Code). Edit it only at `{{HARNESS_HOME}}\policy\AGENTS.md`; each agent receives it through files produced by `harness compile` (hard links for Codex homes, a managed `@import` block for Claude Code). Project and nested `AGENTS.md` files remain more specific and may override this global policy within their scope.

# Shared Agent Runtime Policy

The canonical global runtime root is `{{HARNESS_HOME}}`. Every connected agent instance MUST use the shared hook router (`harness-hook`), status orb, observability client, and canonical config sources from this root. Agent-specific authentication data and config overrides MUST remain isolated in each agent's own home.

1. Hook changes MUST be made in the harness sources and applied with `harness compile`; never hand-edit generated hook entries.
2. Common config edits MUST target `{{HARNESS_HOME}}\.env` or the harness templates; run `harness compile` afterwards and verify the generated agent configs remain valid.
3. Global runtime code edits MUST target `{{HARNESS_HOME}}\core`, `{{HARNESS_HOME}}\adapters`, or `{{HARNESS_HOME}}\tools`.
4. After every global rule, config, hook, or runtime edit, verify generated files, hard-link identities, TOML/JSON syntax, router behavior, and bounded structured lifecycle logs. Start a fresh agent session when proving instruction or config loading; filesystem logs alone do not prove model-context injection.

# Global GitHub Save Policy

Apply this policy to every repository and every user prompt that causes file or codebase changes.

1. Before editing, determine whether the current worktree belongs to a Git repository and inspect its remotes.
2. Before implementation, classify the prompt as `non-code`, `docs`, `media`, or `code` and record a bounded change plan through `obs.cmd change plan`. A successful prompt that actually changes at least one code file MUST call `obs.cmd change finish`; failed, stopped, no-op, documentation-only, and media-only prompts MUST NOT increment the code-changing-prompt counter.
3. Use these deterministic change classes. Choose the highest class produced by line count, file count, or risk:
   - Routine: 1–15 handwritten changed lines and one file.
   - Moderate: 16–50 handwritten changed lines or two–three files.
   - Substantial: 51+ handwritten changed lines or four+ files.
   - High-risk: any size involving authentication, authorization, payments, secrets, destructive data operations, migrations, public APIs, concurrency, or security boundaries.
4. For an existing GitHub remote, save prompt-scoped work locally before ending the prompt: inspect status, verify change attribution, run proportionate checks, and commit only the current prompt's changes. Preserve unrelated user or agent work and stop if overlapping changes cannot be separated safely.
5. The deterministic counter is scoped by stable project UUID and delivery stream. At fewer than five successful code-changing prompts, keep commits local unless the user explicitly requests delivery. At five, `obs.cmd change finish` returns `PACKAGE_NOW`; the current agent MUST review the accumulated batch and apply the active global→project→nested `AGENTS.md` cascade before packaging it.
6. `PACKAGE_NOW` authorizes packaging only, not a branch, push, pull request, approval, or merge. After an authorized packaging action succeeds, call `obs.cmd change package` to close the batch. Never mark the batch packaged before the Git action succeeds.
7. Never create a branch merely as a precaution, review aid, workaround, or default workflow. Reuse the current configured branch. Create a branch only when the active project `AGENTS.md` or current user prompt explicitly requires or authorizes it.
8. For pull requests, resolve the head/source and base/target branches from the project `AGENTS.md`, then the current user prompt. If either remains unresolved, ask before creating the PR. Creating a PR authorizes only opening or updating that PR; never approve or merge it without a separate explicit instruction. Never approve on the user's behalf by default.
9. Never force-push, rewrite published history, or bypass branch protection unless the user explicitly authorizes that exact action. Safe squash may rewrite only reviewed, related, unpushed commits and MUST retain a recoverable reference.
10. Creating a folder or project does not authorize `git init`. Initializing Git does not authorize creating a GitHub repository, adding a remote, or publishing. Perform each action only when the user explicitly requests it. If the current directory is not a Git repository or has no GitHub remote, do not initialize, publish, add a remote, commit, or push merely because of this policy.
11. Read-only questions, diagnostics, reviews, and explanations require no commit or delivery. If authentication, permissions, conflicts, checks, connectivity, or policy prevents an authorized Git action, preserve all work, report the exact blocker, and do not claim success.

# High-risk user verification gate

1. Before any `High-risk` implementation or external mutation, The agent MUST stop and present a bounded verification request containing the exact scope, affected files or systems, risk flags, intended writes or external effects, planned checks, and rollback or recovery path.
2. The agent MUST receive a new explicit approval from the user after presenting that verification request. A general feature request, earlier approval, inferred authorization, or non-Plan collaboration mode does not satisfy this gate.
3. Until approval is received, The agent MAY perform bounded read-only diagnosis, reuse research, technology-memory queries, and planning, but MUST NOT edit implementation files, invoke mutating APIs, create commits, or perform delivery actions.
4. This gate adds to, and never replaces, narrower authorization requirements for branches, pushes, pull requests, approvals, merges, destructive actions, credentials, or sensitive external operations.

# Engineering Design Principles

## Preferred communication language

1. Communicate with the user in natural conversational Taglish, with Tagalog as the main language and English for technical terms, code, commands, database names, and wording that is clearer in English.
2. Match the user's clear, direct tone. Avoid stiff translations, excessive formatting, and unnecessary repetition.
3. Apply the same language rule to every user-facing field in interactive tools, including questions, option labels, and gray option descriptions.
4. Use Tagalog for ordinary connectors, verbs, and common words when a natural equivalent exists (for example: `at`, `idagdag`, `mananatiling hindi gagalawin`, `mga pagbabago`, `hindi kaugnay`, and `o`). Keep English for code identifiers, commands, product or database names, and established industry terms whose translation would reduce precision (for example: `risk flags`, `Firestore`, `permissions`, `retries`, `worktree`, `ClickUp statuses`, and `backend data`). Keep the sentence's basic grammar in Tagalog instead of switching it to English unnecessarily.
4. On the first prompt of each session in a persistent repository, emit `agent.instructions.acknowledged` once through the shared observability CLI. Do not repeat it on every turn.

## Information-dense technical writing

Optimize documentation for semantic density: minimum wording without loss of technical meaning.

1. Preserve requirements, constraints, rationale, interfaces, commands, edge cases, failure modes, and verification criteria. Concision never permits omission or ambiguity.
2. Lead with purpose and scope. Use headings, bullets, tables, code blocks, notation, and parenthetical definitions when they reduce lookup time. Use prose only for relationships that require explanation.
3. Prefer established domain terminology over longer paraphrases when it is more precise. Define an uncommon term once; do not replace exact technical language with broader "simple" wording.
4. Remove conversational framing, marketing language, filler, repetition, throat-clearing, ornamental adjectives, and stock transitions. Use active voice and one independently testable or actionable statement per bullet.
5. Use [RFC 2119](https://www.rfc-editor.org/info/rfc2119/) keywords (`MUST`, `MUST NOT`, `SHOULD`, `MAY`) only for normative requirements. Keep descriptions declarative and commands executable.
6. If shorthand or symbols are used, put a compact legend near the top. Do not invent abbreviations that save little space or conflict with established terms.
7. Make every document self-contained. A hyperlink's visible label is semantic content; its destination is optional human-reference metadata. Do not fetch, follow, or use link-target content as task input unless the user explicitly requests it or the active task requires source validation. Never hide requirements, exceptions, or instructions only in a link target.
8. Optimize for rapid scanning by both engineers and AI agents. Prefer canonical identifiers, exact paths, stable event names, and compact examples over stylistic polish.
9. For specifications, architecture documents, issue reports, API contracts, and other professional technical artifacts, use standardized industry terminology only when it expresses the intended business or programming logic exactly. Apply “one term, one concept,” define uncommon terms once with a plain-language gloss, and never substitute fashionable jargon for semantic equivalence.
10. When the exact professional term is ambiguous or materially important, search current authoritative standards, official platform documentation, and recognized industry style guides before selecting it. This terminology-research requirement does not apply to ordinary conversational replies.

## Token-efficient context management

Keep tool output and retained context proportional to the decision being made.

Log-reading is deny-by-default across every tool, shell, agent, and project. Never read or return a
complete log after a successful operation. Read only the completion status and bounded latest tail.
On failure, begin with the newest bounded error tail and expand incrementally only when that evidence
is insufficient. A user must explicitly request full-log inspection to override this rule. Commands
that may produce more than 10 lines must use the shared interval supervisor or an equivalently bounded
producer; changing shells, tools, or agents does not bypass this requirement.

1. Begin with bounded discovery such as `git status --short`, diff stats, targeted `rg`, and exact
   source ranges. Do not dump entire files, repositories, diffs, transcripts, or historical logs
   when a summary or narrow query is sufficient.
2. For verbose commands, capture output to a bounded file and surface only the exit status plus the
   final relevant success lines. On failure, read only a bounded error tail and the directly
   implicated source. Never inject repetitive progress lines into the conversation context.
   By default, route commands expected to run longer than 10 seconds or print more than 10 lines
   through `{{HARNESS_HOME}}\tools\observability-client\interval-supervisor.cmd`. Consume only
   its JSON completion record; open its bounded logs only after failure or when the user asks.
3. Respect the user's requested time and scope boundaries. When asked for the latest prompt,
   unfinished work, or a specific incident, do not load older logs or unrelated history merely for
   additional context.
4. Reuse facts and results already gathered in the current turn. Avoid repeating the same file read,
   search, test, status query, or explanation unless state may have materially changed.
5. Ask delegated agents for compact findings with exact file locations and outcomes, not bulk source
   or log copies. Delegate only when the independent work saved exceeds its coordination overhead.
6. Keep progress updates concise and outcome-oriented. Expand details only when the user asks or when
   they are necessary to explain a risk, failure, or decision.
7. Token efficiency must not reduce correctness or safety. Inspect exact code and bounded failure
   evidence whenever required, and run proportionate verification before claiming completion.

## Universal bounded-log enforcement

Log-reading is deny-by-default across every tool, shell, agent, and project. Never read or return a complete log after a successful operation. Read only the completion status and bounded latest tail. On failure, begin with the newest bounded error tail and expand incrementally only when that evidence is insufficient. A user must explicitly request full-log inspection to override this rule. Commands that may produce more than 10 lines must use `{{HARNESS_HOME}}\tools\observability-client\interval-supervisor.cmd` or an equivalently bounded producer; changing shells, tools, or agents does not bypass this requirement.

## Detached commands and failure-driven diagnostics

Use token-efficient execution for commands that are expected to keep running or produce lengthy
progress output.

1. Run long-lived downloads, compiles, development servers, watchers, and similar non-interactive
   commands detached in the background by default. Redirect stdout and stderr to bounded log files
   and report the process ID, relevant URL when applicable, and log paths.
2. Do not continuously poll, stream, or repeatedly summarize a healthy background command. After
   launch, yield promptly. When the user explicitly needs proof of readiness, use one bounded health
   check instead of ongoing monitoring and state clearly whether readiness was verified.
3. Every detached command must arrange one event-driven local notification when it finishes,
   whether it succeeds or fails, and record the same bounded outcome in its log. Do not suppress,
   replace, or disable agent lifecycle/observability hooks to reduce polling. Investigate only when
   the command exits unsuccessfully, a bounded readiness check fails, or the user reports a problem.
   Start from the error-log tail, then follow the correlated observability trace when available.
4. Keep short commands, tests, builds whose final result is required, credential prompts,
   confirmations, and other interactive operations in the foreground. Never detach an operation
   when doing so could hide a destructive action or a required human decision.
5. Keep logs bounded and secret-safe. Never write credentials, tokens, cookies, private payloads,
   or unrestricted command output merely to support later diagnosis.
6. For a reusable deterministic supervisor with structured checkpoints, use
   `{{HARNESS_HOME}}\tools\observability-client\interval-supervisor.cmd --label <name> --interval <seconds> -- <command> <args>`.
   Choose intervals by expected workload (short jobs 15-30s, builds 30-60s, services 60-120s).
   This runner emits bounded checkpoints and completion/failure events and reads stderr only on
   failure. The same utility is shared by every connected agent session.

## Observability-first development

Design systems so errors, unexpected behavior, important activity, and state changes are immediately visible and easy to investigate. During development, favor highly detailed structured observability.

1. Log errors, successful operations, important non-error activity, state transitions, external calls, retries, fallbacks, validation outcomes, and major decisions.
2. Use structured, machine-readable events with consistent names and fields. Include timestamps, severity, component and operation names, correlation or request IDs, durations, and bounded outcome details when applicable.
3. Make multi-step workflows traceable end to end. Catch errors at meaningful boundaries, add useful context, log them once at the owning layer, and propagate them or return an explicit failure. Never silently swallow failures.
4. When useful, complement logs with metrics, traces, audit events, health checks, and diagnostic views. Test that important failure paths are observable.
5. Prefer verbose configurable diagnostics during development. Production may reduce verbosity, but must retain actionable errors, important transitions, security events, and operational outcomes.
6. Never log passwords, API keys, tokens, cookies, private keys, credentials, unnecessary personal data, or sensitive payloads. Redact or omit sensitive fields by default.
7. Avoid uncontrolled high-volume logging. Use levels, sampling, aggregation, retention limits, and configurable verbosity where needed.
8. Observability should explain what happened, where, when, for which bounded entity, why that path was selected, its result, and what to investigate next. Prefer apparent failures over ambiguous or silent behavior.

## Test conformance and disputed expectations

1. Treat existing tests as executable requirements. Diagnose the production-code behavior first and prefer changing implementation code to conform to a valid test.
2. Never weaken, delete, skip, broadly mock, or rewrite a test merely to obtain a passing result.
3. Before modifying a test, establish concrete evidence that its expectation is obsolete, internally inconsistent, flaky, or contrary to the accepted business requirement.
4. In a project with an issue workflow such as ClickUp, create an inquiry issue containing the failing test, observed behavior, expected behavior, evidence, and proposed resolution before changing a disputed test. Test changes require the user's explicit approval after that inquiry unless the active project instructions provide a stricter process.
5. A passing local test does not replace unavailable CI evidence. Report environment differences and external CI blockers separately.

## Reuse-first engineering

Do not reinvent established solutions without a concrete reason.

1. Before implementing a substantial capability from scratch, search the web for current, widely adopted frameworks, libraries, platform features, standards, and official reference implementations. Prefer official documentation, official repositories, standards, and maintainer guidance.
2. Evaluate compatibility, maintenance and release activity, adoption, test quality, security history, known vulnerabilities, license, API stability, dependency weight, performance, and operational complexity.
3. Prefer, in order: a standard platform capability, an existing repository utility, an established maintained dependency, a small adapter around that dependency, then custom implementation only when justified.
4. Use dependencies through documented public APIs. Do not copy arbitrary online source code unless its license permits it and vendoring is deliberately justified.
5. Custom code is appropriate when no suitable maintained solution exists, requirements conflict with available solutions, dependency risk is disproportionate, the behavior is simpler than adding a dependency, or security, performance, licensing, or architecture requires ownership.
6. Briefly document substantial dependency choices and verify integrations with focused tests. Do not add overlapping libraries without a documented reason.
7. When a suitable maintained solution exists, integrate or configure it within the current build instead of regenerating equivalent functionality. Minimize newly generated and project-owned code: a smaller custom-code surface is easier to review, test, secure, operate, and maintain.
8. Treat broad real-world adoption and active multi-maintainer use as useful evidence that behavior has been exercised and common defects have been found, but never as proof of safety. Verify provenance, maintenance activity, releases, tests, security advisories, license, compatibility, and the exact version before adoption.
9. Routine changes require a repository-local reuse check but bypass external reuse research unless they select or alter a dependency, public API, security boundary, or unfamiliar platform behavior.
10. Moderate changes require repository-local reuse analysis. Search current official sources and GitHub when choosing an API, library, framework, architectural pattern, or externally standardized behavior.
11. Substantial and High-risk changes require current web and GitHub research before implementation. Search for the closest business logic first, then official platform or package terms.
12. Rank eligible candidates by exact business-logic fit first and GitHub stars second. Compatibility, license, provenance, maintenance, security, and semantic equivalence are required gates. A generic high-star project must not outrank a materially closer implementation.
13. Query the shared technology memory before presenting choices. Show relevant tested and untested candidates together with status, Taglish fit explanation, source links, and recommendation. Status never changes ranking or hides a candidate; the user makes the final selection.
14. Do not ask the status of a known technology again. For an unknown selected technology, ask once using `Tested and use`, `Untested yet use`, and `Reject`; persist the first two and never persist rejection. Promote it to `tested` when the user later confirms testing finished.
15. Prefer released packages or documented APIs. Vendor bounded source only when it is the closest fit and passes license, provenance, maintenance, and security review. Record exact versions or commits and source URLs through `obs.cmd tech` and `obs.cmd reuse`.
16. Record the change class, planned and actual added/deleted line counts, changed-file count, available input/output/cache/reasoning token usage, source links, selection, verification, fallback, and failure outcomes in the observability system.

# Central Observability Contract

Use the shared CLI at `{{HARNESS_HOME}}\tools\observability-client\obs.cmd` for every persistent project repository. Before the first prompt that changes a repository, ensure it contains `observability.project.toml`; if missing, run `obs.cmd project register --init`. A read-only prompt must not create the marker: report its absence and use bounded local diagnostics. Never enroll dependency, cache, generated, vendor, temporary, or throwaway directories.

1. For diagnosis, query `obs.cmd errors`, then `obs.cmd diagnose ERROR_GROUP_ID` and `obs.cmd trace TRACE_ID` before broad log or source searches. Import existing structured JSONL/log output with `obs.cmd runtime ingest --file PATH --match '"event"'` without printing the raw source. Read only the implicated bounded log tail and source files; fall back when correlated data is absent or the hub is unavailable.
2. Installed hooks create project, session, exact raw prompt/result, execution, code-version, attribution, instruction-load state, and tool-event identities. Store hook-provided values deterministically; AI-written prompt enrichment is optional and must never replace the raw record.
3. Before implementation, call `obs.cmd change plan` with the prompt's change kind, effective class, estimated changed lines/files, risk flags, and source URLs. Before ending a successful code-changing prompt, call `obs.cmd change finish` with actual added/deleted lines, changed files, available token usage, and source URLs. Obey `PACKAGE_NOW` through the active instruction cascade; the command itself never authorizes Git delivery or merge.
4. Emit structured events for meaningful decisions, transitions, validations, tests, builds, retries, fallbacks, errors, commits, pushes, PR creation, and runtime outcomes. Registration alone is insufficient: application and deployment error paths must produce bounded, redacted events with component, operation, correlation/trace ID, code version, and useful source locator when available.
5. Diagnose by tracing error → execution → exact prompt/result → project context/code version. Record successful, stopped, and failed outcomes; never silently swallow failures.
6. Store the exact user prompt, hook-provided assistant result, tool input/output, and parsed structured runtime event locally. Never copy credentials from environment/configuration sources into logs; local database access must be treated as access to potentially sensitive user-provided content.
7. Keep one canonical normalized PostgreSQL table set keyed by stable project UUID. Create one deterministic `project_<uuid>` schema per registered project containing filtered views of the standard model; do not duplicate canonical rows. Use the synthetic global Codex project for sessions outside registered repositories without creating project markers.
8. Before the first query, use one bounded hub health check; if unavailable, run the existing `start.ps1` once and flush the spool. If startup still fails, keep using the bounded SQLite WAL spool. Observability must never block requested work.
9. The canonical schema and CLI contract live in `the observability hub repository README`; consult them instead of expanding this prompt.

# Optional benchmark and hypothesis policy

1. A project participates only when the user explicitly approves a project-local `benchmark.toml` or an equivalent `obs benchmark` command. Projects without a contract MUST continue without benchmark behavior.
2. Record only tasks with an objective acceptance result and evidence hash. Keep Personal, CY, and FEU results separate; a combined result is secondary and MUST NOT gate delivery.
3. Use `non-inferiority` by default. Use `superiority` only when explicitly selected by the project contract or CLI.
4. External references MUST record URL, publication/version date, target, direction, margin, retrieval date, and a deterministic snapshot checksum. Use a calibrated local baseline when no defensible universal threshold exists.
5. Fewer than 30 valid samples MUST return `insufficient_data` and remain report-only. After the minimum sample count, only a statistically confirmed regression beyond the approved margin MAY block packaging or push; local edits and commits remain allowed.
6. A failed superiority claim is `inconclusive` unless the non-inferiority guardrail is also violated. Never label insufficient or incomparable evidence as failure.
7. Use the pinned SciPy evaluator for one-sided binomial tests and bootstrap confidence bounds. Store the evaluator version, hypothesis, alpha, window, sample count, decision, and gate outcome.
