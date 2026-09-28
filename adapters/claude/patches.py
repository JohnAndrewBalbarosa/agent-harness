"""Claude Code settings not covered by rulesync hooks: OpenTelemetry env and the policy @import block."""
from __future__ import annotations

from pathlib import Path

from core.config import Config

POLICY_BEGIN = "<!-- agent-harness:begin -->"
POLICY_END = "<!-- agent-harness:end -->"


def env(cfg: Config) -> dict[str, str]:
    return {
        "CLAUDE_CODE_ENABLE_TELEMETRY": "1",
        "OTEL_METRICS_EXPORTER": "none",
        "OTEL_LOGS_EXPORTER": "otlp",
        "OTEL_EXPORTER_OTLP_LOGS_PROTOCOL": "http/json",
        "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT": f"http://127.0.0.1:{cfg.otlp_port}/v1/logs",
        "OTEL_LOGS_EXPORT_INTERVAL": "5000",
    }


def policy_import(cfg: Config, home: Path | None = None) -> str:
    """`~/` form under the user profile, so usernames with spaces cannot break Claude's @import parsing."""
    policy = cfg.harness_home / "policy" / "AGENTS.md"
    home = home or Path.home()
    try:
        return "@~/" + policy.relative_to(home).as_posix()
    except ValueError:
        return "@" + policy.as_posix()
