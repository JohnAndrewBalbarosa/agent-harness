"""Sandbox end-to-end proof: packaged Rust selection -> shared spool -> HTTP hub -> normalized SQLite.

Run after building matching binaries: python tests/system/markdown_context.py --bin-dir target-windows
Only synthetic sandbox files, a free loopback port and the process started here are touched.
"""
from __future__ import annotations

import argparse
from contextlib import closing
import json
import os
import shutil
from pathlib import Path
import socket
import sqlite3
import subprocess
import tempfile
import time
import urllib.request

PROJECT = "01a0d736-8ed8-76dc-9c20-c342229f9a82"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--bin-dir", type=Path, required=True)
    args = parser.parse_args()
    suffix = ".exe" if os.name == "nt" else ""
    binaries = args.bin_dir.resolve()
    with tempfile.TemporaryDirectory(prefix="markdown-context-e2e-") as temporary:
        work = Path(temporary)
        projects = work / "Projects"
        repo = projects / "repo"
        (repo / "docs").mkdir(parents=True)
        (repo / "observability.project.toml").write_text(
            f'[project]\nid="{PROJECT}"\nname="sandbox"\nslug="sandbox"\nschema_version=1\n', encoding="utf-8")
        body = "Synthetic Markdown café 🙂\n"
        (repo / "docs" / "design.md").write_bytes(body.encode("utf-8"))
        env = sandbox_environment(work, projects)
        hook = binaries / f"harness-hook{suffix}"
        inventory = command(hook, env, repo, "index")
        assert inventory["paths"] == ["docs/design.md"] and body not in json.dumps(inventory)
        common = ["--session", "sandbox-session", "--file", "docs/design.md"]
        first = command(hook, env, repo, "read", *common, "--max-chars", "8")
        assert first["documents"][0]["coverage"] == "partial"
        duplicate = command(hook, env, repo, "read", *common, "--max-chars", "8")
        assert duplicate["documents"][0]["outcome"] == "already_observed"
        rest = command(hook, env, repo, "read", *common, "--offset", "8")
        assert first["documents"][0]["content"] + rest["documents"][0]["content"] == body
        full = command(hook, env, repo, "read", *common)
        assert full["documents"][0]["outcome"] == "already_observed"
        (repo / "docs" / "new.md").write_text("new", encoding="utf-8")
        assert "docs/new.md" in command(hook, env, repo, "index")["paths"]
        assert command(hook, env, projects, "index", success=False)["status"] == "failed"
        queued = queued_operations(Path(env["AH_OBS_SPOOL"]))
        assert any(operation["kind"] == "markdown-context.record" for operation in queued)
        verify_hub(binaries / f"obs-hub{suffix}", work, env, queued)
        if os.name == "nt":
            verify_windows_wrapper(hook, work, env, repo, projects)
        # An unavailable metadata spool must not hide a successfully selected document.
        env["AH_OBS_SPOOL"] = str(work / "not-a-directory" / "spool.sqlite3")
        (work / "not-a-directory").write_text("file", encoding="utf-8")
        recovered = command(hook, env, repo, "read", *common, "--epoch", "new-context")
        assert recovered["documents"][0]["content"] == body
    print(json.dumps({"status": "passed", "checks": ["paths_only", "coverage", "dedupe", "refresh",
        "scope", "spool", "http_ingestion", "normalized_rows", "telemetry_failure_isolation", "packaged_wrapper"]}))


def verify_windows_wrapper(hook: Path, work: Path, env: dict[str, str], repo: Path, projects: Path) -> None:
    harness = work / "harness"
    (harness / "core").mkdir(parents=True, exist_ok=True)
    (harness / "bin").mkdir(parents=True, exist_ok=True)
    wrapper = harness / "core" / "harness.cmd"
    shutil.copy2(Path(__file__).resolve().parents[2] / "core" / "harness.cmd", wrapper)
    shutil.copy2(hook, harness / "bin" / "harness-hook.exe")
    for cwd, expected_status in ((repo, 0), (projects, 2)):
        result = subprocess.run(["cmd.exe", "/d", "/c", str(wrapper), "markdown", "index"],
                                cwd=cwd, env=env, capture_output=True, text=True, encoding="utf-8", timeout=30)
        assert result.returncode == expected_status, (
            f"wrapper status {result.returncode}, expected {expected_status}; "
            f"stdout={result.stdout[:200]!r}; stderr={result.stderr[:200]!r}")
        output = json.loads(result.stdout)
        assert ("docs/design.md" in output.get("paths", [])) == (expected_status == 0)


def sandbox_environment(work: Path, projects: Path) -> dict[str, str]:
    env = dict(os.environ)
    for key in list(env):
        if key.startswith("AH_MARKDOWN_"):
            del env[key]
    env.update(AGENT_HARNESS_HOME=str(work / "harness"), AH_OBS_SPOOL=str(work / "spool.sqlite3"),
               AH_MARKDOWN_PROJECTS_DIR=str(projects), AH_MARKDOWN_TELEMETRY_ENABLED="1")
    return env


def command(binary: Path, env: dict[str, str], cwd: Path, action: str, *args: str,
            success: bool = True) -> dict:
    result = subprocess.run([str(binary), "markdown", action, "--cwd", str(cwd), *args], env=env,
                            capture_output=True, text=True, encoding="utf-8", timeout=30)
    assert (result.returncode == 0) == success, f"command status {result.returncode}"
    return json.loads(result.stdout)


def queued_operations(path: Path) -> list[dict]:
    with closing(sqlite3.connect(path)) as connection:
        rows = connection.execute("SELECT payload FROM queue ORDER BY created_at,rowid LIMIT 100").fetchall()
    return [json.loads(row[0]) for row in rows]


def verify_hub(binary: Path, work: Path, env: dict[str, str], operations: list[dict]) -> None:
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    database = work / "hub.sqlite3"
    hub_env = {**env, "OBS_PORT": str(port), "OBS_DB_PATH": str(database)}
    flags = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
    with (work / "hub.log").open("wb") as log:
        process = subprocess.Popen([str(binary), "serve"], env=hub_env, stdout=log, stderr=log,
                                   creationflags=flags)
        try:
            endpoint = f"http://127.0.0.1:{port}"
            wait_until_ready(endpoint, process)
            payload = json.dumps({"operations": operations}).encode()
            for _ in range(2):
                request = urllib.request.Request(endpoint + "/v1/operations", data=payload,
                                                 headers={"content-type": "application/json"})
                with urllib.request.urlopen(request, timeout=5) as response:
                    assert response.status == 200
            with closing(sqlite3.connect(database)) as connection:
                assert connection.execute("SELECT COUNT(*) FROM markdown_documents").fetchone()[0] == 1
                assert connection.execute("SELECT COUNT(*) FROM markdown_document_versions").fetchone()[0] == 1
                assert connection.execute("SELECT COUNT(*) FROM markdown_context_observations").fetchone()[0] == 4
                # None of the dedicated metadata tables has a document-body column.
                columns = connection.execute("PRAGMA table_info(markdown_context_observations)").fetchall()
                assert all(column[1] != "content" for column in columns)
        finally:
            process.terminate()
            process.wait(timeout=10)


def wait_until_ready(endpoint: str, process: subprocess.Popen) -> None:
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        assert process.poll() is None, "sandbox hub exited before readiness"
        try:
            with urllib.request.urlopen(endpoint + "/health", timeout=0.5) as response:
                assert response.status == 200
                return
        except OSError:
            time.sleep(0.1)
    raise AssertionError("sandbox hub readiness timeout")


if __name__ == "__main__":
    main()
