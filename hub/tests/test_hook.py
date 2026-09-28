import importlib.util
import io
import os
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).parents[1]
OBS_SPEC = importlib.util.spec_from_file_location("obs", ROOT / "obs.py")
obs = importlib.util.module_from_spec(OBS_SPEC)
OBS_SPEC.loader.exec_module(obs)
HOOK_SPEC = importlib.util.spec_from_file_location("hook", ROOT / "hook.py")
hook = importlib.util.module_from_spec(HOOK_SPEC)
HOOK_SPEC.loader.exec_module(hook)


class HookLifecycleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.config = self.root / "observability.project.toml"
        self.project = {"id":"00000000-0000-7000-8000-000000000001","name":"Test","slug":"test"}
        self.config.write_text('[project]\nid="00000000-0000-7000-8000-000000000001"\nname="Test"\nslug="test"\nschema_version=1\n', encoding="utf-8")
        self.codex_home = self.root / "codex-home"
        self.codex_home.mkdir()
        self.codex_home.joinpath("AGENTS.md").write_text("test global instructions", encoding="utf-8")
        self.old = (obs.VAR, obs.QUEUE, obs.KEY, hook.HOOK_AUDIT_PATH)
        obs.VAR, obs.QUEUE, obs.KEY = self.root / "var", self.root / "var" / "spool.sqlite3", self.root / "var" / "hmac.key"
        hook.HOOK_AUDIT_PATH = self.root / "hook-invocations.jsonl"
        hook.obs = obs
        self.batches = []

    def tearDown(self):
        obs.VAR, obs.QUEUE, obs.KEY, hook.HOOK_AUDIT_PATH = self.old
        self.temp.cleanup()

    def invoke(self, event, **extra):
        payload = {"hook_event_name":event,"session_id":"native-session","cwd":str(self.root),**extra}
        with patch.object(hook, "payload", return_value=payload), \
             patch.object(obs, "project_config", return_value=(self.config,{"project":self.project})), \
             patch.object(obs, "register_project", return_value=self.project), \
             patch.object(obs, "enqueue", side_effect=lambda operations: self.batches.append(operations)), \
             patch.object(obs, "git_value", side_effect=lambda _root,*args: "abc123" if args == ("rev-parse","HEAD") else ("master" if args == ("branch","--show-current") else "")), \
             patch.object(obs, "worktree_fingerprint", return_value="ab" * 32), \
             patch.dict(os.environ,{"CODEX_OBS_INSTANCE":"personal","CODEX_OBS_HOME":str(self.codex_home)}), \
             redirect_stdout(io.StringIO()):
            self.assertEqual(hook.main(), 0)

    def row(self):
        conn = obs.db()
        try:
            return conn.execute("SELECT * FROM context").fetchone()
        finally:
            conn.close()

    def test_uninstrumented_session_uses_global_project_without_marker(self):
        payload = {"hook_event_name":"SessionStart","session_id":"plain","cwd":str(self.root / "plain")}
        global_value = {"id":obs.GLOBAL_PROJECT_ID,"name":"Codex Global Sessions","slug":"codex-global-sessions"}
        batches = []
        with patch.object(hook, "payload", return_value=payload), \
             patch.object(hook, "load_obs", return_value=obs), \
             patch.object(obs, "project_config", return_value=None), \
             patch.object(hook, "global_project", return_value=global_value), \
             patch.object(obs, "enqueue", side_effect=lambda operations: batches.append(operations)), \
             redirect_stdout(io.StringIO()):
            self.assertEqual(hook.main(), 0)
        self.assertTrue(any(op["kind"] == "session.start" for op in batches[-1]))
        self.assertFalse((self.root / "plain" / "observability.project.toml").exists())

    def test_hook_self_audit_records_trigger_outcome_and_duration_without_payload(self):
        self.invoke("SessionStart", source="startup", secret="must-not-be-copied")
        records = hook.HOOK_AUDIT_PATH.read_text(encoding="utf-8").splitlines()
        record = __import__("json").loads(records[-1])
        self.assertEqual(record["hookEvent"], "SessionStart")
        self.assertEqual(record["outcome"], "succeeded")
        self.assertGreaterEqual(record["durationMs"], 0)
        self.assertNotIn("must-not-be-copied", records[-1])

    def test_request_input_language_mismatch_checks_gray_descriptions(self):
        english = {"tool_input":{"questions":[{"question":"Which technology should we use?","options":[{"description":"Use the maintained package with documented public APIs."}]}]}}
        taglish = {"tool_input":{"questions":[{"question":"Aling technology ang gagamitin natin?","options":[{"description":"Gamitin ang maintained package at documented public API."}]}]}}
        self.assertEqual(len(hook.communication_language_mismatches(english, "request_user_input")), 2)
        self.assertEqual(hook.communication_language_mismatches(taglish, "request_user_input"), [])

    def test_resume_reuses_session_and_turns_form_a_parent_chain(self):
        self.invoke("SessionStart", source="startup")
        first_session = self.row()["session_id"]
        self.invoke("SessionStart", source="resume")
        self.assertEqual(self.row()["session_id"], first_session)

        self.invoke("UserPromptSubmit", turn_id="turn-1", prompt="sensitive raw prompt")
        first = self.row()
        first_prompt = first["prompt_id"]
        prompt_begin = next(op for op in self.batches[-1] if op["kind"] == "prompt.begin")
        self.assertEqual(prompt_begin["data"]["rawPrompt"], "sensitive raw prompt")
        self.assertIsNone(prompt_begin["data"]["parentPromptId"])

        self.invoke("Stop", turn_id="turn-1")
        stopped = self.row()
        self.assertIsNone(stopped["prompt_id"])
        self.assertEqual(stopped["last_prompt_id"], first_prompt)
        self.assertTrue(any(op["kind"] == "attribution.record" for op in self.batches[-1]))

        self.invoke("UserPromptSubmit", turn_id="turn-2", prompt="next")
        prompt_begin = next(op for op in self.batches[-1] if op["kind"] == "prompt.begin")
        self.assertEqual(prompt_begin["data"]["parentPromptId"], first_prompt)

    def test_duplicate_prompt_hook_is_idempotent(self):
        self.invoke("SessionStart", source="startup")
        self.invoke("UserPromptSubmit", turn_id="same", prompt="one")
        ordinal = self.row()["prompt_ordinal"]
        self.invoke("UserPromptSubmit", turn_id="same", prompt="one")
        self.assertEqual(self.row()["prompt_ordinal"], ordinal)
        self.assertFalse(any(op["kind"] == "prompt.begin" for op in self.batches[-1]))

    def test_prompt_records_instruction_source_load_state(self):
        self.invoke("SessionStart", source="startup")
        self.invoke("UserPromptSubmit", turn_id="instruction-state", prompt="hello")
        event = next(op for op in self.batches[-1] if op["data"].get("code") == "agent.instructions.load_state")
        state = event["data"]["attributes"]
        self.assertTrue(state["loaded"])
        self.assertEqual(state["instance"], "personal")
        self.assertEqual(state["globalPrompt"]["path"], str(self.codex_home / "AGENTS.md"))
        self.assertEqual(len(state["globalPrompt"]["sha256"]), 64)
        self.assertNotIn("test global instructions", str(event))

    def test_behavior_audit_records_state_and_repeated_wait_with_raw_tool_record(self):
        self.invoke("SessionStart", source="startup")
        self.invoke("UserPromptSubmit", turn_id="turn-wait", prompt="private request")
        state = next(op for op in self.batches[-1] if op["data"].get("code") == "agent.behavior.audit_state")
        self.assertTrue(state["data"]["attributes"]["enabled"])

        self.invoke("PostToolUse", tool_name="functions.wait", tool_response="secret raw response")
        first = self.batches[-1]
        tool_event = next(op for op in first if op["data"].get("code") == "agent.tool.completed")
        wait_event = next(op for op in first if op["data"].get("code") == "agent.behavior.wait_observed")
        self.assertEqual(tool_event["data"]["rawPayload"]["toolResponse"], "secret raw response")
        self.assertNotIn("secret raw response", str(wait_event))

        self.invoke("PostToolUse", tool_name="functions.wait", tool_response="another private response")
        second = self.batches[-1]
        self.assertTrue(any(op["data"].get("code") == "agent.behavior.polling_threshold_exceeded" for op in second if op["kind"] == "event.emit"))
        self.assertTrue(any(op["kind"] == "error.record" for op in second))
        tool_event = next(op for op in second if op["data"].get("code") == "agent.tool.completed")
        self.assertEqual(tool_event["data"]["rawPayload"]["toolResponse"], "another private response")

    def test_behavior_audit_off_state_is_recorded_and_suppresses_wait_events(self):
        with patch.dict(os.environ, {"CODEX_OBS_BEHAVIOR_AUDIT":"off"}):
            self.invoke("SessionStart", source="startup")
            self.invoke("UserPromptSubmit", turn_id="turn-off", prompt="private request")
            state = next(op for op in self.batches[-1] if op["data"].get("code") == "agent.behavior.audit_state")
            self.assertFalse(state["data"]["attributes"]["enabled"])
            self.invoke("PostToolUse", tool_name="functions.wait", tool_response="private response")
        codes = [op["data"].get("code") for op in self.batches[-1] if op["kind"] == "event.emit"]
        self.assertEqual(codes, ["agent.tool.completed"])

    def test_background_transition_is_classified_without_storing_response(self):
        self.invoke("SessionStart", source="startup")
        self.invoke("UserPromptSubmit", turn_id="turn-background", prompt="private request")
        self.invoke("PostToolUse", tool_name="functions.exec", tool_response="Script running with cell ID 42; private payload")
        background = next(op for op in self.batches[-1] if op["data"].get("code") == "agent.behavior.background_started")
        tool_event = next(op for op in self.batches[-1] if op["data"].get("code") == "agent.tool.completed")
        self.assertNotIn("private payload", str(background))
        self.assertEqual(tool_event["data"]["rawPayload"]["toolResponse"], "Script running with cell ID 42; private payload")

    def test_stop_records_exact_assistant_result(self):
        self.invoke("SessionStart", source="startup")
        self.invoke("UserPromptSubmit", turn_id="turn-result", prompt="exact prompt")
        self.invoke("Stop", turn_id="turn-result", last_assistant_message="exact assistant result")
        finished = next(op for op in self.batches[-1] if op["kind"] == "prompt.finish")
        self.assertEqual(finished["data"]["rawResult"], "exact assistant result")


class ToolAnalyticsTests(unittest.TestCase):
    def test_tool_record_redacts_payload_and_preserves_explicit_signals(self):
        record = hook.tool_analytics({"tool_use_id":"tool-1","tool_response":{"is_error":True,"secret":"private"},
                                      "duration_ms":17}, "mcp__calendar__list", "2026-09-29T00:00:00Z")
        self.assertEqual((record["toolUseId"],record["origin"],record["durationMs"],record["failed"]),
                         ("tool-1","mcp",17,True))
        self.assertEqual(record["cacheStatus"], "unknown")
        self.assertNotIn("private", str(record))
        cached = hook.tool_analytics({"tool_response":{"cache_hit":True}}, "harness.cache", "2026-09-29T00:00:00Z")
        self.assertEqual((cached["cacheHit"],cached["cacheStatus"]), (True,"hit"))


if __name__ == "__main__":
    unittest.main()
