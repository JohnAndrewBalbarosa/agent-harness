from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import sys
import time
import uuid
from contextlib import contextmanager
from pathlib import Path
from typing import Any, BinaryIO, Iterator

from common import DATA_ROOT, SESSION_ROOT, load_observability_module, log_event, read_json


SENSITIVE_KEY = re.compile(r"(token|secret|password|cookie|authorization|private.?key)", re.I)
LARGE_KEY = re.compile(r"(surface|screen|buffer|output|content|raw)", re.I)


def sanitize(value: Any, key: str = "", depth: int = 0) -> Any:
    if depth > 5:
        return "<depth-limited>"
    if SENSITIVE_KEY.search(key):
        return "<redacted>"
    if isinstance(value, dict):
        return {str(k)[:120]: sanitize(v, str(k), depth + 1) for k, v in list(value.items())[:80]}
    if isinstance(value, list):
        return [sanitize(item, key, depth + 1) for item in value[:80]]
    if isinstance(value, str):
        limit = 500 if LARGE_KEY.search(key) else 2000
        return value[:limit] + ("<truncated>" if len(value) > limit else "")
    return value


def find_pane(value: Any) -> str:
    if isinstance(value, dict):
        for key in ("pane_id", "paneId"):
            if value.get(key):
                return str(value[key])
        for child in value.values():
            found = find_pane(child)
            if found:
                return found
    elif isinstance(value, list):
        for child in value:
            found = find_pane(child)
            if found:
                return found
    return ""


def binding_for_pane(pane_id: str) -> dict[str, Any]:
    if not pane_id or not SESSION_ROOT.exists():
        return {}
    candidates = []
    for path in SESSION_ROOT.glob("*.json"):
        value = read_json(path)
        if value.get("pane_id") == pane_id and value.get("project_id"):
            candidates.append(value)
    return max(candidates, key=lambda item: float(item.get("updated_at", 0)), default={})


def read_line(pipe: BinaryIO) -> bytes:
    data = bytearray()
    while True:
        chunk = pipe.read(1)
        if not chunk:
            raise EOFError("Herdr socket closed")
        if chunk == b"\n":
            return bytes(data)
        data.extend(chunk)
        if len(data) > 1024 * 1024:
            raise ValueError("Herdr event exceeded 1 MiB")


@contextmanager
def event_stream(socket_path: str) -> Iterator[tuple[BinaryIO, BinaryIO]]:
    if os.name == "nt":
        flags = int(getattr(subprocess, "CREATE_NO_WINDOW", 0))
        process = subprocess.Popen(
            ["node", str(Path(__file__).with_name("socket-stream.mjs")), socket_path],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            creationflags=flags,
        )
        try:
            assert process.stdin is not None and process.stdout is not None
            yield process.stdin, process.stdout
        finally:
            if process.poll() is None:
                process.terminate()
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            if process.returncode:
                error = (process.stderr.read(80) if process.stderr else b"").decode("ascii", errors="replace").strip()
                log_event("subscriber.transport.exited", "failed", exitCode=process.returncode, error=error)
            for stream in (process.stdin, process.stdout, process.stderr):
                if stream:
                    stream.close()
    else:
        import socket

        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
            connection.connect(socket_path)
            with connection.makefile("rwb", buffering=0) as pipe:
                yield pipe, pipe


def acquire_lock(path: Path) -> BinaryIO | None:
    handle = path.open("a+b")
    try:
        handle.seek(0)
        if os.name == "nt":
            import msvcrt

            msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl

            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        handle.close()
        return None
    return handle


def record_event(value: dict[str, Any], sequence: int) -> None:
    event = value.get("event") if isinstance(value.get("event"), dict) else value
    event_name = str(event.get("event") or event.get("type") or value.get("type") or "herdr.event")[:160]
    pane_id = find_pane(event)
    binding = binding_for_pane(pane_id)
    bounded = sanitize(event)
    log_event("herdr.event", event_name=event_name, pane=pane_id or None, sequence=sequence, payload=bounded)
    if not binding:
        return
    try:
        obs = load_observability_module()
        obs.enqueue([obs.operation("herdr-event.record", {
            "herdrEventId": str(uuid.uuid4()),
            "projectId": binding["project_id"],
            "nativeSession": binding.get("session_id"),
            "paneId": pane_id or None,
            "eventName": event_name,
            "sequenceNo": sequence,
            "payload": bounded if isinstance(bounded, dict) else {"value": bounded},
        })], flush_now=False)
    except Exception as exc:
        log_event("herdr.event.persist", "failed", event_name=event_name, error=type(exc).__name__)


def main() -> int:
    socket_path = os.environ.get("HERDR_SOCKET_PATH", "").strip()
    if not socket_path:
        return 2
    DATA_ROOT.mkdir(parents=True, exist_ok=True)
    identity = hashlib.sha256(socket_path.encode("utf-8")).hexdigest()[:16]
    pid_path = DATA_ROOT / f"subscriber-{identity}.pid"
    lock = acquire_lock(DATA_ROOT / f"subscriber-{identity}.lock")
    if lock is None:
        return 0
    pid_path.write_text(str(os.getpid()), encoding="ascii")
    sequence = 0
    delay = 1.0
    log_event("subscriber.started", socket_hash=identity, pid=os.getpid())
    try:
        while True:
            try:
                with event_stream(socket_path) as (writer, reader):
                    request = {
                        "id": f"shared-{uuid.uuid4()}",
                        "method": "events.subscribe",
                        "params": {"subscriptions": [
                            {"type": name} for name in (
                                "workspace.created", "workspace.closed",
                                "tab.created", "tab.closed",
                                "pane.created", "pane.exited", "pane.closed",
                                "pane.agent_detected",
                            )
                        ]},
                    }
                    writer.write((json.dumps(request, separators=(",", ":")) + "\n").encode("utf-8"))
                    writer.flush()
                    while True:
                        line = read_line(reader)
                        value = json.loads(line.decode("utf-8", errors="replace"))
                        if not isinstance(value, dict):
                            continue
                        if value.get("id") == request["id"]:
                            if value.get("error"):
                                raise ValueError("Herdr subscription rejected")
                            log_event("subscriber.connected", socket_hash=identity)
                            continue
                        delay = 1.0
                        sequence += 1
                        record_event(value, sequence)
            except (OSError, EOFError, ValueError, json.JSONDecodeError) as exc:
                log_event("subscriber.reconnect", "failed", error=type(exc).__name__, retry_seconds=delay)
                time.sleep(delay)
                delay = min(30.0, delay * 2)
    finally:
        pid_path.unlink(missing_ok=True)
        lock.close()
        log_event("subscriber.stopped", pid=os.getpid())


if __name__ == "__main__":
    raise SystemExit(main())
