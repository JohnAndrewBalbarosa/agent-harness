//! Port of tests/unit/test_usage_fallback.py plus spool enqueue/lookup against a temp spool.

use std::collections::BTreeMap;
use std::path::Path;

use harness_hook::consumers::usage_fallback::spool_enqueue::{enqueue_usage, spool_project};
use harness_hook::consumers::usage_fallback::usage_record::{build_record, global_project_id};
use harness_hook::consumers::usage_fallback::{handle, LOCAL_LOG_FILE, LOG_MAX_RECORDS};
use harness_hook::events::{make_event, Event};
use harness_hook::transcripts::TurnUsage;
use serde_json::{json, Map, Value};

fn event(event_type: &str, agent: &str, instance: &str, payload: Value) -> Event {
    let payload = payload.as_object().cloned().unwrap();
    let native = if event_type == "turn.stop" {
        "Stop"
    } else {
        "UserPromptSubmit"
    };
    make_event(event_type, agent, instance, &payload, native, Map::new())
}

fn usage(
    input: u64,
    output: u64,
    cached: u64,
    reasoning: Option<u64>,
    calls: u64,
    turn: Option<&str>,
) -> TurnUsage {
    TurnUsage {
        input_tokens: input,
        output_tokens: output,
        cached_input_tokens: cached,
        reasoning_tokens: reasoning,
        api_calls: calls,
        turn_id: turn.map(str::to_owned),
    }
}

fn write_rollout(dir: &Path) -> String {
    let rollout = dir.join("rollout.jsonl");
    let line = json!({"type": "token_usage_record", "payload": {
        "turn_id": "t1", "response_id": "r1",
        "usage": {"input_tokens": 50, "cached_input_tokens": 20, "output_tokens": 5, "reasoning_output_tokens": 1}}});
    std::fs::write(&rollout, format!("{line}\n")).unwrap();
    rollout.display().to_string()
}

#[test]
fn record_schema() {
    let stop = event(
        "turn.stop",
        "codex",
        "cy",
        json!({"session_id": "s1", "turn_id": "t1"}),
    );
    let record = build_record(
        &stop,
        &usage(300, 30, 180, Some(10), 2, Some("t1")),
        "project-1",
    );
    assert_eq!(
        (
            record["source"].as_str(),
            record["availability"].as_str(),
            record["projectId"].as_str()
        ),
        (Some("codex_hook"), Some("exact"), Some("project-1"))
    );
    assert_eq!(
        (
            &record["inputTokens"],
            &record["outputTokens"],
            &record["cachedInputTokens"],
            &record["reasoningTokens"]
        ),
        (&json!(300), &json!(30), &json!(180), &json!(10))
    );
    assert_eq!(
        (
            record["nativeSession"].as_str(),
            record["nativeTurn"].as_str(),
            record["sourceEventId"].as_str()
        ),
        (Some("s1"), Some("t1"), Some("s1:t1:Stop"))
    );
    let keys: Vec<&str> = record
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "usageId",
            "projectId",
            "nativeSession",
            "nativeTurn",
            "promptId",
            "inputTokens",
            "outputTokens",
            "cachedInputTokens",
            "reasoningTokens",
            "availability",
            "source",
            "sourceEventId"
        ]
    );
}

#[test]
fn usage_id_is_python_uuid5_of_the_turn() {
    let stop = event("turn.stop", "claude", "default", json!({"session_id": "s"}));
    let record = build_record(&stop, &usage(0, 0, 0, None, 0, None), "p");
    // str(uuid.uuid5(uuid.NAMESPACE_URL, "claude-hook:s:None")), turn None rendered like Python.
    assert_eq!(
        record["usageId"],
        obs_cli::python_compat::uuid5_url_text("claude-hook:s:None")
    );
    assert_eq!(record["sourceEventId"], "s:None:Stop");
    assert_eq!(record["nativeTurn"], Value::Null);
}

#[test]
fn no_calls_is_unavailable() {
    let stop = event("turn.stop", "claude", "default", json!({"session_id": "s"}));
    let record = build_record(&stop, &usage(0, 0, 0, None, 0, None), "p");
    assert_eq!(record["availability"], "unavailable");
    assert_eq!(record["reasoningTokens"], Value::Null);
}

#[test]
fn same_turn_gets_same_usage_id() {
    let stop = event(
        "turn.stop",
        "codex",
        "personal",
        json!({"session_id": "s", "turn_id": "t"}),
    );
    let turn = usage(1, 1, 0, Some(0), 1, Some("t"));
    assert_eq!(
        build_record(&stop, &turn, "p")["usageId"],
        build_record(&stop, &turn, "p")["usageId"]
    );
}

