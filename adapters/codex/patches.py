"""Codex CLI config.toml settings not covered by rulesync hooks: the OpenTelemetry exporter (spike 2026-09-28)."""
from __future__ import annotations

from core.config import Config

OTEL_BEGIN = "# >>> agent-harness managed >>>"
OTEL_END = "# <<< agent-harness managed <<<"


def otel_block(cfg: Config) -> str:
    endpoint = f"http://127.0.0.1:{cfg.otlp_port}/v1/logs"
    return f'[otel]\nexporter = {{ otlp-http = {{ endpoint = "{endpoint}", protocol = "json" }} }}'
