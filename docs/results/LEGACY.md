# Legacy results (Python hooks + Node/PostgreSQL/MongoDB hub)

Scope: results recorded **before 2026-09-30**, when the observability hub ran as Node (Fastify) + PostgreSQL 17 +
MongoDB 7 in Docker, and every hook ran the Python router. Kept verbatim for comparison; current results live in
[`UPDATES.md`](UPDATES.md).

## Runtime baseline at cutover (2026-09-29, measured)

| Metric | Legacy value | Source |
|---|---:|---|
| Observability stack RAM | ~750 MB (Postgres 109 MB, MongoDB 63 MB, Docker Desktop ~233 MB, WSL VM ~345 MB) | `docker stats`, `Get-Process` |
| MongoDB data | ~0 MB (technology memory was dead code: `technology-memory.mjs` never imported) | `listDatabases` |
| PostgreSQL data | 96 MB, 34 tables, 28,824 rows at export | `pg_database_size`, export manifest |
| Free RAM on the 7.7 GB workstation | 0.5-0.8 GB | `Win32_OperatingSystem` |
| Hook dispatch latency, 19:30-21:30Z (n=945) | p50 667 ms, p95 2,023 ms, max 18,343 ms | `var/logs/hook-router.lifecycle.jsonl` |
| `obs` consumer latency, same window (n=817) | p50 699 ms, p95 2,137 ms, max 18,310 ms | same |

## Performance hypotheses (legacy)

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
