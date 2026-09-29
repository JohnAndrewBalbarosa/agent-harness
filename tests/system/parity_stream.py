"""Deterministic Phase A input: operation batches covering all 28 kinds of server.mjs:141-377,
replayed batches, failing batches, acceptance edge probes, and GET probes for all 12 routes.

Ids are uuid5 values of stable names (version 5, RFC variant — valid for zod `.uuid()`), hex
values are SHA-256 prefixes of stable names, and timestamps are offsets from a recent base
time (analytics and evaluation-data windows are relative to `now()`).
"""

from __future__ import annotations

import hashlib
import itertools
import json
import uuid
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from types import SimpleNamespace
from urllib.parse import quote

from parity_diff import MODE_AUTO, MODE_DB_ERROR, MODE_EXACT, MODE_FRAMEWORK_ERROR

ID_NAMESPACE = uuid.UUID("5b0e5d8e-3f55-4c1a-9d7e-2b1f0c6a9e11")
BASE_AGE = timedelta(hours=3)
OLD_OFFSET_SECONDS = -3 * 24 * 60 * 60
JSON_CONTENT_TYPE = "application/json"
OPERATIONS_PATH = "/v1/operations"
MAX_BATCH_OPERATIONS = 500
FASTIFY_BODY_LIMIT_BYTES = 1024 * 1024
REPLAYED_BATCH_INDEXES = (1, 2)


@dataclass(frozen=True)
class Probe:
    label: str
    method: str
    path: str
    mode: str = MODE_AUTO
    body: bytes | None = None
    content_type: str | None = None


def ident(name: str) -> str:
    return str(uuid.uuid5(ID_NAMESPACE, name))


def hex_of(name: str, size_bytes: int) -> str:
    return hashlib.sha256(name.encode("utf-8")).hexdigest()[: size_bytes * 2]


def json_bytes(value: object) -> bytes:
    """ASCII JSON so lone surrogates travel as `\\udXXX` escapes exactly like a JS client sends them."""
    return json.dumps(value, ensure_ascii=True, separators=(",", ":")).encode("ascii")


def operations_body(operations: list[dict]) -> bytes:
    return json_bytes({"operations": operations})


def iso_millis(instant: datetime) -> str:
    return f"{instant:%Y-%m-%dT%H:%M:%S}.{instant.microsecond // 1000:03d}Z"


class OperationClock:
    """Deterministic operation ids and `occurredAt` values relative to one base instant."""

    def __init__(self, base: datetime) -> None:
        self.base = base
        self.counter = itertools.count(1)

    @staticmethod
    def recent() -> "OperationClock":
        now = datetime.now(timezone.utc).replace(microsecond=0)
        return OperationClock(now - BASE_AGE)

    def at(self, seconds: float) -> str:
        return iso_millis(self.base + timedelta(seconds=seconds))

    def at_micros(self, seconds: int, micros: str) -> str:
        whole = self.base + timedelta(seconds=seconds)
        return f"{whole:%Y-%m-%dT%H:%M:%S}.{micros}Z"

    def operation(self, kind: str, seconds: float, data: dict, occurred_at: str | None = None) -> dict:
        number = next(self.counter)
        return {
            "operationId": ident(f"operation:{number}"),
            "kind": kind,
            "occurredAt": occurred_at or self.at(seconds),
            "data": data,
        }


def synthetic_ids() -> SimpleNamespace:
    names = [
        "project_alpha", "project_beta", "project_unknown", "agent_codex", "agent_claude",
        "session_alpha", "session_beta", "session_unknown", "prompt_root", "prompt_child",
        "prompt_beta", "prompt_unknown", "code_alpha", "code_beta", "exec_root", "exec_child",
        "exec_beta", "exec_surrogate", "event_start", "event_failed", "event_child_failed",
        "event_child_retry", "event_beta_crash", "event_surrogate", "attribution", "context_v1",
        "context_v2", "context_beta", "context_rolled_back", "binding_first", "binding_second",
        "state_event", "herdr_event", "usage_hook", "usage_otel", "usage_before_link",
        "usage_unavailable", "usage_no_turn", "usage_old", "tool_read_hit", "tool_read_miss",
        "tool_bash", "tool_mcp", "tool_zeta", "tool_duplicate", "tool_old", "tool_coerced",
        "handoff_progress", "handoff_decision", "handoff_stale", "handoff_architecture",
        "handoff_constraint", "pane_evidence", "pane_evidence_duplicate", "assessment_code",
        "assessment_failed", "assessment_docs", "package_main", "program_delivery",
        "program_latency", "program_unknown", "contract_v1", "contract_v2", "contract_latency",
        "reference_dora", "reference_slo", "observation_manual", "observation_pending",
        "observation_cycle", "run_completed", "run_partial", "error_group", "error_group_alias",
        "error_group_beta", "error_group_unknown",
    ]
    ids = SimpleNamespace(**{name: ident(name) for name in names})
    ids.trace_root = hex_of("trace:root", 16)
    ids.trace_child = hex_of("trace:child", 16)
    ids.trace_beta = hex_of("trace:beta", 16)
    ids.trace_surrogate = hex_of("trace:surrogate", 16)
    ids.trace_unknown = hex_of("trace:unknown", 16)
    return ids


# --------------------------------------------------------------------------- stream batches

def build_operation_batches(clock: OperationClock, ids: SimpleNamespace) -> list[list[dict]]:
    return [
        identity_batch(clock, ids),
        execution_batch(clock, ids),
        runtime_batch(clock, ids),
        delivery_batch(clock, ids),
    ]


