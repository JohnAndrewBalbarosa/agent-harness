//! Port of tests/unit/test_transcripts.py and the context-token cases of test_context_guard.py.

use std::path::{Path, PathBuf};

use harness_hook::transcripts::{claude, codex, TranscriptAgent, TurnUsage};
use serde_json::{json, Value};

fn write_jsonl(dir: &Path, name: &str, entries: &[Value]) -> PathBuf {
    let path = dir.join(name);
    let mut text: String = entries.iter().map(|entry| format!("{entry}\n")).collect();
    text.push_str("not json\n");
    std::fs::write(&path, text).unwrap();
    path
}

fn assistant(
    msg_id: &str,
    ts: &str,
    input: u64,
    output: u64,
    create: u64,
    read: u64,
    thinking: Option<u64>,
) -> Value {
    let mut usage = json!({"input_tokens": input, "output_tokens": output,
                           "cache_creation_input_tokens": create, "cache_read_input_tokens": read});
    if let Some(thinking) = thinking {
        usage["output_tokens_details"] = json!({"thinking_tokens": thinking});
    }
    json!({"type": "assistant", "timestamp": ts, "message": {"id": msg_id, "usage": usage}})
}

fn user(content: Value, ts: &str, uuid: &str, meta: bool) -> Value {
    let mut entry =
        json!({"type": "user", "timestamp": ts, "uuid": uuid, "message": {"content": content}});
    if meta {
        entry["isMeta"] = json!(true);
    }
    entry
}

fn record(
    turn: &str,
    response: &str,
    input: u64,
    cached: u64,
    output: u64,
    reasoning: u64,
) -> Value {
    json!({"timestamp": "2026-09-27T20:00:00Z", "type": "token_usage_record", "payload": {
        "turn_id": turn, "response_id": response,
        "usage": {"input_tokens": input, "cached_input_tokens": cached, "cache_write_input_tokens": 0,
                  "output_tokens": output, "reasoning_output_tokens": reasoning, "total_tokens": input + output}}})
}

#[test]
fn claude_counts_only_after_last_real_prompt_and_dedupes() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_jsonl(
        dir.path(),
        "c.jsonl",
        &[
            user(json!("first"), "2026-09-28T00:00:00Z", "p1", false),
            assistant("old", "2026-09-28T00:00:01Z", 999, 999, 0, 0, None),
            user(json!("second"), "2026-09-28T00:01:00Z", "p2", false),
            assistant("a1", "2026-09-28T00:01:01Z", 10, 1, 5, 100, Some(3)),
            assistant("a1", "2026-09-28T00:01:01Z", 10, 1, 5, 100, Some(3)),
            user(
                json!([{"type": "tool_result", "tool_use_id": "t", "content": "ok"}]),
                "2026-09-28T00:01:02Z",
                "u",
                false,
            ),
            user(json!("reminder"), "2026-09-28T00:01:02Z", "u", true),
            assistant("a2", "2026-09-28T00:01:03Z", 20, 2, 0, 50, None),
        ],
    );
    assert_eq!(
        claude::turn_usage(&path),
        TurnUsage {
            input_tokens: 30 + 150 + 5,
            output_tokens: 3,
            cached_input_tokens: 150,
            reasoning_tokens: Some(3),
            api_calls: 2,
            turn_id: Some("p2".to_owned()),
        }
    );
}

#[test]
fn claude_task_notification_starts_a_turn() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_jsonl(
        dir.path(),
        "c.jsonl",
        &[
            user(json!("go"), "2026-09-28T00:00:00Z", "p1", false),
            assistant("a", "2026-09-28T00:00:01Z", 5, 0, 0, 0, None),
            user(
                json!("<task-notification>\n<task-id>x</task-id>"),
                "2026-09-28T00:05:00Z",
                "n1",
                false,
            ),
            assistant("b", "2026-09-28T00:05:01Z", 7, 0, 0, 0, None),
        ],
    );
    let usage = claude::turn_usage(&path);
    assert_eq!(
        (usage.turn_id.as_deref(), usage.input_tokens),
        (Some("n1"), 7)
    );
}

#[test]
fn claude_missing_transcript_is_empty_usage() {
    let usage = claude::turn_usage(Path::new("/nope/missing.jsonl"));
    assert_eq!(
        (usage.api_calls, usage.input_tokens, usage.turn_id),
        (0, 0, None)
    );
}

#[test]
fn codex_sums_turn_records_and_dedupes_responses() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_jsonl(
        dir.path(),
        "x.jsonl",
        &[
            record("t-old", "r0", 999, 0, 999, 0),
            record("t1", "r1", 100, 60, 10, 4),
            record("t1", "r1", 100, 60, 10, 4),
            record("t1", "r2", 200, 120, 20, 6),
            json!({"type": "event_msg", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 5}}}}),
        ],
    );
    assert_eq!(
        codex::turn_usage(&path, Some("t1")),
        TurnUsage {
            input_tokens: 300,
            output_tokens: 30,
            cached_input_tokens: 180,
            reasoning_tokens: Some(10),
            api_calls: 2,
            turn_id: Some("t1".to_owned()),
        }
    );
}

#[test]
fn codex_without_turn_id_uses_latest_turn() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_jsonl(
        dir.path(),
        "x.jsonl",
        &[
            record("t1", "r1", 1, 0, 1, 0),
            record("t2", "r2", 7, 0, 3, 1),
        ],
    );
    let usage = codex::turn_usage(&path, None);
    assert_eq!(
        (usage.turn_id.as_deref(), usage.input_tokens),
        (Some("t2"), 7)
    );
}

#[test]
fn codex_unknown_turn_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_jsonl(dir.path(), "x.jsonl", &[record("t1", "r1", 1, 0, 1, 0)]);
    assert_eq!(codex::turn_usage(&path, Some("zzz")).api_calls, 0);
}

#[test]
fn claude_last_request_context() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_jsonl(
        dir.path(),
        "c.jsonl",
        &[
            json!({"type": "assistant", "message": {"id": "m1", "usage": {"input_tokens": 5, "cache_read_input_tokens": 1000, "cache_creation_input_tokens": 10, "output_tokens": 3}}}),
            json!({"type": "user", "message": {"content": "x"}}),
            json!({"type": "assistant", "message": {"id": "m2", "usage": {"input_tokens": 7, "cache_read_input_tokens": 180000, "cache_creation_input_tokens": 500, "output_tokens": 9}}}),
        ],
    );
    assert_eq!(claude::context_tokens(&path), Some(180_507));
}

#[test]
fn codex_last_request_context() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_jsonl(
        dir.path(),
        "x.jsonl",
        &[
            json!({"type": "event_msg", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 90000}}}}),
            json!({"type": "event_msg", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 210000}}}}),
            json!({"type": "event_msg", "payload": {"type": "token_count", "info": null}}),
        ],
    );
    assert_eq!(codex::context_tokens(&path), Some(210_000));
}

#[test]
fn missing_transcript_context_is_none() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(claude::context_tokens(&dir.path().join("nope.jsonl")), None);
}

#[test]
fn agent_dispatch_knows_only_claude_and_codex() {
    assert_eq!(
        TranscriptAgent::from_agent("claude"),
        Some(TranscriptAgent::Claude)
    );
    assert_eq!(
        TranscriptAgent::from_agent("codex"),
        Some(TranscriptAgent::Codex)
    );
    assert_eq!(TranscriptAgent::from_agent("gemini"), None);
}
