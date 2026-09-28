"""Local OTLP/HTTP-JSON receiver for every connected agent's built-in OpenTelemetry (spec §9 usage).

Log records are dispatched by resource `service.name` (prefix match against each adapter manifest's
`otel_service_names`) to `adapters.<agent>.otel.map`, which whitelists fields into turn-usage.record data.
Listens on 127.0.0.1 only. CLI: `serve` (quiet exit if the port is taken) | `ensure` (start hidden if not healthy).
"""
from __future__ import annotations

import importlib
import json
import os
import socket
import subprocess
import sys
import tomllib
from functools import lru_cache
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from typing import Any, Callable
from urllib.request import urlopen

from core.config import runtime_env
from core.usage.obs_client import GLOBAL_PROJECT_ID, ROOT, enqueue_usage, spool_project

HOST = "127.0.0.1"
MAX_BODY_BYTES = 10 * 1024 * 1024
ENV_FILE = ROOT / ".env"


def port() -> int:
    return int(runtime_env(ENV_FILE, os.environ).get("OTLP_PORT") or 4320)


def _any_value(value: Any) -> Any:
    if not isinstance(value, dict):
        return None
    for key in ("stringValue", "intValue", "doubleValue", "boolValue"):
        if key in value:
            return value[key]
    return None


def decode_attrs(payload: Any) -> list[tuple[str, dict[str, Any]]]:
    """[(service.name, log-record attributes)] for every log record in an OTLP JSON logs payload."""
    found: list[tuple[str, dict[str, Any]]] = []
    resource_logs = payload.get("resourceLogs") if isinstance(payload, dict) else None
    for resource_log in resource_logs if isinstance(resource_logs, list) else []:
        resource = {a.get("key"): _any_value(a.get("value")) for a in (resource_log or {}).get("resource", {}).get("attributes") or []
                    if isinstance(a, dict)}
        service = str(resource.get("service.name") or "")
        for scope_log in (resource_log or {}).get("scopeLogs") or []:
            for log_record in (scope_log or {}).get("logRecords") or []:
                attrs = {a.get("key"): _any_value(a.get("value")) for a in log_record.get("attributes") or [] if isinstance(a, dict)}
                found.append((service, attrs))
    return found


@lru_cache(maxsize=1)
def _service_prefixes() -> tuple[tuple[str, str], ...]:
    prefixes = []
    for manifest in sorted((ROOT / "adapters").glob("*/manifest.toml")):
        data = tomllib.loads(manifest.read_text(encoding="utf-8"))
        prefixes.extend((prefix, data["agent"]) for prefix in data.get("otel_service_names", []))
    return tuple(prefixes)


def adapter_for(service_name: str) -> str | None:
    return next((agent for prefix, agent in _service_prefixes() if service_name.startswith(prefix)), None)


def usage_records(payload: Any, resolve_project: Callable[[str], str | None] = spool_project) -> list[dict]:
    records = []
    for service, attrs in decode_attrs(payload):
        agent = adapter_for(service)
        if agent is None:
            continue
        data = importlib.import_module(f"adapters.{agent}.otel").map(attrs)
        if data is not None:
            records.append({**data, "projectId": resolve_project(data["nativeSession"]) or GLOBAL_PROJECT_ID})
    return records


class Handler(BaseHTTPRequestHandler):
    def _reply(self, status: int, body: dict) -> None:
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self) -> None:
        self._reply(200, {"status": "ok"}) if self.path == "/health" else self._reply(404, {})

    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length") or 0)
        if self.path != "/v1/logs" or length > MAX_BODY_BYTES:
            self._reply(404 if self.path != "/v1/logs" else 413, {})
            return
        try:
            enqueue_usage(usage_records(json.loads(self.rfile.read(length) or b"{}")))
        except Exception as error:  # telemetry must never break an agent's exporter loop
            sys.stderr.write(f"usage receiver: {type(error).__name__}\n")
        self._reply(200, {})

    def log_message(self, *args: Any) -> None:
        pass


def is_running() -> bool:
    try:
        with urlopen(f"http://{HOST}:{port()}/health", timeout=1) as response:
            return response.status == 200
    except OSError:
        return False


def ensure() -> None:
    if is_running():
        return
    pythonw = Path(sys.executable).with_name("pythonw.exe")
    flags = sum(int(getattr(subprocess, n, 0)) for n in ("CREATE_NO_WINDOW", "DETACHED_PROCESS", "CREATE_NEW_PROCESS_GROUP"))
    subprocess.Popen([str(pythonw if pythonw.exists() else sys.executable), "-m", "core.usage.receiver", "serve"],
                     cwd=str(ROOT), env={**os.environ, "PYTHONPATH": str(ROOT)}, stdin=subprocess.DEVNULL,
                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, close_fds=True, creationflags=flags)


class _ExclusiveServer(HTTPServer):
    """Single instance per port. HTTPServer's SO_REUSEADDR lets a second process bind the same port on Windows."""

    allow_reuse_address = False

    def server_bind(self) -> None:
        if hasattr(socket, "SO_EXCLUSIVEADDRUSE"):
            self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_EXCLUSIVEADDRUSE, 1)
        super().server_bind()


def make_server(listen_port: int) -> HTTPServer:
    return _ExclusiveServer((HOST, listen_port), Handler)


def serve() -> None:
    try:
        server = make_server(port())
    except OSError:
        return
    server.serve_forever()


if __name__ == "__main__":
    ensure() if sys.argv[1:2] == ["ensure"] else serve()
