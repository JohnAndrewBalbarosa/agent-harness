"""Scenario catalogue for the hook parity run: workspaces, synthetic transcripts and the ordered hook steps.

Every step is fed byte-identically to side A (Python router) and side B (Rust exe); payload paths point at one shared
workspace and one shared transcript per agent, so dedupe fingerprints and identity hashes are comparable.
"""
from __future__ import annotations

import json
import subprocess
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[2]
TEST_PROJECT_ID = "01a0d736-0000-7000-8000-00000000a11e"
LONE_SURROGATE_PROMPT_JSON = b'"gawin mo \\ud800 na"'


@dataclass(frozen=True)
class Step:
    agent: str
    native_event: str | None
    stdin: bytes

    @property
    def label(self) -> str:
        return f"{self.agent}:{self.native_event or '-'}"


@dataclass(frozen=True)
class Scenario:
    name: str
    steps: tuple[Step, ...]
    expected_deviation: str | None = None


@dataclass(frozen=True)
class Workspace:
    marked: Path
    unmarked: Path
    claude_transcript: Path
    codex_transcript: Path


# Mental model: fixtures (one scenario each) → scripted sessions (marked + unmarked cwd) → edge-case inputs.
def build_scenarios(workspace: Workspace) -> list[Scenario]:
    scenarios = []
    for agent in ("claude", "codex"):
        scenarios.extend(fixture_scenarios(agent, workspace))
        scenarios.append(scripted_session(agent, workspace, workspace.marked, f"{agent}-session-marked", full=True))
        scenarios.append(scripted_session(agent, workspace, workspace.unmarked, f"{agent}-session-global", full=False))
    scenarios.extend(edge_case_scenarios(workspace))
    return scenarios


def fixture_scenarios(agent: str, workspace: Workspace) -> list[Scenario]:
    scenarios = []
    for path in sorted((REPO / "adapters" / agent / "fixtures").glob("*.json")):
        payload = json.loads(path.read_text(encoding="utf-8"))
        payload = rewrite_payload(payload, agent, workspace, workspace.marked, f"fx-{agent}-{path.stem}")
        native = str(payload.get("hook_event_name") or path.stem.split(".")[0])
        scenarios.append(Scenario(f"fixture:{agent}:{path.stem}", (Step(agent, native, encode(payload)),)))
    return scenarios


def rewrite_payload(payload: dict[str, Any], agent: str, workspace: Workspace, cwd: Path, session: str) -> dict[str, Any]:
    transcript = workspace.claude_transcript if agent == "claude" else workspace.codex_transcript
    return {**payload, "session_id": session, "cwd": str(cwd), "transcript_path": str(transcript)}


# Mental model: SessionStart → prompt → 3 tools (ok/failed/background) → Stop → prompt → Stop → subagent pair →
# attention → SessionEnd; the unmarked variant keeps only start → prompt → tool → Stop → end.
def scripted_session(agent: str, workspace: Workspace, cwd: Path, name: str, full: bool) -> Scenario:
    session = f"scripted-{name}"
    base = rewrite_payload({}, agent, workspace, cwd, session)
    turn = (lambda n: {"turn_id": f"turn-{n}"}) if agent == "codex" else (lambda _n: {})
    events = [
        ("SessionStart", {"source": "startup"}),
        ("UserPromptSubmit", {"prompt": "ayusin mo ang parity test ñ", **turn(1)}),
        ("PostToolUse", {**first_tool(agent), **turn(1)}),
    ]
    if full:
        events += [
            ("PostToolUse", {**failed_tool(agent), **turn(1)}),
            ("PostToolUse", {**background_tool(agent), **turn(1)}),
        ]
    events.append(("Stop", {"last_assistant_message": "Tapos na ang unang turn.", "stop_hook_active": False, **turn(1)}))
    if full:
        events += [
            ("UserPromptSubmit", {"prompt": "isa pa", **turn(2)}),
            ("Stop", {"last_assistant_message": "Tapos na ulit.", "stop_hook_active": False, **turn(2)}),
            ("SubagentStart", {"agent_id": "agent-parity-1", "agent_type": "worker", **turn(2)}),
            ("SubagentStop", {"agent_id": "agent-parity-1", "agent_type": "worker",
                              "last_assistant_message": "sub done", **turn(2)}),
            attention_event(agent),
        ]
    events.append(("SessionEnd", {"reason": "exit"}))
    steps = tuple(Step(agent, native, encode({"hook_event_name": native, **base, **extra})) for native, extra in events)
    return Scenario(f"session:{name}", steps)


