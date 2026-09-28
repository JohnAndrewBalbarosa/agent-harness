import json
import subprocess
import sys
import tempfile
import unittest
from collections import deque
from pathlib import Path

from interval_supervisor import (
    completion_record,
    meaningful_tail,
    normalize_line,
    select_failure_summary,
)


ROOT = Path(__file__).resolve().parents[1]
RUNNER = ROOT / "interval_supervisor.py"


class IntervalSupervisorTests(unittest.TestCase):
    def test_normalize_line_keeps_final_progress_frame_and_strips_ansi(self):
        self.assertEqual(
            normalize_line("loading 90%\r\x1b[32mAll tests passed!\x1b[0m\n", 200),
            "All tests passed!",
        )

    def test_meaningful_tail_deduplicates_consecutive_progress(self):
        self.assertEqual(
            meaningful_tail(["running", "running", "done"], 2),
            ["running", "done"],
        )

    def test_success_record_returns_only_configured_tail(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            record = completion_record(
                label="test",
                exit_code=0,
                duration_seconds=1.2345,
                stdout_lines=deque(["progress", "All tests passed!"]),
                stderr_lines=deque(),
                success_tail_lines=1,
                failure_tail_lines=20,
                stdout_path=root / "out.log",
                stderr_path=root / "err.log",
            )
        self.assertEqual(record["status"], "succeeded")
        self.assertEqual(record["exit_code"], 0)
        self.assertEqual(record["tail"], ["All tests passed!"])

    def test_failure_record_keeps_bounded_context_from_both_streams(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            record = completion_record(
                label="test",
                exit_code=7,
                duration_seconds=2,
                stdout_lines=deque(["progress", "Error: server timed out"]),
                stderr_lines=deque(["warning", "warning details"]),
                success_tail_lines=2,
                failure_tail_lines=2,
                stdout_path=root / "out.log",
                stderr_path=root / "err.log",
            )
        self.assertEqual(record["status"], "failed")
        self.assertEqual(record["exit_code"], 7)
        self.assertEqual(record["tail"], ["Error: server timed out", "warning details"])
        self.assertEqual(record["summary"], "Error: server timed out")

    def test_failure_summary_prefers_specific_error_over_aggregate(self):
        self.assertEqual(
            select_failure_summary(
                ["Error: page context was destroyed", "trace.zip", "3 failed"]
            ),
            "Error: page context was destroyed",
        )

    def test_runner_emits_one_json_summary_and_bounds_persisted_output(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            command = [
                sys.executable,
                str(RUNNER),
                "--label",
                "bounded-success",
                "--interval",
                "5",
                "--cwd",
                str(root),
                "--log-dir",
                "logs",
                "--max-log-lines",
                "3",
                "--success-tail-lines",
                "1",
                "--",
                sys.executable,
                "-c",
                "print('one'); print('two'); print('three'); print('SUCCESS')",
            ]
            result = subprocess.run(command, text=True, capture_output=True, check=False)
            record = json.loads(result.stdout)
            persisted = (root / "logs" / "bounded-success.stdout.log").read_text(
                encoding="utf-8"
            )

        self.assertEqual(result.returncode, 0)
        self.assertEqual(record["status"], "succeeded")
        self.assertEqual(record["exit_code"], 0)
        self.assertEqual(record["tail"], ["SUCCESS"])
        self.assertNotIn("one", persisted)
        self.assertEqual(persisted.splitlines(), ["two", "three", "SUCCESS"])


if __name__ == "__main__":
    unittest.main()
