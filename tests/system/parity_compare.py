"""State capture, masking and structural diff for the hook parity run.

Values are never reported: a mismatch names the file, a JSON path and the types/lengths on each side. Volatile values
are masked before comparison (uuid7, random 32-hex trace ids, ISO timestamps, *_ns/pid/create_time/durationMs and
*_at/*At keys, sandbox root paths); deterministic ids (uuid5, sha256/HMAC fingerprints) stay unmasked on purpose.
"""
from __future__ import annotations

import json
import os
import re
import sqlite3
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any

UUID7 = re.compile(r"\b[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\b")
HEX32 = re.compile(r"\b[0-9a-f]{32}\b")
ISO_TIME = re.compile(r"\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:[.,]\d+)?(?:Z|[+-]\d{2}:?\d{2})?")
NUMBER = re.compile(r"(?<![A-Za-z_])-?\d+(?:\.\d+)?")
VOLATILE_SNAKE_KEY = re.compile(r"(?i)(_ns|pid|create_time|duration_?ms|_ms|_at|timestamp|^ts|^time)$")
VOLATILE_CAMEL_KEY = re.compile(r"[a-z]At$")
EPOCH_FLOOR = 1_000_000_000  # seconds/ms/ns since 2001: clock values, never counters here
SAFE_LABEL = re.compile(r"^[A-Za-z][A-Za-z0-9._:-]{0,60}$")
LOG_SUFFIXES = (".log",)
STATE_SKIP_SUFFIXES = (".sqlite3", ".sqlite3-wal", ".sqlite3-shm", ".sqlite3-journal")


@dataclass(frozen=True)
class Mismatch:
    path: str
    detail: str


# ---------------------------------------------------------------- masking

def mask(value: Any, root: str, key: str = "") -> Any:
    if key and is_volatile_key(key) and value not in (None, "", 0):
        return f"<{type(value).__name__}>"
    if isinstance(value, dict):
        return {k: mask(v, root, str(k)) for k, v in value.items()}
    if isinstance(value, list):
        return [mask(item, root) for item in value]
    if isinstance(value, str):
        return mask_text(value, root)
    if isinstance(value, (int, float)) and not isinstance(value, bool) and value >= EPOCH_FLOOR:
        return "<epoch>"
    return value


def is_volatile_key(key: str) -> bool:
    return bool(VOLATILE_SNAKE_KEY.search(key) or VOLATILE_CAMEL_KEY.search(key))


def mask_text(text: str, root: str) -> str:
    for form in root_forms(root):
        text = text.replace(form, "<ROOT>")
    text = UUID7.sub("<uuid7>", text)
    text = HEX32.sub("<hex32>", text)
    return ISO_TIME.sub("<time>", text)


def root_forms(root: str) -> list[str]:
    forward = root.replace("\\", "/")
    return sorted({root, forward, root.lower(), forward.lower(), root.replace("\\", "\\\\")}, key=len, reverse=True)


# ---------------------------------------------------------------- structural diff (types/lengths only)

def describe(value: Any) -> str:
    if isinstance(value, (str, list, dict)):
        return f"{type(value).__name__}(len {len(value)})"
    return type(value).__name__


def deep_diff(a: Any, b: Any, path: str = "$") -> list[Mismatch]:
    if isinstance(a, dict) and isinstance(b, dict):
        found = []
        for key in sorted(set(a) | set(b), key=str):
            child = f"{path}.{key}"
            if key not in a:
                found.append(Mismatch(child, f"only in B: {describe(b[key])}"))
            elif key not in b:
                found.append(Mismatch(child, f"only in A: {describe(a[key])}"))
            else:
                found.extend(deep_diff(a[key], b[key], child))
        return found
    if isinstance(a, list) and isinstance(b, list):
        if len(a) != len(b):
            return [Mismatch(path, f"list length A={len(a)} B={len(b)}")]
        return [m for index, (x, y) in enumerate(zip(a, b)) for m in deep_diff(x, y, f"{path}[{index}]")]
    if a != b or type(a) is not type(b):
        return [Mismatch(path, f"A={describe(a)} B={describe(b)}")]
    return []


