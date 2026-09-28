import json
import subprocess
import sys
import tempfile
import threading
import unittest
from pathlib import Path

from core import lifecycle

ROOT = Path(__file__).resolve().parents[2]
WRITER = """
import sys
sys.path.insert(0, sys.argv[1])
from pathlib import Path
from core import lifecycle
for i in range(40):
    lifecycle.emit(Path(sys.argv[2]), "test", "proc.write", "ok", worker=sys.argv[3], i=i, pad="x" * 300)
"""


class LifecycleTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.log = Path(self.dir.name) / "router.lifecycle.jsonl"

    def tearDown(self):
        self.dir.cleanup()

    def lines(self):
        return self.log.read_text(encoding="utf-8").splitlines()

    def test_emit_writes_structured_record(self):
        lifecycle.emit(self.log, "router", "hook.dispatch", "succeeded", agent="claude", note="ñ")
        record = json.loads(self.lines()[0])
        self.assertEqual((record["component"], record["operation"], record["outcome"]), ("router", "hook.dispatch", "succeeded"))
        self.assertEqual(record["details"], {"agent": "claude", "note": "ñ"})
        self.assertTrue(record["timestamp"].endswith("Z"))

    def test_concurrent_log_appends_are_line_atomic(self):
        def work(n):
            for i in range(50):
                lifecycle.emit(self.log, "t", "thread.write", "ok", worker=n, i=i, pad="y" * 200)
        threads = [threading.Thread(target=work, args=(n,)) for n in range(20)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        lines = self.lines()
        self.assertEqual(len(lines), 1000)
        for line in lines:
            json.loads(line)

    def test_concurrent_processes_do_not_interleave(self):
        procs = [subprocess.Popen([sys.executable, "-c", WRITER, str(ROOT), str(self.log), str(n)]) for n in range(6)]
        for p in procs:
            self.assertEqual(p.wait(timeout=60), 0)
        lines = self.lines()
        self.assertEqual(len(lines), 240)
        for line in lines:
            json.loads(line)

    def test_rotates_when_file_exceeds_limit(self):
        self.log.write_bytes(b"x" * (lifecycle.MAX_BYTES + 10))
        lifecycle.emit(self.log, "t", "after.rotate", "ok")
        self.assertEqual(len(self.lines()), 1)
        self.assertTrue(self.log.with_name(self.log.name + ".1").exists())

    def test_emit_never_raises(self):
        lifecycle.emit(Path(self.dir.name) / "no" / "such" / "\0bad", "t", "x", "y")


if __name__ == "__main__":
    unittest.main()
