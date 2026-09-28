from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import os
import re
import secrets
import sqlite3
import subprocess
import sys
import time
import tomllib
import urllib.error
import urllib.request
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent
VAR = ROOT / "var"
QUEUE = VAR / "spool.sqlite3"
KEY = VAR / "hmac.key"
ENDPOINT = os.environ.get("OBS_ENDPOINT", "http://127.0.0.1:4319")
MAX_QUEUED_OPERATIONS = max(100, int(os.environ.get("OBS_MAX_QUEUED_OPERATIONS", "10000")))
MAX_DEAD_LETTER_OPERATIONS = max(100, int(os.environ.get("OBS_MAX_DEAD_LETTER_OPERATIONS", "1000")))
HTTP_TIMEOUT_SECONDS = max(2.5, min(60.0, float(os.environ.get("OBS_HTTP_TIMEOUT_SECONDS", "15"))))
SECRET_PATTERNS = re.compile(r"(?i)(password|passwd|secret|token|api[_-]?key|cookie|authorization|private[_-]?key)")
SECRET_ASSIGNMENT = re.compile(r"(?i)(password|passwd|secret|token|api[_-]?key|cookie|authorization|private[_-]?key)(\s*[:=]\s*)\S+")
BEARER_VALUE = re.compile(r"(?i)Bearer\s+\S+")
SOURCE_LOCATOR = re.compile(r"(?P<locator>(?:file:///)?[^\s]+\.(?:dart|js|jsx|mjs|ts|tsx|py|java|kt|swift|cs|go|rs):\d+(?::\d+)?)")
NON_CODE_SUFFIXES = {
    ".md", ".mdx", ".rst", ".txt", ".pdf", ".png", ".jpg", ".jpeg", ".gif", ".webp",
    ".svg", ".ico", ".mp3", ".mp4", ".mov", ".avi", ".csv",
}


def now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def uuid7() -> str:
    if hasattr(uuid, "uuid7"):
        return str(uuid.uuid7())
    ms = int(time.time() * 1000)
    raw = bytearray(ms.to_bytes(6, "big") + secrets.token_bytes(10))
    raw[6] = (raw[6] & 0x0F) | 0x70
    raw[8] = (raw[8] & 0x3F) | 0x80
    return str(uuid.UUID(bytes=bytes(raw)))


def secret_key() -> bytes:
    VAR.mkdir(parents=True, exist_ok=True)
    if not KEY.exists():
        KEY.write_bytes(secrets.token_bytes(32))
        try:
            os.chmod(KEY, 0o600)
        except OSError:
            pass
    return KEY.read_bytes()


def digest(value: str, keyed: bool = False) -> str:
    data = value.encode("utf-8", "replace")
    return (hmac.new(secret_key(), data, hashlib.sha256) if keyed else hashlib.sha256(data)).hexdigest()


def redact(value: Any) -> Any:
    if isinstance(value, dict):
        return {str(k): "[REDACTED]" if SECRET_PATTERNS.search(str(k)) else redact(v) for k, v in value.items()}
    if isinstance(value, list):
        return [redact(v) for v in value[:100]]
    if isinstance(value, str):
        bounded = value[:1200]
        bounded = SECRET_ASSIGNMENT.sub(lambda match: f"{match.group(1)}{match.group(2)}[REDACTED]", bounded)
        return BEARER_VALUE.sub("Bearer [REDACTED]", bounded)
    return value


def structured_event_from_line(line: str) -> dict[str, Any] | None:
    """Extract one structured event without returning the surrounding raw log line."""
    decoder = json.JSONDecoder()
    offset = line.find("{")
    while offset >= 0:
        try:
            value, _ = decoder.raw_decode(line[offset:])
            if isinstance(value, dict) and isinstance(value.get("event"), str):
                return value
        except json.JSONDecodeError:
            pass
        offset = line.find("{", offset + 1)
    return None


def source_locator(value: dict[str, Any]) -> str:
    for field in ("diagnostics", "stack", "context"):
        candidate = value.get(field)
        if isinstance(candidate, str):
            match = SOURCE_LOCATOR.search(candidate)
            if match:
                return match.group("locator")[:500]
    return ""


def severity_number(value: Any) -> int:
    if isinstance(value, int):
        return max(1, min(5, value))
    return {"debug": 1, "info": 1, "warning": 2, "warn": 2, "error": 4, "fatal": 5}.get(str(value).lower(), 1)


def outcome_number(value: Any) -> int | None:
    return {"success": 1, "succeeded": 1, "completed": 1, "stopped": 2, "cancelled": 2, "failed": 3, "failure": 3}.get(str(value).lower())


def observed_time(value: Any) -> str:
    if isinstance(value, str):
        try:
            parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
            if parsed.tzinfo is None:
                parsed = parsed.replace(tzinfo=timezone.utc)
            return parsed.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")
        except ValueError:
            pass
    return now()


def runtime_summary(value: dict[str, Any]) -> str:
    details = value.get("details") if isinstance(value.get("details"), dict) else {}
    for candidate in (value.get("exception"), value.get("message"), details.get("message"), value.get("event")):
        if candidate is not None and str(candidate).strip():
            return str(redact(str(candidate)))[:500]
    return "runtime.event"


def bounded_runtime_details(value: dict[str, Any]) -> dict[str, Any]:
    details = value.get("details")
    if not isinstance(details, dict):
        return {}
    bounded: dict[str, Any] = {}
    for key, item in list(details.items())[:20]:
        safe_key = str(key)[:80]
        if isinstance(item, (str, int, float, bool)) or item is None:
            bounded[safe_key] = item[:300] if isinstance(item, str) else item
    return redact(bounded)


