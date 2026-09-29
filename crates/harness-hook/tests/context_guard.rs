//! Port of the guard cases of tests/unit/test_context_guard.py.

use std::collections::BTreeMap;
use std::path::PathBuf;

use harness_hook::consumers::context_guard::{
    handle, warning_message, GuardOutcome, LIFECYCLE_FILE,
};
use harness_hook::events::make_event;
use serde_json::{json, Map};

struct Guard {
    dir: tempfile::TempDir,
    sent: Vec<String>,
}

impl Guard {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            sent: Vec::new(),
        }
    }

    fn root(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    fn stop(&mut self, context: u64) -> GuardOutcome {
        let root = self.root();
        let transcript = root.join("t.jsonl");
        let line = json!({"type": "assistant", "message": {"id": format!("m{context}"), "usage": {
            "input_tokens": 0, "cache_read_input_tokens": context, "cache_creation_input_tokens": 0, "output_tokens": 1}}});
        std::fs::write(&transcript, format!("{line}\n")).unwrap();

        let payload =
            json!({"session_id": "s1", "transcript_path": transcript.display().to_string()});
        let event = make_event(
            "turn.stop",
            "claude",
            "default",
            payload.as_object().unwrap(),
            "Stop",
            Map::new(),
        );
        let env = BTreeMap::from([
            ("CONTEXT_WARN_TOKENS".to_owned(), "150000".to_owned()),
            (
                "AH_CONTEXT_GUARD_DIR".to_owned(),
                root.join("state").display().to_string(),
            ),
            (
                "AH_LOG_DIR".to_owned(),
                root.join("logs").display().to_string(),
            ),
        ]);
        let sent = &mut self.sent;
        let mut notify = |message: &str| sent.push(message.to_owned());
        handle(&event, &env, &root, &mut notify).unwrap()
    }
}

#[test]
fn below_threshold_is_quiet() {
    let mut guard = Guard::new();
    assert_eq!(guard.stop(90_000), GuardOutcome::Below);
    assert!(guard.sent.is_empty());
}

#[test]
fn warns_once_per_50k_band() {
    let mut guard = Guard::new();
    assert_eq!(guard.stop(160_000), GuardOutcome::Warned);
    assert_eq!(guard.stop(170_000), GuardOutcome::AlreadyWarned);
    assert_eq!(guard.stop(205_000), GuardOutcome::Warned);
    assert_eq!(guard.sent.len(), 2);
    assert!(guard.sent.last().unwrap().contains("205K"));

    let log = std::fs::read_to_string(guard.root().join("logs").join(LIFECYCLE_FILE)).unwrap();
    assert!(log.contains("context.warned"));
    assert!(log.contains(r#""details":{"agent":"claude","instance":"default","session":"s1","tokens":205000,"threshold":150000}"#));

    let state = std::fs::read_to_string(guard.root().join("state").join("claude-s1.json")).unwrap();
    assert_eq!(state, r#"{"band": 4, "tokens": 205000}"#);
}

#[test]
fn non_stop_and_unknown_agents_are_skipped() {
    let guard = Guard::new();
    let root = guard.root();
    let payload = json!({"session_id": "s1", "transcript_path": "x.jsonl"});
    let mut notify = |_: &str| panic!("must not notify");
    let prompt = make_event(
        "prompt.submit",
        "claude",
        "default",
        payload.as_object().unwrap(),
        "UserPromptSubmit",
        Map::new(),
    );
    assert_eq!(
        handle(&prompt, &BTreeMap::new(), &root, &mut notify).unwrap(),
        GuardOutcome::Ignored
    );
    let other = make_event(
        "turn.stop",
        "gemini",
        "default",
        payload.as_object().unwrap(),
        "Stop",
        Map::new(),
    );
    assert_eq!(
        handle(&other, &BTreeMap::new(), &root, &mut notify).unwrap(),
        GuardOutcome::Unsupported
    );
}

#[test]
fn message_rounds_half_to_even_like_python() {
    assert!(warning_message("claude", 150_500).starts_with("claude: 150K tokens per request."));
    assert!(warning_message("codex", 151_500).starts_with("codex: 152K"));
}