def identity_batch(clock: OperationClock, ids: SimpleNamespace) -> list[dict]:
    op = clock.operation
    return [
        op("project.register", 0, {
            "projectId": ids.project_alpha, "displayName": "Parity Alpha", "slug": "parity-alpha",
            "aliases": [{"kind": 1, "hash": hex_of("alias:alpha:path", 32)},
                        {"kind": 2, "hash": hex_of("alias:alpha:remote", 32)}]}),
        op("project.register", 1, {
            "projectId": ids.project_beta, "displayName": "Parity Beta ñ", "slug": "parity-beta.v2"}),
        op("agent.register", 2, {
            "agentInstanceId": ids.agent_codex, "instanceKey": "codex-personal", "sourceKind": 1,
            "displayName": "Codex Personal"}),
        op("agent.register", 2, {
            "agentInstanceId": ids.agent_claude, "instanceKey": "claude-cy", "sourceKind": 2,
            "displayName": "Claude CY"}),
        op("session.start", 3, {
            "sessionId": ids.session_alpha, "projectId": ids.project_alpha, "agentInstanceId": ids.agent_codex,
            "sessionKind": 2, "sessionFingerprint": hex_of("session:alpha", 32)}),
        op("session.start", 4, {
            "sessionId": ids.session_beta, "projectId": ids.project_beta, "agentInstanceId": ids.agent_claude,
            "sessionFingerprint": hex_of("session:beta", 32)}),
        op("prompt.begin", 5.57, {
            "promptId": ids.prompt_root, "sessionId": ids.session_alpha, "ordinal": 1,
            "promptHmac": hex_of("hmac:root", 32),
            "rawPrompt": "Ayusin mo yung parity test 🚀 — pakicheck ang \"quotes\"\nat newline"}),
        op("prompt.begin", 6, {
            "promptId": ids.prompt_child, "sessionId": ids.session_alpha, "parentPromptId": ids.prompt_root,
            "ordinal": 2, "promptHmac": hex_of("hmac:child", 32), "rawPrompt": "second prompt",
            "languageCode": "en"}),
        op("prompt.begin", 7.29, {
            "promptId": ids.prompt_beta, "sessionId": ids.session_beta, "ordinal": 1,
            "promptHmac": hex_of("hmac:beta", 32), "rawPrompt": ""}),
        op("prompt.enrich", 8, {
            "promptId": ids.prompt_root, "summary": "Parity test", "intent": "verify rewrite",
            "targetSystem": "obs-hub", "constraints": ["no network", 3],
            "successCriteria": [{"check": "parity"}], "resultSummary": ""}),
        op("code-version.register", 8.5, {
            "codeVersionId": ids.code_alpha, "projectId": ids.project_alpha, "commitSha": hex_of("commit:a", 20),
            "branchName": "main", "dirtyFingerprint": hex_of("dirty:a", 32), "buildVersion": "0.1.0"}),
        op("code-version.register", 8.6, {
            "codeVersionId": ids.code_beta, "projectId": ids.project_beta, "commitSha": hex_of("commit:b", 20),
            "branchName": "feat/x"}),
        op("agent.register", 9, {
            "agentInstanceId": ids.agent_codex, "instanceKey": "codex-personal", "sourceKind": 1,
            "displayName": "Codex Personal (renamed)"}),
    ]


def execution_batch(clock: OperationClock, ids: SimpleNamespace) -> list[dict]:
    op = clock.operation
    surrogate_split_summary = "a" * 499 + "😀" + "tail"  # text(…,500) splits the surrogate pair
    return [
        op("execution.start", 10, {
            "executionId": ids.exec_root, "projectId": ids.project_alpha, "sessionId": ids.session_alpha,
            "promptId": ids.prompt_root, "executionKind": 2, "traceId": ids.trace_root,
            "codeVersionId": ids.code_alpha}),
        op("execution.start", 11, {
            "executionId": ids.exec_child, "projectId": ids.project_alpha, "sessionId": ids.session_alpha,
            "promptId": ids.prompt_child, "parentExecutionId": ids.exec_root, "attemptNo": 2,
            "traceId": ids.trace_child}),
        op("execution.start", 12, {
            "executionId": ids.exec_beta, "projectId": ids.project_beta, "sessionId": ids.session_beta,
            "promptId": ids.prompt_beta, "traceId": ids.trace_beta}),
        op("execution.code-version", 12.5, {"executionId": ids.exec_child, "codeVersionId": ids.code_alpha}),
        op("event.emit", 13, {
            "eventId": ids.event_start, "projectId": ids.project_alpha, "executionId": ids.exec_root,
            "code": "hub.request.started", "category": 2, "severity": 1, "component": "router",
            "componentKind": 2, "sequenceNo": 1, "traceId": ids.trace_root, "spanId": hex_of("span:1", 8),
            "messageSummary": "started", "attributes": {"route": "/v1/x", "count": 3, "nested": {"ok": True}},
            "rawPayload": {"k": [1, 2]}}),
        op("event.emit", 14.57, {
            "eventId": ids.event_failed, "projectId": ids.project_alpha, "executionId": ids.exec_root,
            "code": "hub.request.failed", "category": 3, "severity": 4, "outcome": 2, "component": "router",
            "sequenceNo": 2, "traceId": ids.trace_root, "spanId": hex_of("span:2", 8),
            "parentSpanId": hex_of("span:1", 8), "messageSummary": surrogate_split_summary,
            "rawPayload": "plain string payload"}),
        op("event.emit", 15, {
            "eventId": ids.event_child_failed, "projectId": ids.project_alpha, "executionId": ids.exec_child,
            "code": "hub.request.failed", "severity": 3, "sequenceNo": 1, "traceId": ids.trace_child,
            "attributes": {"b": 1, "a": 2}}),
        op("event.emit", 15, {
            "eventId": ids.event_child_retry, "projectId": ids.project_alpha, "executionId": ids.exec_child,
            "code": "worker.retry", "sequenceNo": 2, "traceId": ids.trace_child, "attributes": [1, 2]}),
        op("event.emit", 16, {
            "eventId": ids.event_beta_crash, "projectId": ids.project_beta, "executionId": ids.exec_beta,
            "code": "beta.crash", "category": 4, "severity": 5, "outcome": 3, "component": "worker",
            "traceId": ids.trace_beta, "messageSummary": "crash"}),
        op("execution.finish", 0, {
            "executionId": ids.exec_root, "status": 2, "outcome": 1, "resultSummary": "done"},
           occurred_at=clock.at_micros(75, "123789")),  # µs end: Postgres ::bigint rounds duration
        op("execution.finish", 80.29, {
            "executionId": ids.exec_child, "status": 3, "outcome": 2, "resultSummary": "failed"}),
        op("execution.finish", 90, {"executionId": ids.exec_beta, "outcome": 1}),
        op("attribution.record", 91, {
            "attributionId": ids.attribution, "executionId": ids.exec_root, "codeVersionId": ids.code_alpha,
            "artifactKind": 2, "locatorHash": hex_of("locator:1", 32), "locatorLabel": "src/lib.rs",
            "contentHash": hex_of("content:1", 32), "metadata": {"lines": 12}}),
        op("context.record", 20, {
            "contextId": ids.context_v1, "projectId": ids.project_alpha, "visionSummary": "Vision",
            "architectureSummary": "Rust + SQLite", "currentGoalSummary": "Parity",
            "constraints": ["no docker at runtime"], "sourcePromptId": ids.prompt_root}),
        op("context.record", 21, {
            "contextId": ids.context_v2, "projectId": ids.project_alpha, "languageCode": "en",
            "visionSummary": "Vision v2", "constraints": "not-an-array"}),
        op("context.record", 22, {"contextId": ids.context_beta, "projectId": ids.project_beta}),
    ]


