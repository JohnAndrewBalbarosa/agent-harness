import json
import threading
import unittest
from http.server import HTTPServer
from unittest.mock import patch
from urllib.request import Request, urlopen

from core.usage import receiver as rx
from tests.unit.test_usage_otel import CLAUDE_API, payload


class ReceiverHttpTests(unittest.TestCase):
    def setUp(self):
        self.server = HTTPServer(("127.0.0.1", 0), rx.Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()

    def post(self, path, body: bytes):
        with urlopen(Request(self.url + path, data=body, headers={"Content-Type": "application/json"}), timeout=5) as r:
            return r.status

    def test_health(self):
        with urlopen(self.url + "/health", timeout=5) as r:
            self.assertEqual(json.loads(r.read()), {"status": "ok"})

    def test_logs_are_enqueued_without_personal_data(self):
        with patch.object(rx, "enqueue_usage") as enqueue, patch.object(rx, "spool_project", return_value=None):
            self.assertEqual(self.post("/v1/logs", json.dumps(payload("claude-code", CLAUDE_API)).encode()), 200)
        (records,), _ = enqueue.call_args
        self.assertEqual(len(records), 1)
        self.assertNotIn("someone@example.com", repr(records))

    def test_garbage_body_still_returns_200(self):
        with patch.object(rx, "enqueue_usage") as enqueue:
            self.assertEqual(self.post("/v1/logs", b"not json"), 200)
        enqueue.assert_not_called()


class PortTests(unittest.TestCase):
    def test_port_comes_from_the_harness_env_file(self):
        import tempfile
        from pathlib import Path
        with tempfile.TemporaryDirectory() as tmp:
            env_file = Path(tmp) / ".env"
            env_file.write_text("OTLP_PORT=4399\n", encoding="utf-8")
            with patch.object(rx, "ENV_FILE", env_file), patch.dict("os.environ", {}, clear=False) as environ:
                environ.pop("OTLP_PORT", None)
                self.assertEqual(rx.port(), 4399)


class SingleInstanceTests(unittest.TestCase):
    def test_second_server_cannot_bind_the_same_port(self):
        first = rx.make_server(0)
        try:
            with self.assertRaises(OSError):
                rx.make_server(first.server_address[1]).server_close()
        finally:
            first.server_close()


if __name__ == "__main__":
    unittest.main()
