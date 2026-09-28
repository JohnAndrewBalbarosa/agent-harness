import importlib
import io
import json
import sys
import unittest
from pathlib import Path
from unittest import mock

ORB = Path(__file__).resolve().parents[2] / "tools" / "codex-status-orb"


def load(module):
    sys.path.insert(0, str(ORB))
    try:
        return importlib.import_module(module)
    finally:
        sys.path.remove(str(ORB))


class OrbHookTests(unittest.TestCase):
    def test_session_start_always_starts_the_orb_process(self):
        # The orb process owns sounds and session recovery; ORB_UI=0 only makes it headless.
        hook = load("hook")
        payload = json.dumps({"hook_event_name": "SessionStart", "session_id": "s1", "cwd": "C:/x"})
        with mock.patch.object(hook, "launch_orb") as launch, mock.patch.object(hook, "write_session") as write, \
             mock.patch.object(hook, "claim_hook_event", return_value=True), mock.patch.object(hook, "checkpoint"), \
             mock.patch.object(hook, "process_context", return_value={}), mock.patch.dict("os.environ", {"ORB_UI": "0"}), \
             mock.patch.object(sys, "stdin", io.StringIO(payload)):
            hook.main()
        write.assert_called_once()
        launch.assert_called_once()


class OrbModeTests(unittest.TestCase):
    def test_headless_from_flag_or_environment(self):
        mode = load("mode")
        self.assertTrue(mode.headless_requested(["orb.py", "--headless"], {}))
        self.assertTrue(mode.headless_requested(["orb.py"], {"ORB_UI": "0"}))
        self.assertFalse(mode.headless_requested(["orb.py"], {"ORB_UI": "1"}))
        self.assertFalse(mode.headless_requested(["orb.py"], {}))


if __name__ == "__main__":
    unittest.main()