# ---------------------------------------------------------------- state capture

def baseline_listing(root: Path) -> dict[str, tuple[int, int]]:
    return {rel: (stat.st_size, stat.st_mtime_ns) for rel, stat in walk_files(root)}


def walk_files(root: Path):
    for directory, dirs, files in os.walk(root):
        dirs[:] = [d for d in dirs if d not in (".venv", "__pycache__", "bin")]
        for name in files:
            full = Path(directory) / name
            yield full.relative_to(root).as_posix(), full.stat()


def is_log(rel: str) -> bool:
    return rel.startswith("var/logs/") or rel.endswith(LOG_SUFFIXES)


# Mental model: every new/changed non-log file under the sandbox → masked parsed content; spool via SQL.
def capture_state(root: Path, baseline: dict[str, tuple[int, int]]) -> dict[str, Any]:
    state: dict[str, Any] = {}
    for rel, stat in walk_files(root):
        if baseline.get(rel) == (stat.st_size, stat.st_mtime_ns) or is_log(rel) or rel == "var/launched.txt":
            continue
        if rel.endswith(STATE_SKIP_SUFFIXES):
            continue
        state[rel] = file_content(root / rel, str(root))
    spool = root / "tools" / "observability-client" / "var" / "spool.sqlite3"
    if spool.exists():
        state["<spool>"] = spool_content(spool, str(root))
    return state


def file_content(path: Path, root: str) -> Any:
    raw = path.read_bytes()
    if path.suffix == ".lock" or path.name == "hmac.key":
        return {"<bytes>": len(raw)} if path.suffix == ".lock" else {"<sha-prefix>": raw[:4].hex()}
    text = raw.decode("utf-8", errors="replace")
    try:
        if path.suffix == ".jsonl":
            return [mask(json.loads(line), root) for line in text.splitlines() if line.strip()]
        return mask(json.loads(text), root)
    except ValueError:
        return {"<text-lines>": [mask_text(line, root) for line in text.splitlines()]}


def spool_content(spool: Path, root: str) -> dict[str, Any]:
    conn = sqlite3.connect(f"file:{spool}?mode=ro", uri=True, timeout=5)
    try:
        payloads = [row[0] for table in ("queue", "dead_letter")
                    for row in conn.execute(f"SELECT payload FROM {table}")]
        conn.row_factory = sqlite3.Row
        context = [dict(row) for row in conn.execute("SELECT * FROM context ORDER BY native_session")]
    finally:
        conn.close()
    operations: dict[str, list[str]] = {}
    for payload in payloads:
        op = json.loads(payload)
        masked = json.dumps(mask(op.get("data"), root), sort_keys=True, ensure_ascii=False)
        operations.setdefault(str(op.get("kind")), []).append(masked)
    return {"operations": {kind: sorted(items) for kind, items in operations.items()},
            "context": [mask(row, root) for row in context]}


def compare_states(a: dict[str, Any], b: dict[str, Any]) -> list[Mismatch]:
    found = []
    for rel in sorted(set(a) | set(b)):
        if rel not in a or rel not in b:
            found.append(Mismatch(rel, f"file only in {'B' if rel not in a else 'A'}"))
        elif rel == "<spool>":
            found.extend(compare_spool(a[rel], b[rel]))
        else:
            found.extend(Mismatch(f"{rel} {m.path}", m.detail) for m in deep_diff(a[rel], b[rel]))
    return found


def compare_spool(a: dict[str, Any], b: dict[str, Any]) -> list[Mismatch]:
    found = []
    ops_a, ops_b = a["operations"], b["operations"]
    for kind in sorted(set(ops_a) | set(ops_b)):
        items_a, items_b = ops_a.get(kind, []), ops_b.get(kind, [])
        label = kind if SAFE_LABEL.match(kind) else "<kind>"
        if len(items_a) != len(items_b):
            found.append(Mismatch(f"spool.queue[{label}]", f"operation count A={len(items_a)} B={len(items_b)}"))
            continue
        unmatched_a = list((Counter(items_a) - Counter(items_b)).elements())
        unmatched_b = list((Counter(items_b) - Counter(items_a)).elements())
        for x, y in zip(unmatched_a, unmatched_b):
            found.extend(Mismatch(f"spool.queue[{label}] {m.path}", m.detail) for m in deep_diff(json.loads(x), json.loads(y)))
    found.extend(Mismatch(f"spool.context {m.path}", m.detail) for m in deep_diff(a["context"], b["context"]))
    return found


