//! Human-facing agent names shared by notifications and observability (`core/display.py`).

const DEFAULT_INSTANCES: [&str; 2] = ["default", "personal"];

/// `display.title(agent, instance)`: `Claude Code`, `Codex`, or `<name> (<instance>)`.
pub fn title(agent: &str, instance: &str) -> String {
    let name = display_name(agent);
    if DEFAULT_INSTANCES.contains(&instance) {
        name.to_owned()
    } else {
        format!("{name} ({instance})")
    }
}

#[inline]
fn display_name(agent: &str) -> &str {
    match agent {
        "claude" => "Claude Code",
        "codex" => "Codex",
        other => other,
    }
}
