//! Stop on a live execution (hook.py `elif event == "Stop"`): register the code version the turn
//! ended on, attribute it to the execution, finish execution + prompt (with the exact assistant
//! result), and clear the context row's live prompt.

use obs_cli::python_compat::{
    python_utc_now, to_python_json, uuid5_url_text, uuid7_text, JsonLayout,
};
use rusqlite::params;
use serde_json::{json, Value};

use super::code_version::CodeVersion;
use super::fingerprints::sha256_hex;
use super::hook_scope::HookScope;
use super::operations::operation;
use super::payload_fields::Payload;
use super::project_registration::ProjectIdentity;
use super::session_chain::ContextRow;
use super::spool_store::Spool;

const RESULT_KEYS: [&str; 4] = [
    "last_assistant_message",
    "lastAssistantMessage",
    "assistant_message",
    "result",
];
const COMPLETED_SUMMARY: &str = "Agent turn completed.";
const ARTIFACT_KIND_CODE_VERSION: i64 = 2;

/// Mental model: capture version (marker root, else cwd) → five operations → clear the live prompt.
pub fn finish_turn(
    scope: &HookScope,
    spool: &Spool,
    project: &ProjectIdentity,
    row: &ContextRow,
    operations: &mut Vec<Value>,
) -> anyhow::Result<()> {
    let root = project
        .marker_root
        .as_deref()
        .unwrap_or(scope.cwd_path.as_path());
    let version = CodeVersion::capture(root);
    operations.extend(completion_operations(scope.payload, project, row, &version));

    spool.conn().execute(
        "UPDATE context SET prompt_id=NULL,execution_id=NULL,trace_id=NULL,native_turn=NULL,sequence_no=0,updated_at=? WHERE native_session=?",
        params![python_utc_now(), scope.native_session],
    )?;
    Ok(())
}

fn completion_operations(
    payload: &Payload,
    project: &ProjectIdentity,
    row: &ContextRow,
    version: &CodeVersion,
) -> Vec<Value> {
    let dirty_hash = version.dirty_hash.as_deref();
    let version_key = format!(
        "{}|{}|{}",
        project.id_text,
        version.commit,
        dirty_hash.unwrap_or_default()
    );
    let code_version_id = uuid5_url_text(&version_key);
    let label = version.label();
    let execution_id = row.execution_id_json();

    vec![
        operation(
            "code-version.register",
            json!({
                "codeVersionId": code_version_id, "projectId": project.id,
                "commitSha": non_empty(&version.commit), "branchName": non_empty(&version.branch),
                "dirtyFingerprint": dirty_hash,
            }),
            None,
        ),
        operation(
            "execution.code-version",
            json!({"executionId": execution_id, "codeVersionId": code_version_id}),
            None,
        ),
        operation(
            "attribution.record",
            json!({
                "attributionId": uuid7_text(), "executionId": execution_id, "codeVersionId": code_version_id,
                "artifactKind": ARTIFACT_KIND_CODE_VERSION, "locatorHash": sha256_hex(&label), "locatorLabel": label,
                "contentHash": dirty_hash, "metadata": {"branch": version.branch, "dirty": dirty_hash.is_some()},
            }),
            None,
        ),
        operation(
            "execution.finish",
            json!({"executionId": execution_id, "status": 2, "outcome": 1, "resultSummary": COMPLETED_SUMMARY}),
            None,
        ),
        operation(
            "prompt.finish",
            json!({
                "promptId": row.prompt_id_json(), "status": 2, "resultSummary": COMPLETED_SUMMARY, "rawResult": exact_result(payload),
            }),
            None,
        ),
    ]
}

/// hook.py `exact_result`: the first non-null result key; non-strings as compact `ensure_ascii=False` JSON.
pub fn exact_result(payload: &Payload) -> String {
    match RESULT_KEYS
        .iter()
        .find_map(|key| payload.get(*key).filter(|value| !value.is_null()))
    {
        None => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => to_python_json(other, JsonLayout::Compact, false),
    }
}

#[inline]
fn non_empty(text: &str) -> Option<&str> {
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_result_prefers_first_non_null_key() {
        let payload = json!({"last_assistant_message": null, "lastAssistantMessage": "exact", "result": "other"});
        assert_eq!(exact_result(payload.as_object().unwrap()), "exact");
        let structured = json!({"result": {"a": "ñ"}});
        assert_eq!(
            exact_result(structured.as_object().unwrap()),
            "{\"a\":\"ñ\"}"
        );
        assert_eq!(exact_result(json!({}).as_object().unwrap()), "");
    }
}