def runtime_batch(clock: OperationClock, ids: SimpleNamespace) -> list[dict]:
    return herdr_operations(clock, ids) + usage_operations(clock, ids) + handoff_operations(clock, ids)


def herdr_operations(clock: OperationClock, ids: SimpleNamespace) -> list[dict]:
    op = clock.operation
    binding = {
        "projectId": ids.project_alpha, "nativeSession": "ns-alpha", "profileName": "personal",
        "workspaceId": "w1", "tabId": "t1", "runtimeScope": "herdr", "cwd": "C:\\Users\\dev\\repo"}
    return [
        op("herdr.binding", 30, {**binding, "bindingId": ids.binding_first, "paneId": "p1"}),
        op("herdr.binding", 31, {**binding, "bindingId": ids.binding_second, "paneId": "p2"}),
        op("agent-state.record", 32, {
            "stateEventId": ids.state_event, "projectId": ids.project_alpha, "nativeSession": "ns-alpha",
            "nativeTurn": "turn-1", "profileName": "personal", "previousState": "idle",
            "canonicalState": "working", "projectedState": "working", "reason": "prompt submitted",
            "authority": "hook", "signalSource": "UserPromptSubmit", "sequenceNo": 1, "conflict": False,
            "attributes": {"x": 1}}),
        op("herdr-event.record", 33, {
            "herdrEventId": ids.herdr_event, "projectId": ids.project_alpha, "nativeSession": "ns-alpha",
            "paneId": "p2", "eventName": "pane.output", "sequenceNo": 5, "payload": {"bytes": 10}}),
        op("pane-evidence.record", 57, {
            "snapshotId": ids.pane_evidence, "projectId": ids.project_alpha, "nativeSession": "ns-alpha",
            "paneId": "p2", "triggerCode": "stop", "contentSha256": hex_of("pane:1", 32), "lineCount": 120,
            "byteCount": 40000, "redactedContent": "line\n[REDACTED]", "explain": {"why": "stop"}}),
        op("pane-evidence.record", 58, {
            "snapshotId": ids.pane_evidence_duplicate, "projectId": ids.project_alpha,
            "nativeSession": "ns-alpha", "triggerCode": "stop", "contentSha256": hex_of("pane:1", 32),
            "lineCount": 1, "byteCount": 1, "redactedContent": "dup"}),
    ]


# Mental model:
# link turn-1 before usage (usage picks prompt) → usage for turn-2 before its link (link backfills)
# → conflicting relink is a no-op → old rows fall outside `days=1`
def usage_operations(clock: OperationClock, ids: SimpleNamespace) -> list[dict]:
    op = clock.operation
    alpha = {"projectId": ids.project_alpha, "nativeSession": "ns-alpha"}
    return [
        op("prompt.native-link", 34, {**alpha, "nativeTurn": "turn-1", "promptId": ids.prompt_root}),
        op("turn-usage.record", 35, {
            **alpha, "usageId": ids.usage_hook, "nativeTurn": "turn-1", "inputTokens": 1000,
            "outputTokens": 200, "cachedInputTokens": 300, "reasoningTokens": 50, "availability": "exact",
            "source": "codex_hook", "sourceEventId": "evt-1"}),
        op("turn-usage.record", 36, {
            **alpha, "usageId": ids.usage_otel, "nativeTurn": "turn-1", "inputTokens": 1100,
            "outputTokens": 210, "availability": "exact", "source": "codex_otel", "sourceEventId": "otel-1"}),
        op("turn-usage.record", 37, {
            **alpha, "usageId": ids.usage_before_link, "nativeTurn": "turn-2", "inputTokens": 500,
            "outputTokens": 40, "cachedInputTokens": 0, "reasoningTokens": 0, "availability": "exact",
            "source": "codex_hook", "sourceEventId": "evt-2"}),
        op("prompt.native-link", 38, {**alpha, "nativeTurn": "turn-2", "promptId": ids.prompt_child}),
        op("prompt.native-link", 38.5, {**alpha, "nativeTurn": "turn-1", "promptId": ids.prompt_child}),
        op("turn-usage.record", 39, {
            **alpha, "usageId": ids.usage_unavailable, "nativeTurn": "turn-3", "availability": "unavailable",
            "source": "codex_hook"}),
        op("turn-usage.record", 40, {
            **alpha, "usageId": ids.usage_no_turn, "inputTokens": 7, "availability": "exact",
            "source": "claude_hook"}),
        op("prompt.native-link", 40.5, {
            "projectId": ids.project_beta, "nativeSession": "ns-beta", "nativeTurn": "turn-9",
            "promptId": ids.prompt_beta}),
        op("turn-usage.record", OLD_OFFSET_SECONDS, {
            "projectId": ids.project_beta, "nativeSession": "ns-beta", "usageId": ids.usage_old,
            "nativeTurn": "turn-9", "inputTokens": 9, "outputTokens": 1, "availability": "exact",
            "source": "claude_hook", "sourceEventId": "old-1"}),
    ] + tool_use_operations(clock, ids)