def db() -> sqlite3.Connection:
    VAR.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(QUEUE)
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("CREATE TABLE IF NOT EXISTS queue(operation_id TEXT PRIMARY KEY,payload TEXT NOT NULL,created_at TEXT NOT NULL,attempts INTEGER NOT NULL DEFAULT 0,last_error TEXT)")
    conn.execute("CREATE TABLE IF NOT EXISTS dead_letter(operation_id TEXT PRIMARY KEY,payload TEXT NOT NULL,created_at TEXT NOT NULL,failed_at TEXT NOT NULL,attempts INTEGER NOT NULL,last_error TEXT NOT NULL)")
    conn.execute("CREATE TABLE IF NOT EXISTS context(native_session TEXT PRIMARY KEY,project_id TEXT NOT NULL,session_id TEXT NOT NULL,prompt_id TEXT,execution_id TEXT,trace_id TEXT,last_prompt_id TEXT,native_turn TEXT,prompt_ordinal INTEGER NOT NULL DEFAULT 0,sequence_no INTEGER NOT NULL DEFAULT 0,cwd TEXT NOT NULL,updated_at TEXT NOT NULL)")
    conn.execute("""CREATE TABLE IF NOT EXISTS prompt_change_assessments(
        assessment_id TEXT PRIMARY KEY,project_id TEXT NOT NULL,prompt_id TEXT NOT NULL,
        delivery_stream TEXT NOT NULL,change_kind TEXT NOT NULL,declared_class TEXT NOT NULL,
        effective_class TEXT NOT NULL,classification_reason TEXT NOT NULL DEFAULT '',
        estimated_lines INTEGER NOT NULL DEFAULT 0,estimated_files INTEGER NOT NULL DEFAULT 0,
        actual_lines_added INTEGER NOT NULL DEFAULT 0,actual_lines_deleted INTEGER NOT NULL DEFAULT 0,
        actual_files INTEGER NOT NULL DEFAULT 0,risk_flags TEXT NOT NULL DEFAULT '[]',
        source_urls TEXT NOT NULL DEFAULT '[]',input_tokens INTEGER,output_tokens INTEGER,
        cached_input_tokens INTEGER,reasoning_tokens INTEGER,status TEXT NOT NULL DEFAULT 'planned',
        increments_counter INTEGER NOT NULL DEFAULT 0,package_id TEXT,created_at TEXT NOT NULL,
        completed_at TEXT,UNIQUE(project_id,prompt_id,delivery_stream))""")
    conn.execute("""CREATE TABLE IF NOT EXISTS change_packages(
        package_id TEXT PRIMARY KEY,project_id TEXT NOT NULL,delivery_stream TEXT NOT NULL,
        delivery_mode TEXT NOT NULL,commit_sha TEXT,pr_url TEXT,prompt_count INTEGER NOT NULL,
        created_at TEXT NOT NULL)""")
    conn.execute("CREATE INDEX IF NOT EXISTS change_assessments_counter ON prompt_change_assessments(project_id,delivery_stream,increments_counter,package_id)")
    columns = {row[1] for row in conn.execute("PRAGMA table_info(context)")}
    if "prompt_ordinal" not in columns:
        conn.execute("ALTER TABLE context ADD COLUMN prompt_ordinal INTEGER NOT NULL DEFAULT 0")
    if "last_prompt_id" not in columns:
        conn.execute("ALTER TABLE context ADD COLUMN last_prompt_id TEXT")
    if "native_turn" not in columns:
        conn.execute("ALTER TABLE context ADD COLUMN native_turn TEXT")
    if "wait_count" not in columns:
        conn.execute("ALTER TABLE context ADD COLUMN wait_count INTEGER NOT NULL DEFAULT 0")
    if "background_count" not in columns:
        conn.execute("ALTER TABLE context ADD COLUMN background_count INTEGER NOT NULL DEFAULT 0")
    if "behavior_audit_enabled" not in columns:
        conn.execute("ALTER TABLE context ADD COLUMN behavior_audit_enabled INTEGER NOT NULL DEFAULT 1")
    conn.commit()
    return conn


def project_config(start: Path | None = None) -> tuple[Path, dict[str, Any]] | None:
    current = (start or Path.cwd()).resolve()
    for directory in [current, *current.parents]:
        path = directory / "observability.project.toml"
        if path.exists():
            with path.open("rb") as handle:
                return path, tomllib.load(handle)
    return None


def operation(kind: str, data: dict[str, Any], occurred_at: str | None = None) -> dict[str, Any]:
    safe_data = redact(data)
    # These fields are explicitly configured as lossless records. They are
    # copied from hook/structured JSON input without AI transformation.
    raw_fields = {
        "prompt.begin": ("rawPrompt",),
        "prompt.finish": ("rawResult",),
        "event.emit": ("rawPayload",),
    }
    for field in raw_fields.get(kind, ()):
        if field in data:
            safe_data[field] = data[field]
    return {"operationId": uuid7(), "kind": kind, "occurredAt": occurred_at or now(), "data": safe_data}


def enqueue(ops: list[dict[str, Any]], *, flush_now: bool = True) -> None:
    conn = db()
    try:
        with conn:
            for op in ops:
                conn.execute("INSERT OR IGNORE INTO queue(operation_id,payload,created_at) VALUES(?,?,?)", (op["operationId"], json.dumps(op,separators=(",",":")), now()))
            conn.execute(
                "DELETE FROM queue WHERE operation_id IN (SELECT operation_id FROM queue ORDER BY created_at DESC,operation_id DESC LIMIT -1 OFFSET ?)",
                (MAX_QUEUED_OPERATIONS,),
            )
        if flush_now:
            flush(conn)
    finally:
        conn.close()


class HubDeliveryError(RuntimeError):
    def __init__(self, status: int, message: str):
        super().__init__(f"hub returned HTTP {status}: {message[:240]}")
        self.status = status


def post(ops: list[dict[str, Any]]) -> None:
    wire_ops = []
    for operation_value in ops:
        wire_operation = dict(operation_value)
        wire_operation["occurredAt"] = observed_time(wire_operation.get("occurredAt"))
        wire_ops.append(wire_operation)
    body = json.dumps({"operations": wire_ops}, separators=(",", ":")).encode()
    request = urllib.request.Request(f"{ENDPOINT}/v1/operations", data=body, headers={"Content-Type":"application/json"}, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=HTTP_TIMEOUT_SECONDS) as response:
            if response.status >= 300:
                raise HubDeliveryError(response.status, "request rejected")
    except urllib.error.HTTPError as exc:
        reason = "request rejected"
        try:
            parsed = json.loads(exc.read(2048).decode("utf-8", "replace"))
            if isinstance(parsed, dict) and parsed.get("error"):
                reason = str(redact(parsed["error"]))[:240]
        except Exception:
            pass
        raise HubDeliveryError(exc.code, reason) from None