#[test]
fn consumer_writes_local_log_in_dry_run() {
    let dir = tempfile::tempdir().unwrap();
    let rollout = write_rollout(dir.path());
    let stop = event(
        "turn.stop",
        "codex",
        "personal",
        json!({"session_id": "s1", "turn_id": "t1", "transcript_path": rollout}),
    );
    let env = BTreeMap::from([
        ("AH_USAGE_DIR".to_owned(), dir.path().display().to_string()),
        ("AH_USAGE_DRY_RUN".to_owned(), "1".to_owned()),
        (
            "AH_OBS_SPOOL".to_owned(),
            dir.path().join("missing.sqlite3").display().to_string(),
        ),
    ]);

    let record = handle(&stop, &env, dir.path()).unwrap().unwrap();
    assert_eq!(record["projectId"], global_project_id());

    let text = std::fs::read_to_string(dir.path().join(LOCAL_LOG_FILE)).unwrap();
    let logged: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
    assert_eq!(
        (
            logged["source"].as_str(),
            &logged["inputTokens"],
            &logged["enqueued"]
        ),
        (Some("codex_hook"), &json!(50), &json!(false))
    );
    assert_eq!(logged["apiCalls"], 1);
    assert!(logged["recordedAt"].as_str().unwrap().ends_with("+00:00"));
    assert!(
        !dir.path().join("missing.sqlite3").exists(),
        "dry run must not create the spool"
    );
}

#[test]
fn consumer_ignores_non_stop_and_missing_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let env = BTreeMap::from([("AH_USAGE_DRY_RUN".to_owned(), "1".to_owned())]);
    let prompt = event(
        "prompt.submit",
        "codex",
        "personal",
        json!({"session_id": "s"}),
    );
    let bare_stop = event("turn.stop", "codex", "personal", json!({"session_id": "s"}));
    assert!(handle(&prompt, &env, dir.path()).unwrap().is_none());
    assert!(handle(&bare_stop, &env, dir.path()).unwrap().is_none());
}

#[test]
fn local_log_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let seeded: String = (0..LOG_MAX_RECORDS)
        .map(|index| format!("{{\"n\": {index}}}\n"))
        .collect();
    std::fs::write(dir.path().join(LOCAL_LOG_FILE), seeded).unwrap();
    let rollout = write_rollout(dir.path());
    let stop = event(
        "turn.stop",
        "codex",
        "personal",
        json!({"session_id": "s1", "turn_id": "t1", "transcript_path": rollout}),
    );
    let env = BTreeMap::from([
        ("AH_USAGE_DIR".to_owned(), dir.path().display().to_string()),
        ("AH_USAGE_DRY_RUN".to_owned(), "1".to_owned()),
    ]);
    handle(&stop, &env, dir.path()).unwrap();
    let text = std::fs::read_to_string(dir.path().join(LOCAL_LOG_FILE)).unwrap();
    assert_eq!(text.lines().count(), LOG_MAX_RECORDS);
    assert!(dir.path().join("turn-usage.jsonl.lock").exists());
}

#[test]
fn enqueue_writes_a_redacted_operation_and_lookup_reads_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let spool = dir.path().join("var").join("spool.sqlite3");
    let stop = event(
        "turn.stop",
        "codex",
        "cy",
        json!({"session_id": "s1", "turn_id": "t1"}),
    );
    let record = build_record(&stop, &usage(3, 2, 1, Some(0), 1, Some("t1")), "p");

    enqueue_usage(&spool, &record, &BTreeMap::new()).unwrap();

    let conn = rusqlite::Connection::open(&spool).unwrap();
    let payload: String = conn
        .query_row("SELECT payload FROM queue", [], |row| row.get(0))
        .unwrap();
    let operation: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(operation["kind"], "turn-usage.record");
    assert_eq!(operation["data"]["usageId"], record["usageId"]);
    assert!(payload.starts_with(r#"{"operationId":""#));

    assert_eq!(spool_project(&spool, "s1"), None);
    conn.execute(
        "INSERT INTO context(native_session,project_id,session_id,cwd,updated_at) VALUES('s1','proj-9','x','/w','t')",
        [],
    )
    .unwrap();
    assert_eq!(spool_project(&spool, "s1").as_deref(), Some("proj-9"));
    assert_eq!(
        spool_project(&dir.path().join("absent.sqlite3"), "s1"),
        None
    );
}
