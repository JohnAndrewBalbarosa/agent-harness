import json
import subprocess
import unittest

from core.notify.decide import Decision
from core.notify.deliver import deliver

DONE = Decision("toast", "done", "done", "Tapos na ang prompt!")


class FakeRun:
    def __init__(self, herdr_stdout=b'{"result":{"shown":true,"reason":"shown"}}', fail=None):
        self.calls, self.herdr_stdout, self.fail = [], herdr_stdout, fail or set()

    def __call__(self, args, **kwargs):
        self.calls.append(list(args))
        kind = "herdr" if "notification" in args else "windows"
        if kind in self.fail:
            raise OSError("launch failed")
        return subprocess.CompletedProcess(args, 0, self.herdr_stdout if kind == "herdr" else b"", b"")


class DeliverTests(unittest.TestCase):
    def test_outside_herdr_windows_toast_with_sound(self):
        run = FakeRun()
        result = deliver(DONE, "Claude Code", None, run=run)
        self.assertEqual(result, "windows:handed")
        self.assertEqual(len(run.calls), 1)
        self.assertNotIn("-Silent", run.calls[0])
        self.assertIn("Tapos na ang prompt!", run.calls[0])

    def test_inside_herdr_bell_plus_silent_windows_popup(self):
        run = FakeRun()
        result = deliver(DONE, "Codex (cy)", "C:/h/herdr.exe", run=run)
        self.assertEqual(result, "herdr:shown + windows:handed(silent)")
        herdr_call, windows_call = run.calls
        self.assertEqual(herdr_call[:4], ["C:/h/herdr.exe", "notification", "show", "Codex (cy)"])
        self.assertEqual(herdr_call[herdr_call.index("--sound") + 1], "done")
        self.assertIn("-Silent", windows_call)

    def test_herdr_not_shown_falls_back_to_sounded_windows_toast(self):
        run = FakeRun(herdr_stdout=json.dumps({"result": {"shown": False, "reason": "disabled"}}).encode())
        result = deliver(DONE, "Claude Code", "herdr.exe", run=run)
        self.assertEqual(result, "herdr:not-shown(disabled) + windows:handed")
        self.assertNotIn("-Silent", run.calls[-1])

    def test_herdr_failure_falls_back(self):
        result = deliver(DONE, "Claude Code", "herdr.exe", run=FakeRun(fail={"herdr"}))
        self.assertEqual(result, "herdr:error(OSError) + windows:handed")

    def test_windows_failure_is_reported_not_raised(self):
        self.assertEqual(deliver(DONE, "Claude Code", None, run=FakeRun(fail={"windows"})), "windows:error(OSError)")

    def test_request_sound_is_forwarded(self):
        run = FakeRun()
        deliver(Decision("toast", "request", "request", "Kailangan ng input mo!"), "Claude Code", "herdr.exe", run=run)
        self.assertEqual(run.calls[0][run.calls[0].index("--sound") + 1], "request")


if __name__ == "__main__":
    unittest.main()