def _move_to_dead_letter(conn: sqlite3.Connection, row: sqlite3.Row, error: Exception) -> None:
    message = str(error)[:300]
    conn.execute(
        "INSERT OR REPLACE INTO dead_letter(operation_id,payload,created_at,failed_at,attempts,last_error) VALUES(?,?,?,?,?,?)",
        (row["operation_id"], row["payload"], row["created_at"], now(), int(row["attempts"]) + 1, message),
    )
    conn.execute("DELETE FROM queue WHERE operation_id=?", (row["operation_id"],))
    conn.execute(
        "DELETE FROM dead_letter WHERE operation_id IN (SELECT operation_id FROM dead_letter ORDER BY failed_at DESC,operation_id DESC LIMIT -1 OFFSET ?)",
        (MAX_DEAD_LETTER_OPERATIONS,),
    )


def _deliver_with_isolation(conn: sqlite3.Connection, rows: list[sqlite3.Row]) -> int:
    """Deliver in order and isolate permanent HTTP 400 poison operations."""
    if not rows:
        return 0
    try:
        post([json.loads(row["payload"]) for row in rows])
    except HubDeliveryError as exc:
        if exc.status not in (400, 413):
            raise
        if len(rows) == 1:
            with conn:
                _move_to_dead_letter(conn, rows[0], exc)
            return 0
        midpoint = len(rows) // 2
        return _deliver_with_isolation(conn, rows[:midpoint]) + _deliver_with_isolation(conn, rows[midpoint:])
    except (ConnectionAbortedError, urllib.error.URLError) as exc:
        if len(rows) == 1:
            if int(rows[0]["attempts"]) >= 4:
                with conn:
                    _move_to_dead_letter(conn, rows[0], exc)
                return 0
            raise
        midpoint = len(rows) // 2
        return _deliver_with_isolation(conn, rows[:midpoint]) + _deliver_with_isolation(conn, rows[midpoint:])
    with conn:
        conn.executemany("DELETE FROM queue WHERE operation_id=?", [(row["operation_id"],) for row in rows])
    return len(rows)


def flush(conn: sqlite3.Connection | None = None) -> int:
    owns_connection = conn is None
    conn = conn or db()
    try:
        rows = conn.execute("SELECT operation_id,payload,created_at,attempts FROM queue ORDER BY created_at LIMIT 50").fetchall()
        if not rows:
            return 0
        try:
            return _deliver_with_isolation(conn, rows)
        except Exception as exc:
            message = str(exc)[:300]
            with conn:
                conn.executemany("UPDATE queue SET attempts=attempts+1,last_error=? WHERE operation_id=?", [(message,row["operation_id"]) for row in rows])
            return 0
    finally:
        if owns_connection:
            conn.close()


def git_value(root: Path, *args: str) -> str:
    try:
        return subprocess.check_output(["git", "-C", str(root), *args], text=True, encoding="utf-8", errors="replace", stderr=subprocess.DEVNULL).strip()
    except Exception:
        return ""


def git_run(root: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", "-C", str(root), *args],
        text=True,
        encoding="utf-8",
        errors="replace",
        capture_output=True,
        check=False,
    )


def require_git(root: Path, *args: str) -> str:
    result = git_run(root, *args)
    if result.returncode:
        detail = (result.stderr or result.stdout or "git command failed").strip()[:500]
        raise SystemExit(detail)
    return result.stdout.strip()


def is_github_remote(remote_url: str) -> bool:
    value = remote_url.strip().lower()
    return bool(
        re.match(r"^(?:https?|git)://github\.com/", value)
        or re.match(r"^ssh://(?:[^/@]+@)?github\.com(?::\d+)?/", value)
        or re.match(r"^[^@\s]+@github\.com:", value)
    )


def commit_touches_code(root: Path, commit: str) -> bool:
    files = require_git(root, "show", "--format=", "--name-only", "--no-renames", commit).splitlines()
    for relative in files:
        path = relative.strip()
        if not path:
            continue
        lowered = path.lower().replace("\\", "/")
        if lowered.startswith(("docs/", ".github/issue_template/")):
            continue
        if Path(lowered).suffix in NON_CODE_SUFFIXES:
            continue
        return True
    return False


CHANGE_CLASS_RANK = {"non-code": 0, "routine": 1, "moderate": 2, "substantial": 3, "high-risk": 4}


def classify_change(lines_changed: int, files_changed: int, high_risk: bool = False) -> str:
    if lines_changed < 0 or files_changed < 0:
        raise SystemExit("Changed-line and file counts cannot be negative.")
    if high_risk:
        return "high-risk"
    if lines_changed == 0 and files_changed == 0:
        return "non-code"
    if lines_changed <= 15 and files_changed <= 1:
        return "routine"
    if lines_changed <= 50 and files_changed <= 3:
        return "moderate"
    return "substantial"


def _change_identity(start: Path | None, prompt_id: str | None, delivery_stream: str | None) -> tuple[str, str, str]:
    found = project_config(start)
    if not found:
        raise SystemExit("No observability.project.toml found.")
    project_id = str(found[1]["project"]["id"])
    row = context_for(str((start or Path.cwd()).resolve()))
    resolved_prompt_id = (prompt_id or (row["prompt_id"] if row else None) or "").strip()
    if not resolved_prompt_id:
        raise SystemExit("No active prompt ID is available; pass --prompt-id explicitly.")
    stream = (delivery_stream or git_value(found[0].parent, "branch", "--show-current") or "project-default").strip()
    if not stream:
        raise SystemExit("Delivery stream cannot be empty.")
    return project_id, resolved_prompt_id, stream[:240]


def _source_urls(values: list[str]) -> list[str]:
    result: list[str] = []
    for value in values:
        url = value.strip()
        if not re.match(r"^https?://", url, re.IGNORECASE):
            raise SystemExit(f"Source URL must use http or https: {url[:120]}")
        if url not in result:
            result.append(url[:1200])
    return result[:50]