def tool_use_operations(clock: OperationClock, ids: SimpleNamespace) -> list[dict]:
    op = clock.operation
    alpha = {"projectId": ids.project_alpha, "nativeSession": "ns-alpha"}
    return [
        op("tool-use.record", 41, {
            **alpha, "toolRecordId": ids.tool_read_hit, "promptId": ids.prompt_root, "executionId": ids.exec_root,
            "nativeTurn": "turn-1", "toolUseId": "tu-1", "toolName": "Read", "origin": "built-in",
            "startedAt": clock.at(40.9), "completedAt": clock.at(41), "durationMs": 10, "failed": False,
            "cacheHit": True, "cacheStatus": "hit"}),
        op("tool-use.record", 42, {
            **alpha, "toolRecordId": ids.tool_read_miss, "toolUseId": "tu-2", "toolName": "Read",
            "origin": "built-in", "durationMs": 15, "failed": True, "cacheStatus": "miss"}),
        op("tool-use.record", 43, {
            **alpha, "toolRecordId": ids.tool_bash, "toolUseId": "tu-3", "toolName": "bash", "origin": "harness",
            "parentToolUseId": "tu-1"}),
        op("tool-use.record", 44, {
            **alpha, "toolRecordId": ids.tool_mcp, "toolUseId": "tu-4", "toolName": "mcp__github__search",
            "origin": "mcp", "durationMs": 7}),
        op("tool-use.record", 45, {**alpha, "toolRecordId": ids.tool_zeta, "toolUseId": "tu-5", "toolName": "Zeta"}),
        op("tool-use.record", 46, {
            **alpha, "toolRecordId": ids.tool_duplicate, "toolUseId": "tu-1", "toolName": "Read",
            "origin": "built-in", "durationMs": 999}),
        op("tool-use.record", OLD_OFFSET_SECONDS, {
            **alpha, "toolRecordId": ids.tool_old, "toolUseId": "tu-old", "toolName": "Read",
            "origin": "built-in", "durationMs": 1000}),
    ]


# Mental model:
# v1 progress, v2 decision (fresh base), v3 stale base → conflict, v4 architecture → needs
# promotion, v5 constraint on stale base → conflict + promotion; promote v4 (works) and v5 (no-op)
def handoff_operations(clock: OperationClock, ids: SimpleNamespace) -> list[dict]:
    op = clock.operation
    alpha = {"projectId": ids.project_alpha, "profileName": "personal", "nativeSession": "ns-alpha"}
    return [
        op("handoff.record", 50, {**alpha, "handoffId": ids.handoff_progress, "handoffKind": "progress",
                                  "content": "step 1 done"}),
        op("handoff.record", 51, {**alpha, "handoffId": ids.handoff_decision, "handoffKind": "decision",
                                  "baseVersion": 1, "content": "use SQLite"}),
        op("handoff.record", 52, {**alpha, "handoffId": ids.handoff_stale, "handoffKind": "progress",
                                  "baseVersion": 0, "content": "stale"}),
        op("handoff.record", 53, {**alpha, "handoffId": ids.handoff_architecture, "handoffKind": "architecture",
                                  "baseVersion": 3, "content": "axum + rusqlite"}),
        op("handoff.record", 54, {**alpha, "handoffId": ids.handoff_constraint, "handoffKind": "constraint",
                                  "baseVersion": 1, "content": "no unsafe"}),
        op("handoff.promote", 55, {"projectId": ids.project_alpha, "handoffId": ids.handoff_architecture}),
        op("handoff.promote", 56, {"projectId": ids.project_alpha, "handoffId": ids.handoff_constraint}),
    ]


def delivery_batch(clock: OperationClock, ids: SimpleNamespace) -> list[dict]:
    return (change_assessment_operations(clock, ids) + benchmark_operations(clock, ids)
            + error_operations(clock, ids) + closing_operations(clock, ids))


def change_assessment_operations(clock: OperationClock, ids: SimpleNamespace) -> list[dict]:
    op = clock.operation
    plan = {"projectId": ids.project_alpha, "deliveryStream": "main", "changeKind": "code",
            "riskFlags": [], "sourceUrls": ["https://example.com/a"]}
    return [
        op("change-assessment.plan", 60, {
            **plan, "assessmentId": ids.assessment_code, "promptId": ids.prompt_root, "declaredClass": "routine",
            "effectiveClass": "routine", "classificationReason": "one file", "estimatedLines": 10,
            "estimatedFiles": 1}),
        op("change-assessment.plan", 61, {
            **plan, "assessmentId": ids.assessment_failed, "promptId": ids.prompt_child,
            "declaredClass": "moderate", "effectiveClass": "moderate", "riskFlags": ["concurrency"]}),
        op("change-assessment.plan", 62, {
            "assessmentId": ids.assessment_docs, "projectId": ids.project_beta, "promptId": ids.prompt_beta,
            "deliveryStream": "main", "changeKind": "docs", "declaredClass": "non-code",
            "effectiveClass": "non-code"}),
        op("change-assessment.plan", 63, {
            **plan, "assessmentId": ids.assessment_code, "promptId": ids.prompt_root, "declaredClass": "routine",
            "effectiveClass": "moderate", "estimatedLines": 20, "estimatedFiles": 2}),
        op("change-assessment.finish", 64.57, {
            "assessmentId": ids.assessment_code, "effectiveClass": "moderate", "actualLinesAdded": 20,
            "actualLinesDeleted": 3, "actualFiles": 2, "sourceUrls": ["https://example.com/a"],
            "usage": {"input": 100, "output": 20, "cachedInput": 5, "reasoning": 1}, "status": "success",
            "incrementsCounter": True}),
        op("change-assessment.finish", 65, {
            "assessmentId": ids.assessment_failed, "effectiveClass": "moderate", "status": "failed",
            "incrementsCounter": True, "completedAt": clock.at(65.5)}),
        op("change-assessment.finish", 66, {
            "assessmentId": ids.assessment_docs, "effectiveClass": "non-code", "status": "stopped"}),
        op("change-assessment.package", 67, {
            "packageId": ids.package_main, "projectId": ids.project_alpha, "deliveryStream": "main",
            "deliveryMode": "local", "commitSha": hex_of("commit:pkg", 20), "promptCount": 2}),
    ]


