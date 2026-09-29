# agent-harness

Portable, Windows-first middleware that every AI coding agent connects to. One policy, one hook router, one set of
consumers (observability, token usage, notifications, background-task tracking, session context) and one database,
instead of per-agent copies. Ships adapters for **Codex CLI** and **Claude Code**; other agents plug in through a
small adapter that passes the conformance kit ([docs/adding-an-agent.md](docs/adding-an-agent.md)).

- **Compile (build time):** one source → each agent's native files. Hooks are generated with
  [rulesync](https://github.com/dyoshikawa/rulesync) (pinned, simulated before writing); the policy reaches Codex by
  hard link and Claude Code by a managed `@import` block; OpenTelemetry settings are merged into managed sections.
- **Aggregate (runtime):** every agent hook runs `harness-hook <agent>`; the agent's adapter normalizes it into an
  `agent-harness.event/v1` common event; consumers run in parallel (hidden, timed out, fail-open).

Architecture: [docs/architecture.md](docs/architecture.md) · Design spec:
[docs/superpowers/specs/2026-09-28-agent-harness-design.md](docs/superpowers/specs/2026-09-28-agent-harness-design.md)

## Safety boundary

The repository contains no credentials, sessions, prompts, histories, logs, database rows or personal policy.
Machine specifics live in `%USERPROFILE%\.agent-harness\.env`. Installing never overwrites owner-owned files
(`.env`, `policy\AGENTS.md`, `var\`); `harness compile` backs up every agent file before changing it
(`var\backups\compile-<timestamp>\`) and only touches managed hook entries, managed env keys and marked blocks.
Telemetry mapping reads whitelisted fields only; personal data carried in OpenTelemetry events is never stored.

## Install

Requirements: Python 3.11+ (uv-managed 3.14 preferred), Node.js 22+ (`npx` for rulesync), optionally Docker for the
observability hub. Missing tools are installed with `winget` unless `-Prerequisites ValidateOnly`.

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\install.ps1                 # install + compile + start receiver
.\scripts\install.ps1 -DryRun                                                   # plan only, writes nothing
.\scripts\install.ps1 -CompileDryRun                                            # install code, show config diff only
.\scripts\install.ps1 -LinkHerdrPlugin                                          # also link the local Herdr plugin
.\scripts\install.ps1 -WithHub                                                  # also deploy the observability hub (Docker)
.\scripts\doctor.ps1 -CheckCompile                                              # read-only health report
.\scripts\uninstall.ps1 [-Purge]
```

Edit `%USERPROFILE%\.agent-harness\.env` (see [.env.example](.env.example)) to list agent instances, e.g.
`INSTANCES="codex:personal=%USERPROFILE%\.codex;claude:default=%USERPROFILE%\.claude"`, then re-run compile:

```powershell
$env:PYTHONPATH="$env:USERPROFILE\.agent-harness"; python -m compile.run --dry-run   # review
$env:PYTHONPATH="$env:USERPROFILE\.agent-harness"; python -m compile.run             # apply (with backups)
```

Restart each agent afterwards. Authentication stays per agent home and is never shared.

Herdr is optional. To link the plugin after a regular install, run
`herdr plugin link "$env:USERPROFILE\.agent-harness\herdr-plugin"` and verify it with `herdr plugin list`.
`-LinkHerdrPlugin` performs the same link during installation and requires Herdr on `PATH` or
`HERDR_BIN_PATH` set to its executable. Install the harness first and link its installed plugin
directory so the plugin's commands use the configured `core` package and retained runtime state.
The plugin provides agent preflight and service startup;
Codex and Claude pane colors remain controlled by Herdr's screen detection. In Herdr, **Done**
means the finished pane has not yet been viewed; after viewing it, the state appears as **Idle**.

## What each agent gets

| Capability | How |
|---|---|
| Shared policy | `policy\AGENTS.md` (owner-edited) → Codex hard link, Claude `@import` |
| Observability | `obs` consumer → observability client → hub (`OBS_ENDPOINT`), offline spool |
| Token usage | built-in OpenTelemetry → local receiver (`127.0.0.1:OTLP_PORT`) → `turn_usage`; transcript fallback per turn |
| Notifications | done / needs-you toasts, deferred while background agents or shells still run; Herdr bell when inside Herdr |
| Background tasks | subagent start/stop hooks + transcript scan for background shells |
| Session context | hub project context injected at session start |
| Services | session start detaches a bootstrap that keeps the receiver, status orb, console monitor and hub running |

> **Token totals:** every turn is recorded twice on purpose — `source = '<agent>_otel'` (per API request, from
> OpenTelemetry) and `source = '<agent>_hook'` (per turn, from the transcript). Filter `turn_usage` by one source
> when summing, or you will double count. `_otel` is authoritative; `_hook` covers sessions without OTel.
> `harness usage` reports cumulative model token/cache counts. Its tool-result breakdown measures
> bytes, not per-tool cache hits, and it does not establish a before/after token saving for one run.

Diagnostics: `python -m core.notify.report --minutes 120` cross-references notifications with Windows toast
history and transcripts; router and consumer lifecycle logs live in `var\logs\`.

## Performance hypotheses

Validated tasks default to non-inferiority. Fewer than 30 valid samples always reports `insufficient_data`; only a
statistically confirmed regression can block packaging or push.

| Evidence (2026-09-29) | Result |
|---|---|
| [Windows verification CI](https://github.com/JohnAndrewBalbarosa/agent-harness/actions/runs/36411974575) on `cc27a89` | Passed the repository's unit, conformance, integration, installer, and secret-scan workflow. This validates that commit, not later local commits. |
| Harness-configured Claude versus Claude with user settings excluded | A three-task SWE-bench Verified pilot measured 3/3 versus 2/3 resolved, with 43.5% more agent runtime and 33.8% more reported model cost for the harness-configured arm. See the full report below; this does not isolate the hooks or establish a general performance advantage. |

```powershell
& "$env:USERPROFILE\.agent-harness\tools\observability-client\obs.cmd" benchmark catalog verify
& "$env:USERPROFILE\.agent-harness\tools\observability-client\obs.cmd" benchmark evaluate --profile personal --window 30 --format table
```

### Coding-agent pilot: observed strengths and costs

On 2026-09-29, six isolated Claude Code runs attempted three preselected public
[SWE-bench Verified](https://www.swebench.com/) pytest issues: one harness-configured run and one comparison run per issue. The
[official evaluator](https://github.com/SWE-bench/SWE-bench/tree/02e7a74ffd0b707aab73d203fe87bdc7c76afc8e)
graded the resulting patches. Its gold patch resolved each issue in the same environment before agent grading.

| Issue | Harness-configured | Comparison | Harness time / reported cost | Comparison time / reported cost |
|---|---|---|---:|---:|
| `pytest-dev__pytest-10051` | Resolved | Resolved | 265.39 s / $0.8281 | 157.62 s / $0.5510 |
| `pytest-dev__pytest-10081` | Resolved | Resolved | 322.32 s / $0.9915 | 235.23 s / $0.7432 |
| `pytest-dev__pytest-10356` | Resolved | Unresolved | 541.08 s / $1.2842 | 393.67 s / $1.0250 |
| **Total** | **3/3** | **2/3** | **1,128.79 s / $3.1037** | **786.52 s / $2.3191** |

The harness-configured arm took **342.27 s (43.5%) longer** and used **$0.7846 (33.8%) more** reported model cost. These are Claude CLI estimates, not an invoice. Its extra resolved issue was `pytest-dev__pytest-10356`: the comparison patch applied, but the official `testing/test_mark.py::test_mark_mro` test failed because `get_unpacked_marks(C)` returned a generator rather than the expected list. Neither arm had evaluator infrastructure failures or permission denials.

**What was held constant.** Each pair started from the issue's exact base commit in a separate clean pytest checkout. Both received the same issue text and instructions, used `claude --model sonnet` (resolved to `claude-sonnet-5`), `--permission-mode bypassPermissions`, `--no-session-persistence`, and a $3 per-run budget cap. Runs were sequential; wall time measures agent execution and excludes grading. The evaluator used one worker and a 600-second test timeout. It was pinned to commit `02e7a74ffd0b707aab73d203fe87bdc7c76afc8e`; a local Windows compatibility change wrote `eval.sh` and `patch.diff` with LF endings, and `PYTHONUTF8=1` prevented a Windows code-page output error. No benchmark tests or gold patches were changed.

**What differed.** The harness-configured arm loaded Claude's `user,project,local` settings; the comparison arm loaded `project,local`. This excluded user hooks in the comparison, but also excluded other user settings and enabled plugins. A user-level `CLAUDE.md` may still have applied to both. Consequently, the one-task correctness difference and the extra time/cost **cannot be attributed specifically to this harness's hooks**. A single run per issue also cannot separate a stable effect from run-to-run variation.

**Observability finding.** `obs analytics --days 1` linked three harness-arm prompts to usage and recorded 96 tool calls: 48 Bash, 16 Read, 15 Edit, 14 Grep, 2 Write, and 1 Skill. It recorded zero tool failures. All 96 tool-cache statuses were `unknown`, so this pilot shows no measured tool-cache hit benefit. The comparison arm's user hooks were excluded; its usage and cost came from Claude CLI JSON rather than the hub's tool-event view. The hub's benchmark catalog did not contain a registered paired benchmark program for this pilot.

**Assessment.** The configured setup solved one more issue in this sample, while the comparison was faster and cheaper on every issue. This supports keeping the observability needed to inspect runs, but it does not establish a quality improvement caused by the harness. Three issues from one repository, one run per arm, different user settings, and no third-party harness are too narrow for a general ranking. Further comparisons should isolate hooks from plugins, repeat runs, diversify repositories, and report verified resolution alongside time and cost. OpenAI's later [audit of coding benchmarks](https://openai.com/index/separating-signal-from-noise-coding-evaluations/) also finds major validity and contamination issues in SWE-bench Verified; this result is a local diagnostic pilot, not an industry-standard capability score.

Local evidence was retained outside Git in `var/benchmarks/pilot-20260929-060900/`: `selection.json` (task IDs and base commits), `predictions-{harness,native}.jsonl`, `agent-*.result.json`, `agent-*.summary.json`, and the evaluator's `logs/evaluation/pilot-{harness,native}/results.json`. Those files are not published with this README because the agent responses and traces can contain raw task context. The two evaluator summary JSON files have SHA-256 hashes `8f412b696210f18453b6266a863ca12896db6caf0608b5e469eb170229d7e84f` (harness) and `b4ebc856581502a74c0610e5f545cfe76c50370488565c88274caf6639516839` (comparison).

## Verification

```powershell
uv run --with scipy==1.18.1 python -m unittest discover -s tests -t . -v   # unit, conformance, integration (needs npx)
.\tests\test_installer.ps1
.\scripts\verify-secrets.ps1
```

Secret verification downloads the official Gitleaks v8.30.1 archive, verifies its checksum, runs a synthetic
detection canary, then scans the repository.

## Sources

- [Claude Code hooks](https://code.claude.com/docs/en/hooks) · [Claude Code monitoring (OpenTelemetry)](https://code.claude.com/docs/en/monitoring-usage)
- [Codex hooks input schemas](https://github.com/openai/codex/tree/main/codex-rs/hooks/schema/generated) · [Codex OpenTelemetry](https://github.com/openai/codex/tree/main/codex-rs/otel)
- [rulesync](https://github.com/dyoshikawa/rulesync) · [Custom instructions with AGENTS.md](https://agents.md/)
- [DORA software delivery performance metrics](https://dora.dev/guides/dora-metrics/)

## License

See [LICENSE](LICENSE).