def first_tool(agent: str) -> dict[str, Any]:
    if agent == "codex":
        questions = [{"question": "Which option should be used for the next deployment step?",
                      "options": [{"label": "A", "description": "Deploy the new build to production right now"}]}]
        return {"tool_name": "request_user_input", "tool_input": {"questions": questions},
                "tool_response": {"answer": "A"}, "tool_use_id": "call_rui_1"}
    return {"tool_name": "Bash", "tool_input": {"command": "echo hi"}, "tool_use_id": "toolu_ok_1",
            "tool_response": {"stdout": "hi", "stderr": "", "interrupted": False}, "duration_ms": 42,
            "started_at": "2026-09-30T01:02:03.000Z"}


def failed_tool(agent: str) -> dict[str, Any]:
    name = "shell" if agent == "codex" else "Bash"
    return {"tool_name": name, "tool_input": {"command": "exit 3"}, "tool_use_id": f"{agent}_fail_1",
            "tool_response": {"exit_code": 3, "output": "boom", "is_error": True}}


def background_tool(agent: str) -> dict[str, Any]:
    name = "exec_command" if agent == "codex" else "Bash"
    return {"tool_name": name, "tool_input": {"command": "npm run dev", "run_in_background": True},
            "tool_use_id": f"{agent}_bg_1",
            "tool_response": {"session_id": "bgparity1", "stdout": "Command running in background with ID: bgparity1"}}


def attention_event(agent: str) -> tuple[str, dict[str, Any]]:
    if agent == "codex":
        return "PermissionRequest", {"tool_name": "shell", "tool_input": {"command": ["rm", "x"]}, "turn_id": "turn-2"}
    return "Notification", {"message": "Claude needs your permission to use Bash", "notification_type": "permission_prompt"}


def edge_case_scenarios(workspace: Workspace) -> list[Scenario]:
    surrogate = rewrite_payload({"hook_event_name": "UserPromptSubmit"}, "claude", workspace, workspace.marked,
                                "edge-surrogate")
    surrogate_bytes = encode(surrogate)[:-1] + b', "prompt": ' + LONE_SURROGATE_PROMPT_JSON + b"}"
    return [
        Scenario("edge:malformed-stdin", (Step("claude", "UserPromptSubmit", b"{not json"),
                                          Step("codex", "Stop", b"{not json"))),
        Scenario("edge:empty-stdin", (Step("claude", None, b""), Step("codex", "Stop", b""))),
        Scenario("edge:unknown-agent", (Step("gemini", "Stop", b"{}"), Step("Bad-Agent!", None, b"{}"))),
        Scenario("edge:lone-surrogate", (Step("claude", "UserPromptSubmit", surrogate_bytes),),
                 expected_deviation="lone-surrogate: Rust decodes \\ud800 as U+FFFD, Python fails to encode it"),
    ]


def encode(payload: dict[str, Any]) -> bytes:
    return json.dumps(payload, ensure_ascii=False).encode("utf-8")