def benchmark_operations(clock: OperationClock, ids: SimpleNamespace) -> list[dict]:
    op = clock.operation
    return [
        op("benchmark.init", 70, delivery_program(clock, ids, "v1", ids.contract_v1, 1)),
        op("benchmark.init", 71, latency_program(clock, ids)),
        op("benchmark.init", 72, delivery_program(clock, ids, "v2", ids.contract_v2, 2)),
        op("benchmark.observe", 73, {
            "observationId": ids.observation_manual, "programId": ids.program_delivery,
            "metricKey": "manual_score", "value": 0.875, "provenanceKind": "manual",
            "provenanceLocator": "review.md"}),
        op("benchmark.observe", 74, {
            "observationId": ids.observation_pending, "programId": ids.program_delivery,
            "metricKey": "manual_score", "value": 3, "observedAt": clock.at(74.29), "provenanceKind": "import",
            "comparable": False, "validationStatus": "pending", "attributes": {"batch": "b1"}}),
        op("benchmark.observe", 75, {
            "observationId": ids.observation_cycle, "programId": ids.program_delivery, "metricKey": "cycle_time",
            "executionId": ids.exec_root, "promptId": ids.prompt_root, "value": 1234.5,
            "provenanceKind": "hook", "evidenceSha256": hex_of("evidence:1", 32),
            "environmentFingerprint": hex_of("env:1", 32)}),
        op("benchmark.run", 76, {
            "runId": ids.run_completed, "programId": ids.program_delivery, "contractVersionId": ids.contract_v2,
            "windowStart": clock.at(-86400), "windowEnd": clock.at(76), "status": "completed",
            "scipyVersion": "1.14.1", "environmentFingerprint": hex_of("env:1", 32),
            "results": benchmark_results()}),
        op("benchmark.run", 77, {
            "runId": ids.run_partial, "programId": ids.program_delivery, "contractVersionId": ids.contract_v2,
            "windowEnd": clock.at(77), "status": "partial"}),
    ]


def delivery_program(clock: OperationClock, ids: SimpleNamespace, version: str, contract: str,
                     version_no: int) -> dict:
    snapshot_toml = f"[program]\nname = \"Delivery health\"\nversion = \"{version}\"\n"
    return {
        "programId": ids.program_delivery, "projectId": ids.project_alpha, "name": "Delivery health",
        "measuredSubject": "project", "approvalPromptId": ids.prompt_root, "approvedAt": clock.at(69),
        "contractVersionId": contract, "versionNo": version_no, "sourcePath": "benchmarks/delivery.toml",
        "snapshotToml": snapshot_toml, "snapshotSha256": hashlib.sha256(snapshot_toml.encode()).hexdigest(),
        "references": benchmark_references(clock, ids), "metrics": delivery_metrics(ids, version),
    }


def latency_program(clock: OperationClock, ids: SimpleNamespace) -> dict:
    snapshot_toml = "[program]\nname = \"API latency\"\n"
    return {
        "programId": ids.program_latency, "projectId": ids.project_alpha, "name": "API latency",
        "measuredSubject": "service", "approvalPromptId": ids.prompt_child, "contractVersionId": ids.contract_latency,
        "versionNo": 1, "sourcePath": "benchmarks/latency.toml", "snapshotToml": snapshot_toml,
        "snapshotSha256": hashlib.sha256(snapshot_toml.encode()).hexdigest(), "approvedBy": "reviewer",
        "metrics": [{
            "metricId": ident("metric:p95:v1"), "key": "p95_latency", "title": "p95 latency", "unit": "ms",
            "direction": "lower", "evaluationMethod": "conformance", "lowerBound": 0, "upperBound": 250}],
    }


def benchmark_references(clock: OperationClock, ids: SimpleNamespace) -> list[dict]:
    return [
        {"referenceId": ids.reference_dora, "key": "dora-2024", "type": "recognized_benchmark", "issuer": "DORA",
         "title": "Accelerate State of DevOps 2024", "version": "2024",
         "sourceUrl": "https://dora.dev/research/2024/dora-report/", "publishedAt": "2024-10-22T00:00:00.000Z",
         "retrievedAt": clock.at(68), "contentSha256": hex_of("ref:dora", 32),
         "reviewDueAt": "2027-10-22T00:00:00.000Z"},
        {"referenceId": ids.reference_slo, "key": "obs-completeness-slo", "type": "internal_slo",
         "issuer": "harness", "title": "Observability completeness SLO", "version": "1",
         "sourceUrl": "project:observability", "retrievedAt": clock.at(68), "contentSha256": hex_of("ref:slo", 32)},
    ]