def plan_change(args: argparse.Namespace) -> dict[str, Any]:
    project_id, prompt_id, stream = _change_identity(Path.cwd(), args.prompt_id, args.delivery_stream)
    risk_flags = [item.strip()[:120] for item in args.risk if item.strip()][:20]
    computed = classify_change(args.estimated_lines, args.estimated_files, bool(risk_flags))
    declared = "non-code" if args.kind != "code" else args.change_class
    effective = max((declared, computed), key=lambda item: CHANGE_CLASS_RANK[item]) if args.kind == "code" else "non-code"
    assessment_id = str(uuid.uuid5(uuid.NAMESPACE_URL, f"{project_id}|{prompt_id}|{stream}"))
    conn = db()
    try:
        with conn:
            existing = conn.execute("SELECT status FROM prompt_change_assessments WHERE assessment_id=?", (assessment_id,)).fetchone()
            if existing and existing["status"] != "planned":
                raise SystemExit("This prompt change assessment is already finalized.")
            conn.execute("""INSERT INTO prompt_change_assessments(
                assessment_id,project_id,prompt_id,delivery_stream,change_kind,declared_class,effective_class,
                classification_reason,estimated_lines,estimated_files,risk_flags,source_urls,created_at)
                VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(assessment_id) DO UPDATE SET
                change_kind=excluded.change_kind,declared_class=excluded.declared_class,
                effective_class=excluded.effective_class,classification_reason=excluded.classification_reason,
                estimated_lines=excluded.estimated_lines,estimated_files=excluded.estimated_files,
                risk_flags=excluded.risk_flags,source_urls=excluded.source_urls""",
                (assessment_id,project_id,prompt_id,stream,args.kind,declared,effective,args.reason[:500],
                 args.estimated_lines,args.estimated_files,json.dumps(risk_flags),json.dumps(_source_urls(args.source_url)),now()))
    finally:
        conn.close()
    enqueue([operation("change-assessment.plan", {
        "assessmentId":assessment_id,"projectId":project_id,"promptId":prompt_id,
        "deliveryStream":stream,"changeKind":args.kind,"declaredClass":declared,
        "effectiveClass":effective,"classificationReason":args.reason[:500],
        "estimatedLines":args.estimated_lines,"estimatedFiles":args.estimated_files,
        "riskFlags":risk_flags,"sourceUrls":_source_urls(args.source_url),
    })])
    return {"assessmentId":assessment_id,"projectId":project_id,"promptId":prompt_id,
            "deliveryStream":stream,"changeKind":args.kind,"declaredClass":declared,
            "effectiveClass":effective,"counterIncremented":False}


def finish_change(args: argparse.Namespace) -> dict[str, Any]:
    project_id, prompt_id, stream = _change_identity(Path.cwd(), args.prompt_id, args.delivery_stream)
    conn = db()
    try:
        row = conn.execute("SELECT * FROM prompt_change_assessments WHERE project_id=? AND prompt_id=? AND delivery_stream=?",
                           (project_id,prompt_id,stream)).fetchone()
        if not row:
            raise SystemExit("No planned change assessment exists for this prompt and delivery stream.")
        already_finalized = row["status"] != "planned"
        if already_finalized and row["status"] != args.outcome:
            raise SystemExit("A finalized change assessment cannot change its outcome.")
        added, deleted, files = args.lines_added, args.lines_deleted, args.files_changed
        actual_class = classify_change(added + deleted, files, bool(json.loads(row["risk_flags"])))
        effective = max((row["effective_class"], actual_class), key=lambda item: CHANGE_CLASS_RANK[item])
        increment = int(row["increments_counter"]) if already_finalized else int(
            args.outcome == "success" and row["change_kind"] == "code" and files > 0
        )
        sources = list(dict.fromkeys([*json.loads(row["source_urls"]), *_source_urls(args.source_url)]))[:50]
        completed_at = now()
        with conn:
            conn.execute("""UPDATE prompt_change_assessments SET effective_class=?,actual_lines_added=?,
                actual_lines_deleted=?,actual_files=?,source_urls=?,input_tokens=?,output_tokens=?,
                cached_input_tokens=?,reasoning_tokens=?,status=?,increments_counter=?,completed_at=?
                WHERE assessment_id=?""",
                (effective,added,deleted,files,json.dumps(sources),args.input_tokens,args.output_tokens,
                 args.cached_input_tokens,args.reasoning_tokens,args.outcome,increment,completed_at,row["assessment_id"]))
            count = conn.execute("SELECT count(*) FROM prompt_change_assessments WHERE project_id=? AND delivery_stream=? AND increments_counter=1 AND package_id IS NULL",
                                 (project_id,stream)).fetchone()[0]
    finally:
        conn.close()
    enqueue([operation("change-assessment.finish", {
        "assessmentId":row["assessment_id"],"projectId":project_id,"promptId":prompt_id,
        "deliveryStream":stream,"effectiveClass":effective,"actualLinesAdded":added,
        "actualLinesDeleted":deleted,"actualFiles":files,"sourceUrls":sources,
        "usage":{"input":args.input_tokens,"output":args.output_tokens,
                 "cachedInput":args.cached_input_tokens,"reasoning":args.reasoning_tokens},
        "status":args.outcome,"incrementsCounter":bool(increment),"completedAt":completed_at,
    })])
    return _finish_result(row["assessment_id"], project_id, prompt_id, stream, effective, count, args.threshold, bool(increment) and not already_finalized)


def _finish_result(assessment_id: str, project_id: str, prompt_id: str, stream: str,
                   effective_class: str, count: int, threshold: int, incremented: bool) -> dict[str, Any]:
    reached = count >= threshold
    return {"assessmentId":assessment_id,"projectId":project_id,"promptId":prompt_id,
            "deliveryStream":stream,"effectiveClass":effective_class,"counterIncremented":incremented,
            "codeChangePromptCount":count,"threshold":threshold,"thresholdReached":reached,
            "directive":"PACKAGE_NOW" if reached else "CONTINUE",
            "policyResolution":"current-agent-must-apply-active-AGENTS.md-cascade",
            "mergeAuthorized":False}