# Mental model: marked git repo (fixed project UUID, one commit) + plain dir + transcripts stamped "now".
def build_workspace(base: Path) -> Workspace:
    marked, unmarked = base / "ws-marked", base / "ws-plain"
    marked.mkdir()
    unmarked.mkdir()
    (marked / "observability.project.toml").write_text(
        f'[project]\nid = "{TEST_PROJECT_ID}"\nname = "hook-parity"\nslug = "hook-parity"\nschema_version = 1\n',
        encoding="utf-8")
    (marked / "README.md").write_text("parity workspace\n", encoding="utf-8")
    init_git_repo(marked, base / "no-hooks")
    transcripts = base / "transcripts"
    transcripts.mkdir()
    workspace = Workspace(marked, unmarked, transcripts / "claude-session.jsonl", transcripts / "codex-rollout.jsonl")
    write_jsonl(workspace.claude_transcript, claude_transcript_entries())
    write_jsonl(workspace.codex_transcript, codex_transcript_entries())
    return workspace


def init_git_repo(path: Path, empty_hooks: Path) -> None:
    empty_hooks.mkdir()
    config = ["-c", f"core.hooksPath={empty_hooks}", "-c", "commit.gpgsign=false", "-c", "user.name=parity",
              "-c", "user.email=parity@example.invalid"]
    for args in (["init", "-q", "-b", "main"], ["add", "."], ["commit", "-q", "-m", "parity workspace"]):
        subprocess.run(["git", *config, *args], cwd=path, check=True, capture_output=True)


def claude_transcript_entries() -> list[dict[str, Any]]:
    now = datetime.now(timezone.utc)
    stamp = lambda seconds: (now - timedelta(seconds=seconds)).isoformat().replace("+00:00", "Z")
    return [
        {"type": "user", "uuid": "u-1", "timestamp": stamp(60), "message": {"role": "user", "content": "ayusin mo"}},
        {"type": "assistant", "uuid": "a-1", "timestamp": stamp(55), "requestId": "req-1",
         "message": {"id": "msg-1", "usage": {"input_tokens": 12, "output_tokens": 40,
                                              "cache_creation_input_tokens": 300, "cache_read_input_tokens": 90000}}},
        {"type": "user", "uuid": "u-2", "timestamp": stamp(50), "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "toolu_bg", "content": "Command running in background with ID: bgparity1"}]}},
        {"type": "assistant", "uuid": "a-2", "timestamp": stamp(45), "requestId": "req-2",
         "message": {"id": "msg-2", "usage": {"input_tokens": 8, "output_tokens": 70, "cache_creation_input_tokens": 500,
                                              "cache_read_input_tokens": 171000,
                                              "output_tokens_details": {"thinking_tokens": 33}}}},
        {"type": "assistant", "uuid": "a-3", "timestamp": stamp(44), "requestId": "req-2",
         "message": {"id": "msg-2", "usage": {"input_tokens": 8, "output_tokens": 70, "cache_creation_input_tokens": 500,
                                              "cache_read_input_tokens": 171000}}},
    ]


def codex_transcript_entries() -> list[dict[str, Any]]:
    stamp = datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")
    usage = lambda i, c, o, r: {"input_tokens": i, "cached_input_tokens": c, "output_tokens": o, "reasoning_output_tokens": r}
    return [
        {"timestamp": stamp, "type": "token_usage_record", "payload": {"turn_id": "turn-1", "response_id": "resp-1", "usage": usage(9000, 4000, 300, 120)}},
        {"timestamp": stamp, "type": "token_usage_record", "payload": {"turn_id": "turn-1", "response_id": "resp-1", "usage": usage(9000, 4000, 300, 120)}},
        {"timestamp": stamp, "type": "token_usage_record", "payload": {"turn_id": "turn-1", "response_id": "resp-2", "usage": usage(9500, 9000, 200, 50)}},
        {"timestamp": stamp, "type": "token_usage_record", "payload": {"turn_id": "turn-2", "response_id": "resp-3", "usage": usage(12000, 9400, 90, 10)}},
        {"timestamp": stamp, "type": "event_msg", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 12000}}}},
    ]


def write_jsonl(path: Path, entries: list[dict[str, Any]]) -> None:
    path.write_text("".join(json.dumps(entry) + "\n" for entry in entries), encoding="utf-8")