def delivery_metrics(ids: SimpleNamespace, version: str) -> list[dict]:
    """One metric per `coreExtractor` branch (server.mjs:461-473) plus one external, optional metric."""
    return [
        {"metricId": ident(f"metric:cycle_time:{version}"), "referenceId": ids.reference_dora, "key": "cycle_time",
         "title": "Successful cycle time", "unit": "ms", "aggregation": "median", "direction": "lower",
         "evaluationMethod": "one_sample_t", "hypothesisMode": "non_inferiority",
         "nullHypothesis": "median >= target", "alternativeHypothesis": "median < target", "targetValue": 60000,
         "margin": 5000, "alpha": 0.05, "confidenceLevel": 0.95, "minimumSamples": 3, "windowDays": 30,
         "coreExtractor": "successful_cycle_time_ms", "evidenceRequirements": ["executions.duration_ms"],
         "comparabilityRules": {"sameEnvironment": True}},
        {"metricId": ident(f"metric:change_failure_rate:{version}"), "referenceId": ids.reference_dora,
         "key": "change_failure_rate", "title": "Change failure rate", "unit": "ratio",
         "aggregation": "proportion", "direction": "lower", "evaluationMethod": "exact_binomial",
         "hypothesisMode": "superiority", "targetValue": 0.15, "coreExtractor": "change_failure_rate"},
        {"metricId": ident(f"metric:completeness:{version}"), "referenceId": ids.reference_slo,
         "key": "observability_completeness", "title": "Observability completeness", "unit": "ratio",
         "direction": "higher", "evaluationMethod": "conformance", "lowerBound": 0.9, "upperBound": 1,
         "coreExtractor": "observability_completeness_rate"},
        {"metricId": ident(f"metric:manual_score:{version}"), "key": "manual_score", "title": "Manual score",
         "unit": "score", "direction": "higher", "evaluationMethod": "external", "hypothesisMode": "equivalence",
         "required": False},
    ]


def benchmark_results() -> list[dict]:
    return [
        {"resultId": ident("result:cycle_time"), "metricId": ident("metric:cycle_time:v2"), "sampleCount": 3,
         "observedValue": 65123.0, "statistic": -1.25, "pValue": 0.12, "confidenceLow": 50000.5,
         "confidenceHigh": 70000.25, "verdict": "inconclusive", "readinessGaps": ["need 30 samples"],
         "details": {"method": "t"}},
        {"resultId": ident("result:cfr"), "metricId": ident("metric:change_failure_rate:v2"), "sampleCount": 2,
         "observedValue": 0.5, "pValue": 1.0, "verdict": "below"},
        {"resultId": ident("result:completeness"), "metricId": ident("metric:completeness:v2"),
         "sampleCount": 3, "observedValue": 0.5, "verdict": "below"},
        {"resultId": ident("result:manual"), "metricId": ident("metric:manual_score:v2"),
         "verdict": "insufficient_evidence"},
    ]


def error_operations(clock: OperationClock, ids: SimpleNamespace) -> list[dict]:
    op = clock.operation
    alpha = {"projectId": ids.project_alpha, "fingerprint": hex_of("fingerprint:alpha", 32),
             "exceptionClass": "TypeError", "normalizedSummary": "x is undefined"}
    return [
        op("error.record", 78, {
            **alpha, "errorGroupId": ids.error_group, "eventId": ids.event_failed,
            "eventOccurredAt": clock.at(14.57), "handled": False, "retryable": True,
            "stackHash": hex_of("stack:1", 32), "topFrame": "src/app.ts:10"}),
        op("error.record", 79, {  # same fingerprint → existing group, count 2, alias id unused
            **alpha, "errorGroupId": ids.error_group_alias, "eventId": ids.event_child_failed,
            "eventOccurredAt": clock.at(15), "handled": True}),
        op("error.record", 80, {
            "projectId": ids.project_beta, "fingerprint": hex_of("fingerprint:beta", 32),
            "errorGroupId": ids.error_group_beta, "exceptionClass": "Panic", "eventId": ids.event_beta_crash,
            "eventOccurredAt": clock.at(16)}),
    ]


def closing_operations(clock: OperationClock, ids: SimpleNamespace) -> list[dict]:
    op = clock.operation
    return [
        op("session.end", 81, {"sessionId": ids.session_beta}),
        op("prompt.finish", 82, {
            "promptId": ids.prompt_root, "status": 2, "resultSummary": "ok", "rawResult": "Tapos na ✅"}),
        op("prompt.finish", 83, {"promptId": ids.prompt_child, "status": 3, "resultSummary": ""}),
        op("prompt.finish", 84, {"promptId": ids.prompt_beta}),
    ]


# --------------------------------------------------------------------------- failing batches

def failing_batch_probes(clock: OperationClock, ids: SimpleNamespace) -> list[Probe]:
    return shape_failure_probes(clock, ids) + database_failure_probes(clock, ids) + framework_failure_probes(clock, ids)


def harmless_operation(clock: OperationClock, ids: SimpleNamespace, **overrides: object) -> dict:
    operation = clock.operation("session.end", 100, {"sessionId": ids.session_unknown})
    return {**operation, **overrides}


def batch_probe(label: str, payload: object, mode: str) -> Probe:
    return Probe(f"POST {OPERATIONS_PATH} [{label}]", "POST", OPERATIONS_PATH, mode, json_bytes(payload),
                 JSON_CONTENT_TYPE)


def shape_failure_probes(clock: OperationClock, ids: SimpleNamespace) -> list[Probe]:
    """zod issues must match exactly (path + code)."""
    harmless = lambda **overrides: harmless_operation(clock, ids, **overrides)  # noqa: E731
    everything_wrong = harmless(operationId="not-a-uuid", kind="ab", occurredAt="2026-09-30T01:02:03+08:00", data=[])
    too_many = [harmless() for _ in range(MAX_BATCH_OPERATIONS + 1)]
    cases = [
        ("empty operations", {"operations": []}),
        ("missing operations", {"ops": []}),
        ("body is an array", []),
        ("bad operation uuid", {"operations": [harmless(operationId="not-a-uuid")]}),
        ("short kind", {"operations": [harmless(kind="ab")]}),
        ("long kind", {"operations": [harmless(kind="k" * 81)]}),
        ("offset datetime", {"operations": [harmless(occurredAt="2026-09-30T01:02:03+08:00")]}),
        ("data is an array", {"operations": [harmless(data=[])]}),
        ("data is null", {"operations": [harmless(data=None)]}),
        ("every field invalid + non-object item", {"operations": [everything_wrong, "not an object"]}),
        ("501 operations", {"operations": too_many}),
    ]
    return [batch_probe(label, payload, MODE_EXACT) for label, payload in cases]


