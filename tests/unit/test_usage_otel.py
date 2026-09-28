import unittest

from adapters.claude import otel as claude_otel
from adapters.codex import otel as codex_otel
from core.usage import receiver as rx


def kv(key, value):
    if isinstance(value, bool):
        return {"key": key, "value": {"boolValue": value}}
    if isinstance(value, int):
        return {"key": key, "value": {"intValue": str(value)}}
    if isinstance(value, float):
        return {"key": key, "value": {"doubleValue": value}}
    return {"key": key, "value": {"stringValue": value}}


def payload(service, *records):
    return {"resourceLogs": [{"resource": {"attributes": [kv("service.name", service)]},
                              "scopeLogs": [{"logRecords": [{"attributes": [kv(k, v) for k, v in r.items()]} for r in records]}]}]}


CLAUDE_API = {"event.name": "api_request", "session.id": "sess-1", "prompt.id": "prompt-1", "model": "claude-opus-5-5",
              "input_tokens": 10, "output_tokens": 193, "cache_read_tokens": 21480, "cache_creation_tokens": 36624,
              "request_id": "req_1", "user.email": "someone@example.com", "user.id": "u-1", "organization.id": "org-1"}
CODEX_DONE = {"event.name": "codex.sse_event", "event.kind": "response.completed", "input_token_count": "12581",
              "output_token_count": "40", "cached_token_count": 12000, "cache_write_token_count": 0, "reasoning_token_count": 7,
              "conversation.id": "conv-1", "event.timestamp": "2026-09-27T20:30:50.847Z", "model": "gpt-6-astra",
              "user.email": "someone@example.com", "user.account_id": "acct-1"}


class ClaudeOtelTests(unittest.TestCase):
    def test_maps_api_request(self):
        data = claude_otel.map(rx.decode_attrs(payload("claude-code", CLAUDE_API))[0][1])
        self.assertEqual((data["nativeSession"], data["nativeTurn"], data["source"]), ("sess-1", "prompt-1", "claude_otel"))
        self.assertEqual((data["inputTokens"], data["outputTokens"], data["cachedInputTokens"]), (10 + 21480 + 36624, 193, 21480))
        self.assertIsNone(data["reasoningTokens"])

    def test_other_events_ignored(self):
        self.assertIsNone(claude_otel.map({"event.name": "user_prompt"}))
        self.assertIsNone(claude_otel.map({"event.name": "api_request", "session.id": "s"}))

    def test_usage_id_deterministic(self):
        attrs = rx.decode_attrs(payload("claude-code", CLAUDE_API))[0][1]
        self.assertEqual(claude_otel.map(attrs)["usageId"], claude_otel.map(dict(attrs))["usageId"])


class CodexOtelTests(unittest.TestCase):
    def test_maps_response_completed_with_mixed_types(self):
        data = codex_otel.map(rx.decode_attrs(payload("codex_exec", CODEX_DONE))[0][1])
        self.assertEqual((data["nativeSession"], data["source"], data["nativeTurn"]), ("conv-1", "codex_otel", None))
        self.assertEqual((data["inputTokens"], data["outputTokens"], data["cachedInputTokens"], data["reasoningTokens"]),
                         (12581, 40, 12000, 7))

    def test_other_codex_events_ignored(self):
        self.assertIsNone(codex_otel.map({"event.name": "codex.sse_event", "event.kind": "response.created"}))
        self.assertIsNone(codex_otel.map({"event.name": "codex.api_request"}))

    def test_distinct_responses_get_distinct_ids(self):
        first = codex_otel.map(dict(CODEX_DONE))["usageId"]
        second = codex_otel.map({**CODEX_DONE, "event.timestamp": "2026-09-27T20:30:51.000Z"})["usageId"]
        self.assertNotEqual(first, second)


class ReceiverTests(unittest.TestCase):
    def test_dispatch_by_service_name_prefix(self):
        self.assertEqual(rx.adapter_for("codex_exec"), "codex")
        self.assertEqual(rx.adapter_for("codex_cli_rs"), "codex")
        self.assertEqual(rx.adapter_for("claude-code"), "claude")
        self.assertIsNone(rx.adapter_for("something-else"))

    def test_operations_carry_project_and_no_personal_data(self):
        body = payload("claude-code", CLAUDE_API, {"event.name": "user_prompt"})
        ops = rx.usage_records(body, lambda session: "project-9")
        self.assertEqual(len(ops), 1)
        self.assertEqual(ops[0]["projectId"], "project-9")
        text = repr(ops)
        for secret in ("someone@example.com", "u-1", "org-1", "acct-1"):
            self.assertNotIn(secret, text)

    def test_unknown_service_and_malformed_payloads_yield_nothing(self):
        self.assertEqual(rx.usage_records(payload("other", CLAUDE_API), lambda s: "p"), [])
        self.assertEqual(rx.usage_records({"resourceLogs": "nope"}, lambda s: "p"), [])
        self.assertEqual(rx.usage_records({}, lambda s: "p"), [])

    def test_project_falls_back_to_global(self):
        ops = rx.usage_records(payload("codex_exec", CODEX_DONE), lambda session: None)
        self.assertEqual(ops[0]["projectId"], rx.GLOBAL_PROJECT_ID)


if __name__ == "__main__":
    unittest.main()
