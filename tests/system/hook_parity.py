"""One-time whole-system parity: Python hook router (side A) vs Rust `harness-hook.exe` (side B).

Run:  python tests/system/hook_parity.py [--keep] [--only SUBSTRING] [--max-lines 60]

Both sides run the same hook steps sequentially in sandboxed harness roots (see parity_sandbox) with a dead obs
endpoint (http://127.0.0.1:9), dry-run notifications and neutralised background launches. After every step the
harness compares stdout bytes, exit codes, every written state file, the spool (queue + dead_letter as a multiset of
(kind, masked data) and the context table) and the new lifecycle-log records. Output is bounded and never prints
values; the last line is a JSON summary, and the exit code is non-zero on unexpected mismatches.
"""
from __future__ import annotations

import argparse
import json
import statistics
import subprocess
import sys
import time
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from parity_compare import (LogCursor, Mismatch, baseline_listing, capture_state, compare_log_deltas,  # noqa: E402
                            compare_states, launched_counts)
from parity_sandbox import (Sandbox, SandboxPair, build_sandbox_pair, hook_command, hook_environment,  # noqa: E402
                            remove_sandbox_pair)
from parity_scenarios import Scenario, Step, build_scenarios, build_workspace  # noqa: E402

STEP_TIMEOUT_S = 180
DETACHED_SETTLE_S = 4.0
EXPECTED_LAUNCH_DEVIATIONS = {
    "toast": "context-guard honours AH_NOTIFY_DRY_RUN in Rust (Python always toasts)",
    "flush_spool": "obs: sync flush removed; detached flush throttled to one per 3 s (always at turn/session end)",
}
# Files only the Rust side creates, by design (review fixes after the first parity run).
EXPECTED_RUST_ONLY_FILES = {
    "flush-spawn.stamp": "obs: flush throttle stamp",
    "hook-invocations.jsonl.lock": "obs: invocation audit rewrite is now locked (concurrent hooks lost lines)",
}


@dataclass
class Finding:
    scenario: str
    step: str
    mismatch: Mismatch
    expected: str | None


@dataclass
class ParityReport:
    scenarios: int = 0
    steps: int = 0
    findings: list[Finding] = field(default_factory=list)
    signatures: set[tuple[str, str, str | None]] = field(default_factory=set)
    reordered_steps: int = 0
    latency: dict[str, dict[str, list[float]]] = field(default_factory=lambda: defaultdict(lambda: {"A": [], "B": []}))

    def add(self, scenario: Scenario, step: Step, mismatch: Mismatch, expected: str | None) -> None:
        signature = (strip_record_index(mismatch.path), mismatch.detail, expected)
        if signature in self.signatures:
            return
        self.signatures.add(signature)
        self.findings.append(Finding(scenario.name, step.label, mismatch, expected))

    @property
    def unexpected(self) -> list[Finding]:
        return [f for f in self.findings if f.expected is None]

    @property
    def expected(self) -> list[Finding]:
        return [f for f in self.findings if f.expected is not None]


@dataclass
class StepRun:
    stdout: bytes
    exit_code: int | None
    seconds: float


def main(argv: list[str] | None = None) -> int:
    options = parse_arguments(argv)
    pair = build_sandbox_pair(options.python_sync_flush)
    try:
        workspace = build_workspace(pair.base)
        scenarios = [s for s in build_scenarios(workspace) if options.only in s.name]
        report = run_parity(pair, scenarios)
        compare_detached_launches(pair, report)
        print_report(report, options.max_lines)
        return 0 if not report.unexpected else 1
    finally:
        finish_sandboxes(pair, options.keep)


