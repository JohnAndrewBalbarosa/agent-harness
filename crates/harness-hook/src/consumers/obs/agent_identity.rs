//! Agent identity as the observability hub knows it (`core/consumers/obs.py` `instance_key` +
//! hook.py `agent_id`). Codex keeps its legacy instance keys so existing history stays continuous.

use obs_cli::python_compat::uuid5_url_text;

const DEFAULT_INSTANCES: [&str; 2] = ["default", ""];

/// `instance_key(agent, instance)`: the value the Python wrapper exports as `CODEX_OBS_INSTANCE`.
pub fn instance_key(agent: &str, instance: &str) -> String {
    if agent == "codex" {
        return instance.to_owned();
    }
    if DEFAULT_INSTANCES.contains(&instance) {
        agent.to_owned()
    } else {
        format!("{agent}-{instance}")
    }
}

/// `uuid5(NAMESPACE_URL, f"codex-observability:{instance_key}")`.
#[inline]
pub fn agent_instance_id(instance_key: &str) -> String {
    uuid5_url_text(&format!("codex-observability:{instance_key}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_instance_keys_keep_legacy_agent_ids() {
        assert_eq!(instance_key("codex", "personal"), "personal");
        assert_eq!(instance_key("codex", "cy"), "cy");
        assert_eq!(
            agent_instance_id(&instance_key("codex", "personal")),
            uuid5_url_text("codex-observability:personal")
        );
    }

    #[test]
    fn other_agents_are_prefixed_unless_default() {
        assert_eq!(instance_key("claude", "default"), "claude");
        assert_eq!(instance_key("claude", ""), "claude");
        assert_eq!(instance_key("claude", "work"), "claude-work");
    }
}
