import importlib
import io
import json
import sys
import unittest
from pathlib import Path
from unittest import mock

ORB = Path(__file__).resolve().parents[2] / "tools" / "codex-status-orb"


def load_hook():
    sys.path.insert(0, str(ORB))
    try:
        return importlib.import_module("hook")
    finally:
        sys.path.remove(str(ORB))


class OrbHookTests(unittest.TestCase):
    def run_hook(self, env):
        hook = load_hook()
        payload = json.dumps({"hook_event_name": "SessionStart", "session_id": "s1", "cwd": "C:/x"})
        with mock.patch.object(hook, "launch_orb") as launch, mock.patch.object(hook, "write_session") as write, \
             mock.patch.object(hook, "claim_hook_event", return_value=True), mock.patch.object(hook, "checkpoint"), \
             mock.patch.object(hook, "process_context", return_value={}), mock.patch.dict("os.environ", env), \
             mock.patch.object(sys, "stdin", io.StringIO(payload)):
            hook.main()
        return launch, write

    def test_state_is_written_but_no_window_when_ui_disabled(self):
        launch, write = self.run_hook({"ORB_UI": "0"})
        write.assert_called_once()
        launch.assert_not_called()

    def test_window_launches_by_default(self):
        launch, _ = self.run_hook({"ORB_UI": ""})
        launch.assert_called_once()


if __name__ == "__main__":
    unittest.main()