def package_changes(args: argparse.Namespace) -> dict[str, Any]:
    if not args.confirm_policy_reviewed:
        raise SystemExit("Refusing to close the batch until the active AGENTS.md cascade has been reviewed.")
    project_id, prompt_id, stream = _change_identity(Path.cwd(), args.prompt_id, args.delivery_stream)
    conn = db()
    try:
        rows = conn.execute("SELECT assessment_id FROM prompt_change_assessments WHERE project_id=? AND delivery_stream=? AND increments_counter=1 AND package_id IS NULL ORDER BY created_at,assessment_id",
                            (project_id,stream)).fetchall()
        if not rows:
            raise SystemExit("No un-packaged code-changing prompts exist for this delivery stream.")
        package_id = uuid7()
        created_at = now()
        with conn:
            conn.execute("INSERT INTO change_packages(package_id,project_id,delivery_stream,delivery_mode,commit_sha,pr_url,prompt_count,created_at) VALUES(?,?,?,?,?,?,?,?)",
                         (package_id,project_id,stream,args.delivery_mode,args.commit_sha or None,args.pr_url or None,len(rows),created_at))
            conn.executemany("UPDATE prompt_change_assessments SET package_id=? WHERE assessment_id=?",
                             [(package_id,row["assessment_id"]) for row in rows])
    finally:
        conn.close()
    enqueue([operation("change-assessment.package", {
        "packageId":package_id,"projectId":project_id,"promptId":prompt_id,
        "deliveryStream":stream,"deliveryMode":args.delivery_mode,"commitSha":args.commit_sha or None,
        "prUrl":args.pr_url or None,"promptCount":len(rows),"createdAt":created_at,
    })])
    return {"packageId":package_id,"projectId":project_id,"deliveryStream":stream,
            "deliveryMode":args.delivery_mode,"promptCount":len(rows),"remainingCount":0,
            "mergeAuthorized":False}


def git_policy_status(start: Path | None = None, threshold: int = 5) -> dict[str, Any]:
    if threshold < 1:
        raise SystemExit("Threshold must be at least 1.")
    root_text = git_value(start or Path.cwd(), "rev-parse", "--show-toplevel")
    if not root_text:
        raise SystemExit("Current directory is not inside a Git worktree.")
    root = Path(root_text).resolve()
    branch = git_value(root, "branch", "--show-current")
    if not branch:
        raise SystemExit("Current HEAD is detached; check out a branch before applying the Git policy.")
    upstream = git_value(root, "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}")
    if not upstream:
        raise SystemExit("Current branch has no configured upstream.")
    remote_name = git_value(root, "config", "--get", f"branch.{branch}.remote")
    if not remote_name or remote_name == ".":
        raise SystemExit("Current branch is not tracking a remote repository.")
    remote_url = git_value(root, "remote", "get-url", remote_name)
    if not is_github_remote(remote_url):
        raise SystemExit("Configured upstream is not hosted on github.com.")
    counts = require_git(root, "rev-list", "--left-right", "--count", f"{upstream}...HEAD").split()
    if len(counts) != 2:
        raise SystemExit("Unable to determine ahead/behind commit counts.")
    behind, ahead = (int(value) for value in counts)
    commits = require_git(root, "rev-list", "--reverse", f"{upstream}..HEAD").splitlines() if ahead else []
    code_commits = [commit for commit in commits if commit_touches_code(root, commit)]
    found = project_config(root)
    project_id = found[1]["project"]["id"] if found else None
    clean = not bool(git_value(root, "status", "--porcelain=v1", "--untracked-files=all"))
    threshold_reached = len(code_commits) >= threshold
    safe_to_squash = ahead > 1 and behind == 0 and clean
    return {
        "schemaVersion": 1,
        "projectId": project_id,
        "counterSource": "git-upstream-range",
        "branch": branch,
        "remote": remote_name,
        "upstream": upstream,
        "head": require_git(root, "rev-parse", "HEAD"),
        "aheadCommits": ahead,
        "behindCommits": behind,
        "codeCommits": len(code_commits),
        "nonCodeCommits": ahead - len(code_commits),
        "threshold": threshold,
        "thresholdRemaining": max(0, threshold - len(code_commits)),
        "thresholdReached": threshold_reached,
        "pushRecommended": threshold_reached and ahead > 0 and behind == 0 and clean,
        "squashRecommended": threshold_reached and safe_to_squash,
        "workingTreeClean": clean,
        "safeToSquash": safe_to_squash,
    }


def squash_local_commits(
    message: str,
    start: Path | None = None,
    *,
    expected_head: str,
    all_local_commits_are_related: bool,
) -> dict[str, Any]:
    message = message.strip()
    if not message:
        raise SystemExit("A non-empty squash commit message is required.")
    if not all_local_commits_are_related:
        raise SystemExit("Refusing to squash without confirmation that every unpushed commit is related.")
    status = git_policy_status(start)
    root = Path(git_value(start or Path.cwd(), "rev-parse", "--show-toplevel")).resolve()
    if status["head"] != expected_head.strip():
        raise SystemExit("Refusing to squash: HEAD changed after the reviewed git check.")
    if not status["workingTreeClean"]:
        raise SystemExit("Refusing to squash: the worktree has staged, unstaged, or untracked changes.")
    if status["behindCommits"]:
        raise SystemExit("Refusing to squash: the branch is behind or diverged from its upstream.")
    if status["aheadCommits"] < 2:
        raise SystemExit("At least two unpushed commits are required to squash.")
    original_head = require_git(root, "rev-parse", "HEAD")
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    backup_ref = f"refs/codex-observability/pre-squash/{stamp}-{original_head[:12]}"
    require_git(root, "update-ref", backup_ref, original_head)
    try:
        require_git(root, "reset", "--soft", status["upstream"])
        require_git(root, "commit", "-m", message)
    except BaseException:
        git_run(root, "reset", "--soft", original_head)
        raise
    result = git_policy_status(root, status["threshold"])
    result.update({
        "squashedCommitCount": status["aheadCommits"],
        "previousHead": original_head,
        "currentHead": require_git(root, "rev-parse", "HEAD"),
        "backupRef": backup_ref,
    })
    return result


def record_git_policy_event(code: str, message: str, outcome: int, severity: int, attributes: dict[str, Any]) -> bool:
    """Best-effort event recording; Git safety operations must still work if the hub is unavailable."""
    try:
        row = context_for(str(Path.cwd()))
        if not row or not row["prompt_id"] or not row["execution_id"]:
            return False
        seq = int(row["sequence_no"]) + 1
        event_time = now()
        op = operation("event.emit", {
            "eventId": uuid7(),
            "executionId": row["execution_id"],
            "projectId": row["project_id"],
            "traceId": row["trace_id"],
            "sequenceNo": seq,
            "code": code,
            "category": 1,
            "severity": severity,
            "outcome": outcome,
            "component": "git-policy",
            "messageSummary": message,
            "attributes": attributes,
        }, event_time)
        conn = db()
        try:
            with conn:
                conn.execute(
                    "UPDATE context SET sequence_no=?,updated_at=? WHERE native_session=?",
                    (seq, now(), row["native_session"]),
                )
        finally:
            conn.close()
        enqueue([op])
        return True
    except Exception:
        print("Warning: unable to record the Git policy event; the Git operation result is unchanged.", file=sys.stderr)
        return False


