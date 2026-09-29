//! hook.py `instruction_load_state`: whether the global and project `AGENTS.md` exist, by path and
//! SHA-256 only (content is never copied). The Python wrapper always exports
//! `CODEX_OBS_HOME=<harness>/policy`, so the global prompt is `<harness>/policy/AGENTS.md`.

use std::path::Path;

use obs_cli::project_resolution::resolve_path;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const POLICY_DIRECTORY: &str = "policy";
const PROMPT_FILE_NAME: &str = "AGENTS.md";

/// The `agent.instructions.load_state` attributes (also its lossless `rawPayload`).
pub struct InstructionState {
    pub loaded: bool,
    pub document: Value,
}

pub fn instruction_load_state(
    harness_root: &Path,
    instance_key: &str,
    cwd_path: &Path,
) -> InstructionState {
    let codex_home = resolve_path(&harness_root.join(POLICY_DIRECTORY));
    let global_state = inspect_prompt(&codex_home.join(PROMPT_FILE_NAME));
    let project_state = inspect_prompt(&cwd_path.join(PROMPT_FILE_NAME));
    let loaded = global_state["found"] == Value::Bool(true);

    InstructionState {
        loaded,
        document: json!({
            "instance": instance_key,
            "codexHome": codex_home.to_string_lossy(),
            "globalPrompt": global_state,
            "projectPrompt": project_state,
            "loaded": loaded,
            "acknowledgementRequired": true,
            "verification": "filesystem_and_hook",
        }),
    }
}

/// `{"path", "found", "sha256"}`; any read error counts as not found.
fn inspect_prompt(path: &Path) -> Value {
    let path_text = path.to_string_lossy();
    match std::fs::read(path) {
        Ok(content) => {
            json!({"path": path_text, "found": true, "sha256": hex::encode(Sha256::digest(content))})
        }
        Err(_) => json!({"path": path_text, "found": false, "sha256": null}),
    }
}
