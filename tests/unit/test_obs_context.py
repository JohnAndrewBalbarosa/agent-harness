import json
import os
import subprocess
import sys
import tempfile
import threading
import unittest
import uuid
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

from core import display
from core.consumers import obs as obs_consumer
from core.context import consumer as context_consumer
from core.events import make_event

ROOT = Path(__file__).resolve().parents[2]
FAKES = Path(__file__).resolve().parent / "fakes"


class DisplayTests(unittest.TestCase):
    def test_titles(self):
        self.assertEqual(display.title("claude", "default"), "Claude Code")
        self.assertEqual(display.title("codex", "personal"), "Codex")
        self.assertEqual(display.title("codex", "cy"), "Codex (cy)")
        self.assertEqual(display.title("gemini", "default"), "gemini")


class ObsConsumerTests(unittest.TestCase):
    def test_codex_instance_keys_keep_legacy_agent_ids(self):
        self.assertEqual(obs_consumer.instance_key("codex", "personal"), "personal")
        self.assertEqual(obs_consumer.instance_key("codex", "cy"), "cy")
        legacy = str(uuid.uuid5(uuid.NAMESPACE_URL, "codex-observability:personal"))
        self.assertEqual(str(uuid.uuid5(uuid.NAMESPACE_URL, f"codex-observability:{obs_consumer.instance_key('codex', 'personal')}")), legacy)

    def test_other_agents(self):
        self.assertEqual(obs_consumer.instance_key("claude", "default"), "claude")
        self.assertEqual(obs_consumer.instance_key("claude", "work"), "claude-work")

    def test_consumer_forwards_raw_payload_with_identity_env(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "seen.json"
            env = {**os.environ, "PYTHONPATH": str(ROOT), "AH_AGENT": "codex", "AH_INSTANCE": "cy",
                   "AH_OBS_HOOK": str(FAKES / "env_dump.py"), "FAKE_OUT": str(out)}
            raw = json.dumps({"hook_event_name": "Stop", "session_id": "s", "note": "ñ"}).encode("utf-8")
            done = subprocess.run([sys.executable, "-m", "core.consumers.obs"], input=raw, cwd=str(ROOT), env=env, timeout=60)
            self.assertEqual(done.returncode, 0)
            seen = json.loads(out.read_text(encoding="utf-8"))
            self.assertEqual(json.loads(seen["stdin"])["note"], "ñ")
            self.assertEqual((seen["env"]["CODEX_OBS_INSTANCE"], seen["env"]["AH_AGENT_DISPLAY"]), ("cy", "Codex (cy)"))
            self.assertTrue(seen["env"]["CODEX_OBS_HOME"].endswith("policy"))


class FakeHub(BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps({"context": {"vision_summary": "Ship agent-harness", "constraints": ["no secrets"]}}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


class ContextConsumerTests(unittest.TestCase):
    def event(self, source="startup"):
        return make_event("session.start", "claude", "default", {"session_id": "s", "cwd": str(ROOT), "source": source}, "SessionStart")

    def test_unreachable_hub_yields_empty(self):
        self.assertEqual(context_consumer.handle(self.event(), {"OBS_ENDPOINT": "http://127.0.0.1:9"}), "")

    def test_context_text_from_hub(self):
        server = HTTPServer(("127.0.0.1", 0), FakeHub)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            text = context_consumer.handle(self.event(), {"OBS_ENDPOINT": f"http://127.0.0.1:{server.server_address[1]}"})
        finally:
            server.shutdown()
            server.server_close()
        self.assertIn("Vision: Ship agent-harness", text)
        self.assertIn("Constraints: no secrets", text)

    def test_non_session_events_yield_empty(self):
        stop = make_event("turn.stop", "claude", "default", {"session_id": "s"}, "Stop")
        self.assertEqual(context_consumer.handle(stop, {}), "")

    def test_compact_context_is_shorter(self):
        self.assertEqual(context_consumer.limit_for("compact"), 3000)
        self.assertEqual(context_consumer.limit_for("startup"), 6000)


if __name__ == "__main__":
    unittest.main()