def parse_arguments(argv: list[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--keep", action="store_true", help="keep the sandboxes (junctions are still removed)")
    parser.add_argument("--only", default="", help="run scenarios whose name contains this text")
    parser.add_argument("--python-sync-flush", action="store_true",
                        help="keep obs.py inline flush on side A (dead endpoint → obs consumer 20 s kills)")
    parser.add_argument("--max-lines", type=int, default=60, help="maximum mismatch lines printed")
    return parser.parse_args(argv)


# Mental model: per scenario, per step: run A, run B → compare outputs, state, log deltas → record findings.
def run_parity(pair: SandboxPair, scenarios: list[Scenario]) -> ParityReport:
    report = ParityReport(scenarios=len(scenarios))
    baselines = {s.side: baseline_listing(s.root) for s in pair.sides}
    cursors = {s.side: LogCursor(s.root) for s in pair.sides}
    diverged: set[str] = set()
    for scenario in scenarios:
        for step in scenario.steps:
            runs = {s.side: run_step(s, pair, step) for s in pair.sides}
            report.steps += 1
            for side, run in runs.items():
                report.latency[step.label][side].append(run.seconds)
            mismatches, diverged = compare_step(pair, runs, baselines, cursors, report, diverged)
            for mismatch in mismatches:
                report.add(scenario, step, mismatch, classify(scenario, mismatch))
    return report


def run_step(sandbox: Sandbox, pair: SandboxPair, step: Step) -> StepRun:
    started = time.perf_counter()
    try:
        done = subprocess.run(hook_command(sandbox, step.agent, step.native_event), input=step.stdin,
                              capture_output=True, cwd=sandbox.root, timeout=STEP_TIMEOUT_S,
                              env=hook_environment(sandbox, pair.codex_home, step.agent))
        return StepRun(done.stdout, done.returncode, time.perf_counter() - started)
    except subprocess.TimeoutExpired:
        return StepRun(b"", None, time.perf_counter() - started)


# Mental model: outputs + log deltas are per step; state is cumulative, so only paths that newly diverge count.
def compare_step(pair: SandboxPair, runs: dict[str, StepRun], baselines: dict, cursors: dict,
                 report: ParityReport, diverged: set[str]) -> tuple[list[Mismatch], set[str]]:
    a, b = pair.python_side, pair.rust_side
    found = compare_outputs(runs["A"], runs["B"])
    state = compare_states(capture_state(a.root, baselines["A"]), capture_state(b.root, baselines["B"]))
    found += [m for m in state if m.path not in diverged]
    log_mismatches, reordered = compare_log_deltas(cursors["A"].new_lines(), cursors["B"].new_lines(),
                                                   str(a.root), str(b.root))
    report.reordered_steps += reordered
    return found + log_mismatches, {m.path for m in state}


def compare_outputs(a: StepRun, b: StepRun) -> list[Mismatch]:
    found = []
    if a.stdout != b.stdout:
        offset = next((i for i, (x, y) in enumerate(zip(a.stdout, b.stdout)) if x != y), min(len(a.stdout), len(b.stdout)))
        found.append(Mismatch("stdout", f"bytes A={len(a.stdout)} B={len(b.stdout)} first difference at {offset}"))
    for side, run in (("A", a), ("B", b)):
        if run.exit_code != 0:
            found.append(Mismatch(f"exit[{side}]", f"exit code {run.exit_code} (None = timeout)"))
    return found


def classify(scenario: Scenario, mismatch: Mismatch) -> str | None:
    if scenario.expected_deviation:
        return scenario.expected_deviation
    if "turn-usage" in mismatch.path and mismatch.path.endswith("projectId"):
        return ("usage-fallback: Rust serialises spool access, so usage gets the session's project; Python "
                "raced obs and fell back to the global project")
    if mismatch.detail == "file only in B":
        for suffix, reason in EXPECTED_RUST_ONLY_FILES.items():
            if mismatch.path.endswith(suffix):
                return reason
    return None


def compare_detached_launches(pair: SandboxPair, report: ParityReport) -> None:
    time.sleep(DETACHED_SETTLE_S)
    counts_a, counts_b = launched_counts(pair.python_side.launched), launched_counts(pair.rust_side.launched)
    marker = Scenario("detached-launches", ())
    step = Step("-", None, b"")
    for label in sorted(set(counts_a) | set(counts_b)):
        if counts_a[label] != counts_b[label]:
            mismatch = Mismatch(f"var/launched.txt[{label}]", f"launch count A={counts_a[label]} B={counts_b[label]}")
            report.add(marker, step, mismatch, EXPECTED_LAUNCH_DEVIATIONS.get(label))


# ---------------------------------------------------------------- reporting

def print_report(report: ParityReport, max_lines: int) -> None:
    lines = [format_finding("UNEXPECTED", f) for f in report.unexpected]
    lines += [format_finding(f"EXPECTED({f.expected})", f) for f in report.expected]
    for line in lines[:max_lines]:
        print(line)
    if len(lines) > max_lines:
        print(f"... {len(lines) - max_lines} more mismatch lines suppressed")
    if report.reordered_steps:
        print(f"EXPECTED(consumer.exited order: registry vs completion) normalised in {report.reordered_steps} step(s)")
    print_latency(report)
    summary = {"scenarios": report.scenarios, "steps": report.steps, "mismatches": len(report.unexpected),
               "expected": len(report.expected), "ok": not report.unexpected}
    print(json.dumps(summary))


def format_finding(kind: str, finding: Finding) -> str:
    return f"{kind} {finding.scenario} [{finding.step}] {finding.mismatch.path}: {finding.mismatch.detail}"


def print_latency(report: ParityReport) -> None:
    print(f"{'event':34} {'n':>3} {'A p50':>8} {'A p95':>8} {'B p50':>8} {'B p95':>8}  (ms)")
    for label in sorted(report.latency):
        a, b = report.latency[label]["A"], report.latency[label]["B"]
        print(f"{label[:34]:34} {len(a):>3} {pct(a, 50):>8.0f} {pct(a, 95):>8.0f} {pct(b, 50):>8.0f} {pct(b, 95):>8.0f}")


def pct(samples: list[float], percentile: int) -> float:
    if not samples:
        return 0.0
    if len(samples) == 1:
        return samples[0] * 1000
    return statistics.quantiles(samples, n=100, method="inclusive")[percentile - 1] * 1000


def strip_record_index(path: str) -> str:
    head, _, tail = path.partition("#")
    return head + ("#" + tail.split(" ", 1)[1] if " " in tail else "") if tail else head


def finish_sandboxes(pair: SandboxPair, keep: bool) -> None:
    if keep:
        from parity_sandbox import remove_junctions
        problems = [p for s in pair.sides for p in remove_junctions(s)]
        print(f"sandboxes kept at {pair.base}")
    else:
        problems = remove_sandbox_pair(pair)
    for problem in problems:
        print(f"CLEANUP {problem}")


if __name__ == "__main__":
    raise SystemExit(main())
