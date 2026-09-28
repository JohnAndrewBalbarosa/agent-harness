"""Focused hook-to-operation test with an isolated spool."""
import importlib.util
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


obs = load("analytics_obs", ROOT / "obs.py")
hook = load("analytics_hook", ROOT / "hook.py")


class AnalyticsHookTests(unittest.TestCase):
    def test_native_prompt_link_and_tool_record(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = {"id": "00000000-0000-7000-8000-000000000001", "name": "Test", "slug": "test"}
            batches = []
            with patch.object(obs, "VAR", root / "var"), patch.object(obs, "QUEUE", root / "var" / "spool.sqlite3"), \
                 patch.object(obs, "KEY", root / "var" / "hmac.key"), patch.object(hook, "obs", obs), \
                 patch.object(obs, "project_config", return_value=(root / "observability.project.toml", {"project": project})), \
                 patch.object(obs, "register_project", return_value=project), \
                 patch.object(obs, "enqueue", side_effect=lambda operations, **kwargs: batches.append(operations)), \
                 patch.object(hook, "schedule_flush"):
                base = {"session_id": "native-session", "cwd": str(root)}
                hook.process({**base, "hook_event_name": "UserPromptSubmit", "turn_id": "turn-1", "prompt": "private prompt"})
                link = next(op["data"] for op in batches[-1] if op["kind"] == "prompt.native-link")
                self.assertEqual(link["nativeTurn"], "turn-1")
                hook.process({**base, "hook_event_name": "PostToolUse", "turn_id": "turn-1",
                              "tool_use_id": "tool-1", "tool_name": "mcp__demo__run",
                              "tool_response": {"is_error": True, "secret": "private response"}})
                record = next(op["data"] for op in batches[-1] if op["kind"] == "tool-use.record")
                self.assertEqual((record["promptId"], record["toolUseId"], record["failed"]),
                                 (link["promptId"], "tool-1", True))
                self.assertNotIn("private response", str(record))


if __name__ == "__main__":
    unittest.main()