# Mental model:
# each batch starts with a valid context.record, so a missing rollback shows up later in
# /context/bootstrap; messages thrown by server.mjs itself must match, driver messages may differ
def database_failure_probes(clock: OperationClock, ids: SimpleNamespace) -> list[Probe]:
    op = clock.operation
    rollback_canary = lambda: op("context.record", 101, {  # noqa: E731
        "contextId": ids.context_rolled_back, "projectId": ids.project_alpha, "visionSummary": "must roll back"})
    cases = [
        ("unknown kind", op("nope.kind", 102, {}), MODE_EXACT),
        ("bad hex length", op("session.start", 102, {
            "sessionId": ident("session:bad-hex"), "projectId": ids.project_alpha,
            "agentInstanceId": ids.agent_codex, "sessionFingerprint": "abcd"}), MODE_EXACT),
        ("inactive benchmark metric", op("benchmark.observe", 102, {
            "observationId": ident("observation:inactive"), "programId": ids.program_delivery,
            "metricKey": "retired_metric", "value": 1, "provenanceKind": "manual"}), MODE_EXACT),
        ("foreign key violation", op("session.start", 102, {
            "sessionId": ident("session:fk"), "projectId": ids.project_unknown, "agentInstanceId": ids.agent_codex,
            "sessionFingerprint": hex_of("session:fk", 32)}), MODE_DB_ERROR),
        ("bad slug", op("project.register", 102, {
            "projectId": ident("project:bad-slug"), "displayName": "Bad", "slug": "Bad Slug!"}), MODE_DB_ERROR),
        ("bad event code", op("event.emit", 102, {
            "eventId": ident("event:bad-code"), "projectId": ids.project_alpha, "executionId": ids.exec_root,
            "code": "X", "traceId": ids.trace_root, "sequenceNo": 9}), MODE_DB_ERROR),
        ("non-finite benchmark value", op("benchmark.observe", 102, {
            "observationId": ident("observation:infinite"), "programId": ids.program_delivery,
            "metricKey": "manual_score", "value": "Infinity", "provenanceKind": "manual"}), MODE_DB_ERROR),
        ("check constraint (bad profile)", op("herdr.binding", 102, {
            "bindingId": ident("binding:bad"), "projectId": ids.project_alpha, "nativeSession": "ns-x",
            "profileName": "nobody", "runtimeScope": "herdr"}), MODE_DB_ERROR),
    ]
    return [batch_probe(label, {"operations": [rollback_canary(), failing]}, mode) for label, failing, mode in cases]


def framework_failure_probes(clock: OperationClock, ids: SimpleNamespace) -> list[Probe]:
    """Fastify parser errors: compare everything except the parser's free-text message."""
    oversized = operations_body([harmless_operation(clock, ids, data={"blob": "x" * (FASTIFY_BODY_LIMIT_BYTES + 16)})])
    valid = operations_body([harmless_operation(clock, ids)])
    cases = [
        ("unsupported media type", valid, "text/plain"),
        ("invalid json", b"{not json", JSON_CONTENT_TYPE),
        ("empty json body", b"", JSON_CONTENT_TYPE),
        ("body over 1 MiB", oversized, JSON_CONTENT_TYPE),
    ]
    # Fastify ships a text/plain parser, so that body reaches zod and must match exactly.
    return [Probe(f"POST {OPERATIONS_PATH} [{label}]", "POST", OPERATIONS_PATH,
                  MODE_EXACT if content_type == "text/plain" else MODE_FRAMEWORK_ERROR, body, content_type)
            for label, body, content_type in cases]


def acceptance_edge_probes(clock: OperationClock, ids: SimpleNamespace) -> list[Probe]:
    """Inputs one implementation may accept and the other reject; state they create is checked by GETs."""
    op = clock.operation
    day = clock.base.date().isoformat()
    lone_surrogate_batch = [
        op("execution.start", 110, {
            "executionId": ids.exec_surrogate, "projectId": ids.project_alpha, "sessionId": ids.session_alpha,
            "promptId": ids.prompt_root, "traceId": ids.trace_surrogate}),
        op("event.emit", 111, {
            "eventId": ids.event_surrogate, "projectId": ids.project_alpha, "executionId": ids.exec_surrogate,
            "code": "probe.surrogate", "traceId": ids.trace_surrogate, "messageSummary": "x\ud83dy",
            "attributes": {"note": "\ud83d"}}),
    ]
    coerced_tool = op("tool-use.record", 112, {
        "toolRecordId": ids.tool_coerced, "projectId": ids.project_beta, "nativeSession": "ns-beta",
        "toolUseId": "tu-coerced", "toolName": "Grep", "origin": "built-in", "durationMs": "25", "failed": "true"})
    cases = [
        ("datetime without seconds", [harmless_operation(clock, ids, occurredAt=f"{day}T01:02Z")]),
        ("datetime with microseconds", [harmless_operation(clock, ids, occurredAt=f"{day}T01:02:03.123456Z")]),
        ("uppercase operation id", [harmless_operation(clock, ids, operationId=ident("upper").upper())]),
        ("lone surrogate in strings", lone_surrogate_batch),
        ("string-typed numbers and booleans", [coerced_tool]),
    ]
    return [batch_probe(label, {"operations": operations}, MODE_EXACT) for label, operations in cases]


# --------------------------------------------------------------------------- GET probes

def route_probes(clock: OperationClock, ids: SimpleNamespace) -> list[Probe]:
    return ([Probe("GET /health", "GET", "/health")]
            + project_route_probes(ids) + analytics_variant_probes(ids) + bootstrap_variant_probes(ids)
            + benchmark_route_probes(clock, ids) + lookup_route_probes(ids) + unknown_route_probes(ids))


