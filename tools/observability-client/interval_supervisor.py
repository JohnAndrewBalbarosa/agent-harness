"""Low-chatter process runner with bounded, semantic completion output.

The child process never streams into the Codex session. Output is drained into
bounded in-memory tails, persisted as bounded diagnostic files, and reduced to
one JSON completion record. Successful runs expose only their final meaningful
lines; failed runs expose a bounded diagnostic tail.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import threading
import time
from collections import deque
from pathlib import Path
from typing import TextIO

from sdk.python import ObservabilityClient


ANSI_ESCAPE = re.compile(r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1b\\))")


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run a command and return one bounded JSON completion summary"
    )
    parser.add_argument("--label", required=True)
    parser.add_argument(
        "--interval", type=int, default=30, help="seconds between bounded checks"
    )
    parser.add_argument("--log-dir", default="var/logs")
    parser.add_argument("--cwd", default=os.getcwd())
    parser.add_argument("--project-id", default="")
    parser.add_argument(
        "--max-log-lines",
        type=int,
        default=200,
        help="maximum retained lines for each output stream",
    )
    parser.add_argument(
        "--max-line-chars", type=int, default=2000, help="maximum characters per line"
    )
    parser.add_argument(
        "--success-tail-lines",
        type=int,
        default=2,
        help="meaningful lines returned for a successful command",
    )
    parser.add_argument(
        "--failure-tail-lines",
        type=int,
        default=20,
        help="meaningful lines returned for a failed command",
    )
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    if not args.command or args.command[0] != "--":
        parser.error("command must follow --")
    args.command = args.command[1:]
    if args.interval < 5:
        parser.error("interval must be at least 5 seconds")
    for name in (
        "max_log_lines",
        "max_line_chars",
        "success_tail_lines",
        "failure_tail_lines",
    ):
        if getattr(args, name) < 1:
            parser.error(f"--{name.replace('_', '-')} must be at least 1")
    return args


def normalize_line(value: str, maximum: int) -> str:
    """Keep the final carriage-return frame and remove terminal control noise."""
    frames = value.replace("\r\n", "\n").split("\r")
    line = ANSI_ESCAPE.sub("", frames[-1]).strip()
    if len(line) > maximum:
        line = f"{line[: maximum - 1]}…"
    return line


def meaningful_tail(lines: list[str], limit: int) -> list[str]:
    result: deque[str] = deque(maxlen=limit)
    for line in lines:
        if not line:
            continue
        if result and line == result[-1]:
            continue
        result.append(line)
    return list(result)


def select_failure_summary(lines: list[str]) -> str:
    """Prefer a specific failure over trailing aggregate or artifact chatter."""
    ranked_patterns = (
        (100, re.compile(r"^error\s*:", re.IGNORECASE)),
        (90, re.compile(r"\b(exception|fatal|traceback)\b", re.IGNORECASE)),
        (80, re.compile(r"\b(timed out|timeout|assertion)\b", re.IGNORECASE)),
        (20, re.compile(r"\b(failed|failure)\b", re.IGNORECASE)),
    )
    best_score = -1
    best_line = ""
    for line in lines:
        for score, pattern in ranked_patterns:
            if pattern.search(line) and score >= best_score:
                best_score = score
                best_line = line
                break
    return best_line


def drain_stream(
    stream: TextIO,
    destination: deque[str],
    maximum_line_chars: int,
) -> None:
    try:
        for raw_line in iter(stream.readline, ""):
            line = normalize_line(raw_line, maximum_line_chars)
            if line:
                destination.append(line)
    finally:
        stream.close()


def persist_tail(path: Path, lines: deque[str]) -> None:
    content = "\n".join(lines)
    path.write_text(f"{content}\n" if content else "", encoding="utf-8")


def completion_record(
    *,
    label: str,
    exit_code: int,
    duration_seconds: float,
    stdout_lines: deque[str],
    stderr_lines: deque[str],
    success_tail_lines: int,
    failure_tail_lines: int,
    stdout_path: Path,
    stderr_path: Path,
) -> dict[str, object]:
    succeeded = exit_code == 0
    if succeeded:
        preferred = list(stdout_lines if stdout_lines else stderr_lines)
        tail = meaningful_tail(preferred, success_tail_lines)
        summary = tail[-1] if tail else "completed"
    else:
        # Toolchains disagree about which stream owns the actionable failure.
        # Preserve a bounded share of both so benign stderr warnings cannot hide
        # a real stdout error (or the reverse).
        stdout_limit = max(1, failure_tail_lines // 2)
        stderr_limit = max(1, failure_tail_lines - stdout_limit)
        tail = meaningful_tail(list(stdout_lines), stdout_limit)
        tail.extend(meaningful_tail(list(stderr_lines), stderr_limit))
        summary = select_failure_summary(list(stdout_lines) + list(stderr_lines))
        if not summary:
            summary = tail[-1] if tail else "command failed"
    return {
        "label": label,
        "status": "succeeded" if succeeded else "failed",
        "exit_code": exit_code,
        "duration_seconds": round(duration_seconds, 3),
        "summary": summary,
        "tail": tail,
        "stdout_log": str(stdout_path),
        "stderr_log": str(stderr_path),
    }


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    root = Path(args.cwd).resolve()
    log_dir = (root / args.log_dir).resolve()
    log_dir.mkdir(parents=True, exist_ok=True)
    safe_label = "".join(
        ch if ch.isalnum() or ch in "-_" else "_" for ch in args.label
    )
    out_path = log_dir / f"{safe_label}.stdout.log"
    err_path = log_dir / f"{safe_label}.stderr.log"
    state_path = log_dir / f"{safe_label}.state.json"
    stdout_lines: deque[str] = deque(maxlen=args.max_log_lines)
    stderr_lines: deque[str] = deque(maxlen=args.max_log_lines)
    started_at = time.time()
    client = None
    if args.project_id:
        client = ObservabilityClient.start(
            args.project_id,
            component="codex.interval_supervisor",
            execution_kind=2,
        )
        client.emit(
            "process.started",
            message=f"Started {safe_label}",
            attributes={"label": safe_label, "intervalSeconds": args.interval},
        )

    try:
        proc = subprocess.Popen(
            args.command,
            cwd=root,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            stdin=subprocess.DEVNULL,
            text=True,
            encoding="utf-8",
            errors="replace",
            bufsize=1,
        )
    except Exception as error:
        record = {
            "label": safe_label,
            "status": "failed",
            "exit_code": None,
            "duration_seconds": round(time.time() - started_at, 3),
            "summary": f"command could not start: {type(error).__name__}",
            "tail": [str(error)[: args.max_line_chars]],
            "stdout_log": str(out_path),
            "stderr_log": str(err_path),
        }
        err_path.write_text(record["tail"][0] + "\n", encoding="utf-8")
        out_path.write_text("", encoding="utf-8")
        state_path.write_text(json.dumps(record, indent=2), encoding="utf-8")
        print(json.dumps(record, separators=(",", ":")))
        if client:
            client.emit(
                "process.failed",
                category=4,
                severity=3,
                outcome=3,
                message=f"Could not start {safe_label}",
                attributes={"label": safe_label, "exceptionType": type(error).__name__},
            )
            client.finish(outcome=3, status=4, result_summary=f"{safe_label} failed")
        return 1

    state = {
        "label": safe_label,
        "pid": proc.pid,
        "status": "running",
        "intervalSeconds": args.interval,
        "stdout": str(out_path),
        "stderr": str(err_path),
        "startedAt": started_at,
    }
    state_path.write_text(json.dumps(state, indent=2), encoding="utf-8")
    assert proc.stdout is not None
    assert proc.stderr is not None
    stdout_thread = threading.Thread(
        target=drain_stream,
        args=(proc.stdout, stdout_lines, args.max_line_chars),
        daemon=True,
    )
    stderr_thread = threading.Thread(
        target=drain_stream,
        args=(proc.stderr, stderr_lines, args.max_line_chars),
        daemon=True,
    )
    stdout_thread.start()
    stderr_thread.start()

    while True:
        try:
            exit_code = proc.wait(timeout=args.interval)
            break
        except subprocess.TimeoutExpired:
            # Keep bounded diagnostics inspectable during long-running services
            # without streaming their output into the Codex session.
            persist_tail(out_path, stdout_lines)
            persist_tail(err_path, stderr_lines)
            if client:
                client.emit(
                    "process.checkpoint",
                    message=f"Still running: {safe_label}",
                    attributes={
                        "label": safe_label,
                        "pid": proc.pid,
                        "intervalSeconds": args.interval,
                    },
                )

    stdout_thread.join(timeout=2)
    stderr_thread.join(timeout=2)
    persist_tail(out_path, stdout_lines)
    persist_tail(err_path, stderr_lines)
    record = completion_record(
        label=safe_label,
        exit_code=exit_code,
        duration_seconds=time.time() - started_at,
        stdout_lines=stdout_lines,
        stderr_lines=stderr_lines,
        success_tail_lines=args.success_tail_lines,
        failure_tail_lines=args.failure_tail_lines,
        stdout_path=out_path,
        stderr_path=err_path,
    )
    state.update(record)
    state_path.write_text(json.dumps(state, indent=2), encoding="utf-8")
    print(json.dumps(record, separators=(",", ":")))

    if client:
        if exit_code == 0:
            client.emit(
                "process.completed",
                outcome=1,
                message=f"Completed {safe_label}",
                attributes={
                    "label": safe_label,
                    "exitCode": exit_code,
                    "durationSeconds": record["duration_seconds"],
                    "summary": record["summary"],
                },
            )
            client.finish(result_summary=f"{safe_label} completed")
        else:
            client.emit(
                "process.failed",
                category=4,
                severity=3,
                outcome=3,
                message=f"Failed {safe_label}; inspect bounded diagnostic tail",
                attributes={
                    "label": safe_label,
                    "exitCode": exit_code,
                    "durationSeconds": record["duration_seconds"],
                    "diagnosticTail": record["tail"],
                },
            )
            client.finish(
                outcome=3,
                status=4,
                result_summary=f"{safe_label} failed",
            )
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