def worktree_fingerprint(root: Path) -> str | None:
    """Hash tracked content changes plus bounded untracked-file content without retaining either."""
    status = git_value(root, "status", "--porcelain=v1", "--untracked-files=all")
    if not status:
        return None
    hasher = hashlib.sha256()
    hasher.update(status.encode("utf-8", "replace"))
    try:
        patch = subprocess.check_output(
            ["git", "-C", str(root), "diff", "--binary", "--no-ext-diff", "HEAD", "--"],
            stderr=subprocess.DEVNULL,
        )
        hasher.update(hashlib.sha256(patch).digest())
        untracked = git_value(root, "ls-files", "--others", "--exclude-standard").splitlines()
        for relative in sorted(untracked):
            candidate = (root / relative).resolve()
            try:
                candidate.relative_to(root.resolve())
                stat = candidate.stat()
                hasher.update(relative.encode("utf-8", "replace"))
                hasher.update(str(stat.st_size).encode("ascii"))
                if candidate.is_file() and stat.st_size <= 50 * 1024 * 1024:
                    with candidate.open("rb") as handle:
                        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                            hasher.update(chunk)
            except (OSError, ValueError):
                hasher.update(f"unreadable:{relative}".encode("utf-8", "replace"))
    except (OSError, subprocess.SubprocessError):
        pass
    return hasher.hexdigest()


def register_project(config_path: Path, config: dict[str, Any]) -> dict[str, Any]:
    root = config_path.parent
    project = config["project"]
    aliases = [{"kind": 2, "hash": digest(str(root).lower())}]
    remote = git_value(root, "remote", "get-url", "origin")
    if remote:
        aliases.append({"kind": 1, "hash": digest(remote.lower())})
    op = operation("project.register", {"projectId": project["id"], "displayName": project["name"], "slug": project["slug"], "aliases": aliases})
    enqueue([op])
    return project


def context_for(cwd: str | None = None) -> sqlite3.Row | None:
    conn = db()
    try:
        if cwd:
            resolved = Path(cwd).resolve()
            found = project_config(resolved)
            if found:
                project_id = found[1]["project"]["id"]
                return conn.execute("SELECT * FROM context WHERE project_id=? ORDER BY (execution_id IS NOT NULL) DESC,updated_at DESC LIMIT 1", (project_id,)).fetchone()
            return conn.execute("SELECT * FROM context WHERE cwd=? ORDER BY updated_at DESC LIMIT 1", (str(resolved),)).fetchone()
        return conn.execute("SELECT * FROM context ORDER BY updated_at DESC LIMIT 1").fetchone()
    finally:
        conn.close()


def current_or_fail() -> sqlite3.Row:
    row = context_for(str(Path.cwd()))
    if not row or not row["prompt_id"] or not row["execution_id"]:
        raise SystemExit("No active observed prompt for this project. Start a Codex prompt or use the hook first.")
    return row


def emit_event(args: argparse.Namespace) -> None:
    row = current_or_fail()
    seq = int(row["sequence_no"]) + 1
    event_id = uuid7()
    event_time = now()
    op = operation("event.emit", {"eventId":event_id,"executionId":row["execution_id"],"projectId":row["project_id"],"traceId":row["trace_id"],"sequenceNo":seq,"code":args.code,"category":args.category,"severity":args.severity,"outcome":args.outcome,"component":args.component,"messageSummary":args.message,"attributes":{}} , event_time)
    ops = [op]
    if args.error:
        fingerprint = digest(f"{row['project_id']}|{args.exception}|{args.message}")
        ops.append(operation("error.record", {"eventId":event_id,"eventOccurredAt":event_time,"projectId":row["project_id"],"errorGroupId":uuid7(),"fingerprint":fingerprint,"exceptionClass":args.exception,"normalizedSummary":args.message,"handled":args.handled,"retryable":args.retryable}, event_time))
    conn = db()
    try:
        with conn:
            conn.execute("UPDATE context SET sequence_no=?,updated_at=? WHERE native_session=?",(seq,now(),row["native_session"]))
    finally:
        conn.close()
    enqueue(ops)


def enrich_prompt(args: argparse.Namespace) -> None:
    row = current_or_fail()
    enqueue([operation("prompt.enrich", {"promptId":row["prompt_id"],"summary":args.summary,"intent":args.intent,"targetSystem":args.target,"constraints":args.constraint,"successCriteria":args.success,"resultSummary":args.result,"languageCode":args.language})])