def get_probe(label: str, path: str) -> Probe:
    return Probe(f"GET {label}", "GET", path)


def project_route_probes(ids: SimpleNamespace) -> list[Probe]:
    targets = [("alpha", ids.project_alpha), ("beta", ids.project_beta), ("unknown", ids.project_unknown),
               ("alpha-uppercase", ids.project_alpha.upper()), ("malformed", "not-a-uuid")]
    return [get_probe(f"/v1/projects/{{{name}}}/{suffix}", f"/v1/projects/{project}/{suffix}")
            for name, project in targets
            for suffix in ("errors", "context/bootstrap", "benchmarks", "analytics")]


def analytics_variant_probes(ids: SimpleNamespace) -> list[Probe]:
    days_values = ["1", "0", "-5", "90", "3abc", "abc", "30", "7.9", " 2", ""]
    return [get_probe(f"/v1/projects/{{alpha}}/analytics?days={value!r}",
                      f"/v1/projects/{ids.project_alpha}/analytics?days={quote(value)}")
            for value in days_values] + [
        get_probe("/v1/projects/{beta}/analytics?days=1", f"/v1/projects/{ids.project_beta}/analytics?days=1"),
        get_probe("/v1/projects/{beta}/analytics?days=7", f"/v1/projects/{ids.project_beta}/analytics?days=7"),
    ]


def bootstrap_variant_probes(ids: SimpleNamespace) -> list[Probe]:
    queries = {
        "profile+source": "profile=cy&source=resume",
        "profile over 32": f"profile={'x' * 40}",
        "empty profile": "profile=",
        "source multibyte over 32": f"source={quote('ñ😀' * 20)}",
        "repeated profile": "profile=cy&profile=feu",
    }
    return [get_probe(f"/v1/projects/{{alpha}}/context/bootstrap [{name}]",
                      f"/v1/projects/{ids.project_alpha}/context/bootstrap?{query}")
            for name, query in queries.items()]


def benchmark_route_probes(clock: OperationClock, ids: SimpleNamespace) -> list[Probe]:
    alpha, beta = ids.project_alpha, ids.project_beta
    evaluation = lambda project, program, query="": (  # noqa: E731
        f"/v1/projects/{project}/benchmarks/{program}/evaluation-data{query}")
    local_without_zone = clock.at(0)[:19]
    variants = {
        "delivery": evaluation(alpha, ids.program_delivery),
        "delivery from=-1h": evaluation(alpha, ids.program_delivery, f"?from={quote(clock.at(-3600))}"),
        "delivery to=73.5s": evaluation(alpha, ids.program_delivery, f"?to={quote(clock.at(73.5))}"),
        "delivery from+to": evaluation(alpha, ids.program_delivery,
                                       f"?from={quote(clock.at(12))}&to={quote(clock.at(80))}"),
        "delivery to before everything": evaluation(alpha, ids.program_delivery, f"?to={quote(clock.at(-7200))}"),
        "delivery from=garbage": evaluation(alpha, ids.program_delivery, "?from=garbage"),
        "delivery from=date-only": evaluation(alpha, ids.program_delivery, f"?from={clock.base.date().isoformat()}"),
        "delivery from=local time without zone": evaluation(alpha, ids.program_delivery,
                                                            f"?from={quote(local_without_zone)}"),
        "latency": evaluation(alpha, ids.program_latency),
        "delivery under wrong project": evaluation(beta, ids.program_delivery),
        "unknown program": evaluation(alpha, ids.program_unknown),
        "malformed program": evaluation(alpha, "bad"),
        "report delivery": f"/v1/projects/{alpha}/benchmarks/{ids.program_delivery}/report",
        "report latency (no run)": f"/v1/projects/{alpha}/benchmarks/{ids.program_latency}/report",
        "report wrong project": f"/v1/projects/{beta}/benchmarks/{ids.program_delivery}/report",
    }
    return [get_probe(f"/v1/projects/:p/benchmarks/:program [{name}]", path) for name, path in variants.items()]


def lookup_route_probes(ids: SimpleNamespace) -> list[Probe]:
    errors = {"alpha": ids.error_group, "beta": ids.error_group_beta, "alias (never created)": ids.error_group_alias,
              "unknown": ids.error_group_unknown, "malformed": "not-a-uuid", "uppercase": ids.error_group.upper()}
    prompts = {"root": ids.prompt_root, "child": ids.prompt_child, "beta": ids.prompt_beta,
               "unknown": ids.prompt_unknown, "malformed": "not-a-uuid", "uppercase": ids.prompt_root.upper()}
    traces = {"root": ids.trace_root, "child": ids.trace_child, "beta": ids.trace_beta,
              "surrogate probe": ids.trace_surrogate, "uppercase": ids.trace_root.upper(),
              "unknown": ids.trace_unknown, "short": "abc", "non-hex": "zz" * 16}
    return ([get_probe(f"/v1/errors/:id [{name}]", f"/v1/errors/{value}") for name, value in errors.items()]
            + [get_probe(f"/v1/prompts/:id [{name}]", f"/v1/prompts/{value}") for name, value in prompts.items()]
            + [get_probe(f"/v1/traces/:hex [{name}]", f"/v1/traces/{value}") for name, value in traces.items()])


def unknown_route_probes(ids: SimpleNamespace) -> list[Probe]:
    return [
        get_probe("/v1/nope", "/v1/nope"),
        get_probe("/v1/nope?x=1", "/v1/nope?x=1"),
        get_probe("/v1/projects/{alpha}/errors/ (trailing slash)", f"/v1/projects/{ids.project_alpha}/errors/"),
        get_probe(OPERATIONS_PATH, OPERATIONS_PATH),
        Probe(f"DELETE {OPERATIONS_PATH}", "DELETE", OPERATIONS_PATH),
        Probe("POST /health", "POST", "/health"),
    ]