# ---------------------------------------------------------------- logs (delta per step)

class LogCursor:
    """Remembers how many lines of each log file were already compared."""

    def __init__(self, root: Path):
        self.root = root
        self.offsets: dict[str, int] = {}

    def new_lines(self) -> dict[str, list[str]]:
        fresh = {}
        for rel, _stat in walk_files(self.root):
            if not is_log(rel):
                continue
            lines = (self.root / rel).read_text(encoding="utf-8", errors="replace").splitlines()
            fresh[rel] = lines[self.offsets.get(rel, 0):]
            self.offsets[rel] = len(lines)
        return fresh


def lifecycle_shape(line: str, root: str) -> Any:
    try:
        record = json.loads(line)
    except ValueError:
        return {"<text>": NUMBER.sub("N", ISO_TIME.sub("", mask_text(line, root))).strip()}
    if not isinstance(record, dict) or "operation" not in record:
        return mask(record, root)
    details = record.get("details") if isinstance(record.get("details"), dict) else {}
    return {"component": record.get("component"), "operation": record.get("operation"),
            "outcome": record.get("outcome"), "detailKeys": sorted(details), "details": mask(details, root)}


def order_consumer_runs(shapes: list[Any]) -> tuple[list[Any], bool]:
    """Sort each contiguous run of consumer.* records by consumer name; report whether the order changed."""
    ordered, run, changed = [], [], False

    def flush_run() -> None:
        nonlocal changed
        sorted_run = sorted(run, key=lambda s: str(s["details"].get("consumer")))
        changed = changed or sorted_run != run
        ordered.extend(sorted_run)
        run.clear()

    for shape in shapes:
        if isinstance(shape, dict) and str(shape.get("operation", "")).startswith("consumer."):
            run.append(shape)
            continue
        flush_run()
        ordered.append(shape)
    flush_run()
    return ordered, changed


def compare_log_deltas(a: dict[str, list[str]], b: dict[str, list[str]], root_a: str, root_b: str) -> tuple[list[Mismatch], int]:
    found, reordered = [], 0
    for rel in sorted(set(a) | set(b)):
        shapes_a, moved_a = order_consumer_runs([lifecycle_shape(l, root_a) for l in a.get(rel, [])])
        shapes_b, moved_b = order_consumer_runs([lifecycle_shape(l, root_b) for l in b.get(rel, [])])
        reordered += int(moved_a != moved_b)
        if len(shapes_a) != len(shapes_b):
            found.append(Mismatch(rel, f"new records A={len(shapes_a)} B={len(shapes_b)}: "
                                       f"A-only={operation_names(shapes_a, shapes_b)} B-only={operation_names(shapes_b, shapes_a)}"))
            continue
        for index, (x, y) in enumerate(zip(shapes_a, shapes_b)):
            found.extend(Mismatch(f"{rel}#{index} {operation_label(x)} {m.path}", m.detail) for m in deep_diff(x, y))
    return found, reordered


def operation_label(shape: Any) -> str:
    if isinstance(shape, dict) and "operation" in shape:
        parts = [str(shape.get(k)) for k in ("component", "operation", "outcome")]
        return "/".join(p if SAFE_LABEL.match(p) else "<?>" for p in parts)
    return "<line>"


def operation_names(mine: list[Any], theirs: list[Any]) -> list[str]:
    extra = Counter(operation_label(s) for s in mine) - Counter(operation_label(s) for s in theirs)
    return sorted(extra.elements())[:6]


def launched_counts(path: Path) -> Counter:
    if not path.exists():
        return Counter()
    return Counter(line.strip() for line in path.read_text(encoding="utf-8-sig", errors="replace").splitlines() if line.strip())