def ingest_runtime_events(args: argparse.Namespace) -> None:
    found = project_config()
    if not found:
        raise SystemExit("No observability.project.toml found.")
    source = Path(args.file).resolve()
    if not source.is_file():
        raise SystemExit(f"Structured event source not found: {source}")

    config_path, config = found
    project = register_project(config_path, config)
    root = config_path.parent
    commit = args.commit or git_value(root, "rev-parse", "HEAD") or None
    branch = git_value(root, "branch", "--show-current") or None
    code_version_id = str(uuid.uuid5(uuid.NAMESPACE_URL, f"{project['id']}|{commit or ''}|"))
    execution_id, trace_id = uuid7(), os.urandom(16).hex()
    enqueue([
        operation("code-version.register", {"codeVersionId":code_version_id,"projectId":project["id"],"commitSha":commit,"branchName":branch}),
        operation("execution.start", {"executionId":execution_id,"projectId":project["id"],"codeVersionId":code_version_id,"executionKind":2,"attemptNo":1,"traceId":trace_id}),
    ])

    accepted = skipped = errors = sequence = 0
    batch: list[dict[str, Any]] = []
    with source.open("r", encoding="utf-8", errors="replace") as handle:
        for line in handle:
            if args.match and args.match not in line:
                continue
            value = structured_event_from_line(line)
            if value is None:
                skipped += 1
                continue
            if accepted >= args.max_events:
                break
            accepted += 1
            sequence += 1
            event_id = uuid7()
            event_time = observed_time(value.get("timestamp"))
            code = str(value.get("event") or "runtime.event")[:160]
            severity = severity_number(value.get("severity"))
            outcome = outcome_number(value.get("outcome"))
            component = str(args.component or value.get("component") or "application")[:160]
            exception_class = str(value.get("exception_type") or "RuntimeError")[:240]
            summary = runtime_summary(value)
            locator = source_locator(value)
            attributes = {
                "operation": str(value.get("operation") or "unknown")[:160],
                "exceptionType": exception_class,
                "stackPresent": bool(value.get("stack_present")),
                "sourceLocator": locator,
                "layoutOverflow": value.get("layout_overflow"),
                "ingestSource": source.name[:160],
                "details": bounded_runtime_details(value),
            }
            batch.append(operation("event.emit", {
                "eventId":event_id,"executionId":execution_id,"projectId":project["id"],"traceId":trace_id,
                "sequenceNo":sequence,"code":code,"category":4 if severity >= 3 else 1,"severity":severity,
                "outcome":outcome,"component":component,"messageSummary":summary,"attributes":attributes,
                "rawPayload":value,
            }, event_time))
            if severity >= 3 or outcome == 3:
                errors += 1
                normalized = re.sub(r"\b(?:0x)?[0-9a-f]{6,}|\b\d+\b", "#", summary, flags=re.I)
                stack = value.get("stack")
                batch.append(operation("error.record", {
                    "eventId":event_id,"eventOccurredAt":event_time,"projectId":project["id"],"errorGroupId":uuid7(),
                    "fingerprint":digest(f"{project['id']}|{code}|{exception_class}|{normalized}"),
                    "exceptionClass":exception_class,"normalizedSummary":summary,"handled":bool(value.get("handled")),
                    "retryable":bool(value.get("retryable")),"stackHash":digest(stack) if isinstance(stack, str) else None,
                    "topFrame":locator,
                }, event_time))
            if len(batch) >= 100:
                enqueue(batch)
                batch = []
    if batch:
        enqueue(batch)
    enqueue([operation("execution.finish", {
        "executionId":execution_id,"status":2,"outcome":1,
        "resultSummary":f"Ingested {accepted} structured events and grouped {errors} errors from {source.name}.",
    })])
    print(json.dumps({"accepted":accepted,"errors":errors,"skipped":skipped,"executionId":execution_id,"traceId":trace_id}, separators=(",", ":")))


def query(path: str) -> None:
    try:
        with urllib.request.urlopen(f"{ENDPOINT}{path}", timeout=5) as response:
            print(json.dumps(json.load(response), ensure_ascii=False, indent=2, default=str))
    except urllib.error.URLError as exc:
        raise SystemExit(f"Observability hub unavailable: {exc}")


