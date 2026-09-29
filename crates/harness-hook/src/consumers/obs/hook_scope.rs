//! What one hook invocation is about (the first lines of hook.py `process`): the native event,
//! the native session, the resolved cwd, and the observability instance key.

use std::path::PathBuf;

use obs_cli::project_resolution::resolve_path;

use super::agent_identity::instance_key;
use super::payload_fields::{first_truthy, text_or, Payload};
use crate::events::python_str;
use crate::hook_context::HookContext;

const SESSION_ID_LIMIT: usize = 240;

pub struct HookScope<'a> {
    pub context: &'a HookContext,
    pub payload: &'a Payload,
    /// `str(value.get("hook_event_name") or value.get("hookEventName") or "")`.
    pub hook_event: String,
    /// `str(value.get("session_id") or value.get("sessionId") or "unknown")[:240]`.
    pub native_session: String,
    /// `Path(value.get("cwd") or os.getcwd()).resolve()`.
    pub cwd_path: PathBuf,
    /// The Python wrapper's `CODEX_OBS_INSTANCE` (always overridden from the router identity).
    pub instance_key: String,
}

impl<'a> HookScope<'a> {
    pub fn from_hook(context: &'a HookContext, payload: &'a Payload) -> Self {
        Self {
            context,
            payload,
            hook_event: text_or(
                payload,
                &["hook_event_name", "hookEventName"],
                "",
                usize::MAX,
            ),
            native_session: text_or(
                payload,
                &["session_id", "sessionId"],
                "unknown",
                SESSION_ID_LIMIT,
            ),
            cwd_path: resolved_cwd(payload),
            instance_key: instance_key(&context.event.agent, &context.event.instance),
        }
    }

    /// `str(cwd_path)`.
    #[inline]
    pub fn cwd_text(&self) -> String {
        self.cwd_path.to_string_lossy().into_owned()
    }
}

fn resolved_cwd(payload: &Payload) -> PathBuf {
    let requested = first_truthy(payload, &["cwd"])
        .map(|value| PathBuf::from(python_str(value)))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    resolve_path(&requested)
}