def main() -> int:
    parser = argparse.ArgumentParser(prog="obs")
    sub = parser.add_subparsers(dest="command", required=True)
    project = sub.add_parser("project")
    project_sub = project.add_subparsers(dest="project_command", required=True)
    register = project_sub.add_parser("register")
    register.add_argument("--init", action="store_true")
    register.add_argument("--name")
    register.add_argument("--slug")
    prompt = sub.add_parser("prompt")
    prompt_sub = prompt.add_subparsers(dest="prompt_command", required=True)
    enrich = prompt_sub.add_parser("enrich")
    enrich.add_argument("--summary", required=True)
    enrich.add_argument("--intent", default="")
    enrich.add_argument("--target", default="")
    enrich.add_argument("--constraint", action="append", default=[])
    enrich.add_argument("--success", action="append", default=[])
    enrich.add_argument("--result", default="")
    enrich.add_argument("--language", default="tl-en")
    show = prompt_sub.add_parser("show")
    show.add_argument("prompt_id")
    event = sub.add_parser("event")
    event_sub = event.add_subparsers(dest="event_command", required=True)
    emit = event_sub.add_parser("emit")
    emit.add_argument("--code", required=True)
    emit.add_argument("--message", default="")
    emit.add_argument("--component", default="agent")
    emit.add_argument("--category", type=int, default=1)
    emit.add_argument("--severity", type=int, choices=range(1,6), default=1)
    emit.add_argument("--outcome", type=int, choices=range(1,4))
    emit.add_argument("--error", action="store_true")
    emit.add_argument("--exception", default="Error")
    emit.add_argument("--handled", action="store_true")
    emit.add_argument("--retryable", action="store_true")
    errors = sub.add_parser("errors")
    errors.add_argument("--project")
    analytics = sub.add_parser("analytics")
    analytics.add_argument("--project")
    analytics.add_argument("--days", type=int, default=7)
    trace = sub.add_parser("trace")
    trace.add_argument("trace_id")
    diagnose = sub.add_parser("diagnose")
    diagnose.add_argument("error_group_id")
    context = sub.add_parser("context")
    context.add_argument("--vision", default="")
    context.add_argument("--architecture", default="")
    context.add_argument("--goal", default="")
    context.add_argument("--constraint", action="append", default=[])
    runtime = sub.add_parser("runtime")
    runtime_sub = runtime.add_subparsers(dest="runtime_command", required=True)
    ingest = runtime_sub.add_parser("ingest")
    ingest.add_argument("--file", required=True)
    ingest.add_argument("--match", default="")
    ingest.add_argument("--component", default="")
    ingest.add_argument("--commit", default="")
    ingest.add_argument("--max-events", type=int, default=1000)
    change = sub.add_parser("change")
    change_sub = change.add_subparsers(dest="change_command", required=True)
    change_plan = change_sub.add_parser("plan")
    change_plan.add_argument("--kind", choices=("non-code", "docs", "media", "code"), required=True)
    change_plan.add_argument("--class", dest="change_class", choices=("routine", "moderate", "substantial"), default="routine")
    change_plan.add_argument("--estimated-lines", type=int, default=0)
    change_plan.add_argument("--estimated-files", type=int, default=0)
    change_plan.add_argument("--risk", action="append", default=[])
    change_plan.add_argument("--source-url", action="append", default=[])
    change_plan.add_argument("--reason", default="")
    change_plan.add_argument("--prompt-id")
    change_plan.add_argument("--delivery-stream")
    change_finish = change_sub.add_parser("finish")
    change_finish.add_argument("--outcome", choices=("success", "stopped", "failed"), required=True)
    change_finish.add_argument("--lines-added", type=int, default=0)
    change_finish.add_argument("--lines-deleted", type=int, default=0)
    change_finish.add_argument("--files-changed", type=int, default=0)
    change_finish.add_argument("--input-tokens", type=int)
    change_finish.add_argument("--output-tokens", type=int)
    change_finish.add_argument("--cached-input-tokens", type=int)
    change_finish.add_argument("--reasoning-tokens", type=int)
    change_finish.add_argument("--source-url", action="append", default=[])
    change_finish.add_argument("--threshold", type=int, default=5)
    change_finish.add_argument("--prompt-id")
    change_finish.add_argument("--delivery-stream")
    change_package = change_sub.add_parser("package")
    change_package.add_argument("--delivery-mode", choices=("local", "push", "pull-request"), required=True)
    change_package.add_argument("--commit-sha", default="")
    change_package.add_argument("--pr-url", default="")
    change_package.add_argument("--confirm-policy-reviewed", action="store_true")
    change_package.add_argument("--prompt-id")
    change_package.add_argument("--delivery-stream")
    git = sub.add_parser("git")
    git_sub = git.add_subparsers(dest="git_command", required=True)
    git_check = git_sub.add_parser("check")
    git_check.add_argument("--threshold", type=int, default=5)
    git_squash = git_sub.add_parser("squash")
    git_squash.add_argument("--message", required=True)
    git_squash.add_argument("--expected-head", required=True)
    git_squash.add_argument("--confirm-all-local-related", action="store_true")
    sub.add_parser("flush")
    args = parser.parse_args()

    if args.command == "project":
        found = project_config()
        path = Path.cwd() / "observability.project.toml"
        if args.init and not found:
            name = args.name or Path.cwd().name
            slug = args.slug or re.sub(r"[^a-z0-9._-]+", "-", name.lower()).strip("-")
            path.write_text(f'[project]\nid = "{uuid7()}"\nname = {json.dumps(name)}\nslug = {json.dumps(slug)}\nschema_version = 1\n', encoding="utf-8")
            found = project_config()
        if not found:
            raise SystemExit("No observability.project.toml found. Use obs project register --init.")
        print(json.dumps(register_project(*found), indent=2))
    elif args.command == "prompt" and args.prompt_command == "enrich": enrich_prompt(args)
    elif args.command == "prompt" and args.prompt_command == "show": query(f"/v1/prompts/{args.prompt_id}")
    elif args.command == "event": emit_event(args)
    elif args.command == "errors":
        project_id = args.project or (project_config()[1]["project"]["id"] if project_config() else None)
        if not project_id: raise SystemExit("Project ID is required.")
        query(f"/v1/projects/{project_id}/errors")
    elif args.command == "analytics":
        project_id = args.project or (project_config()[1]["project"]["id"] if project_config() else None)
        if not project_id: raise SystemExit("Project ID is required.")
        if not 1 <= args.days <= 30: raise SystemExit("Days must be between 1 and 30.")
        query(f"/v1/projects/{project_id}/analytics?days={args.days}")
    elif args.command == "trace": query(f"/v1/traces/{args.trace_id}")
    elif args.command == "diagnose": query(f"/v1/errors/{args.error_group_id}")
    elif args.command == "context":
        found = project_config()
        if not found: raise SystemExit("No observability.project.toml found.")
        row = context_for(str(Path.cwd()))
        enqueue([operation("context.record", {"contextId":uuid7(),"projectId":found[1]["project"]["id"],"languageCode":"tl-en","visionSummary":args.vision,"architectureSummary":args.architecture,"currentGoalSummary":args.goal,"constraints":args.constraint,"sourcePromptId":row["prompt_id"] if row else None})])
    elif args.command == "runtime" and args.runtime_command == "ingest": ingest_runtime_events(args)
    elif args.command == "change" and args.change_command == "plan": print(json.dumps(plan_change(args), indent=2))
    elif args.command == "change" and args.change_command == "finish":
        if args.threshold < 1:
            raise SystemExit("Threshold must be at least 1.")
        for name in ("lines_added", "lines_deleted", "files_changed", "input_tokens", "output_tokens", "cached_input_tokens", "reasoning_tokens"):
            value = getattr(args, name)
            if value is not None and value < 0:
                raise SystemExit(f"{name.replace('_', '-')} cannot be negative.")
        print(json.dumps(finish_change(args), indent=2))
    elif args.command == "change" and args.change_command == "package": print(json.dumps(package_changes(args), indent=2))
    elif args.command == "git" and args.git_command == "check":
        try:
            result = git_policy_status(threshold=args.threshold)
        except SystemExit as exc:
            record_git_policy_event("git.policy.check_failed", str(exc)[:300], 3, 3, {"threshold": args.threshold})
            raise
        result["observabilityRecorded"] = record_git_policy_event(
            "git.policy.checked",
            "Reviewed the configured GitHub upstream commit threshold.",
            1,
            1,
            {key: result[key] for key in (
                "branch", "upstream", "aheadCommits", "behindCommits", "codeCommits",
                "nonCodeCommits", "threshold", "thresholdReached", "pushRecommended",
            )},
        )
        print(json.dumps(result, indent=2))
    elif args.command == "git" and args.git_command == "squash":
        try:
            result = squash_local_commits(
                args.message,
                expected_head=args.expected_head,
                all_local_commits_are_related=args.confirm_all_local_related,
            )
        except SystemExit as exc:
            record_git_policy_event("git.local_history.squash_refused", str(exc)[:300], 3, 3, {})
            raise
        result["observabilityRecorded"] = record_git_policy_event(
            "git.local_history.squashed",
            "Consolidated reviewed unpushed commits and retained a recovery ref.",
            1,
            1,
            {
                "branch": result["branch"],
                "upstream": result["upstream"],
                "squashedCommitCount": result["squashedCommitCount"],
                "backupRef": result["backupRef"],
            },
        )
        print(json.dumps(result, indent=2))
    elif args.command == "flush": print(json.dumps({"flushed": flush()}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
