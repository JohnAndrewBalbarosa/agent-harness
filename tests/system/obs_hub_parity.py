#!/usr/bin/env python3
"""Whole-system parity test: reference Node observability hub vs the Rust `obs-hub`.

Run:
  python tests/system/obs_hub_parity.py [--keep] [--phase all|a|b] [--node-hub DIR]

Prerequisites (checked, never started or installed here):
  - Docker; container `codex-observability-postgres` running (host port 127.0.0.1:55543).
  - Node hub sources + node_modules at ~/.codex-shared/tools/observability-hub (or --node-hub); `node` on PATH.
  - Linux release binary from `scripts/rust-docker.ps1 build-linux` in volume `agent-harness-cargo-target`
    (image `agent-harness-rust-builder:1.98`).
  - pwsh/powershell (Windows) or bash (POSIX) for scripts/obs-export-postgres.*.
  - Free ports 43190-43193.

Phase A: one deterministic operation stream (all 28 kinds) + replays + failing batches → both hubs → GET every route.
Phase B: frozen copy of the live DB → Node hub on the copy (startup prunes) → export → import-postgres → Rust hub
         → per-project and sampled lookups.

Safety: the live database `observability` is only read (pg_dump). Only `obs_parity_<ts>` / `obs_snapshot_<ts>` are
created and always dropped. Temp dirs and dumps hold raw prompts and are deleted unless --keep.
Output: bounded mismatch lines (route, JSON path, type/length only) then one JSON summary line; exit 1 on
unexpected mismatches or phase errors.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

sys.path.insert(0, str(Path(__file__).resolve().parent))

from parity_diff import MODE_EXACT, HubResponse, MismatchPrinter, ParityReport  # noqa: E402
from parity_stream import (  # noqa: E402
    REPLAYED_BATCH_INDEXES, OperationClock, Probe, acceptance_edge_probes, build_operation_batches,
    failing_batch_probes, operations_body, route_probes, synthetic_ids,
)

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_NODE_HUB = Path.home() / ".codex-shared" / "tools" / "observability-hub"

PG_CONTAINER = "codex-observability-postgres"
PG_HOST_PORT = 55543
PG_USER = "observability"
# Local-only credential of the dev container (same value as hub/docker-compose.yml); override via env.
PG_PASSWORD = os.environ.get("OBS_PG_PASSWORD", "local-observability-only")
LIVE_DATABASE = "observability"
THROWAWAY_DATABASE = re.compile(r"^obs_(parity|snapshot)_\d+$")

RUST_IMAGE = "agent-harness-rust-builder:1.98"
CARGO_TARGET_VOLUME = "agent-harness-cargo-target"
RUST_BINARY = "/work/target/release/obs-hub"
RUST_CONTAINER_PORT = 4319
SQLITE_FILE = "hub.sqlite3"

PHASE_A_RUST_PORT, PHASE_A_NODE_PORT = 43190, 43191
PHASE_B_RUST_PORT, PHASE_B_NODE_PORT = 43192, 43193

MAX_MISMATCH_LINES = 50
LOG_TAIL_LINES = 20
PHASE_B_SAMPLE_LIMIT = 10
HEALTH_ATTEMPTS = 240
HEALTH_DELAY_SECONDS = 0.5
HTTP_TIMEOUT_SECONDS = 60
SHORT_COMMAND_TIMEOUT = 120
LONG_COMMAND_TIMEOUT = 1800

_HTTP = urllib.request.build_opener(urllib.request.ProxyHandler({}))


class ParityError(RuntimeError):
    """Setup/infrastructure failure; message never contains response values."""


# Mental model:
# check prerequisites → phase A (synthetic stream) → phase B (frozen real data)
# → always release processes, containers, databases, temp dirs → one JSON summary line
def main() -> int:
    arguments = parse_arguments()
    resources = RunResources(run_id=int(time.time()), keep=arguments.keep)
    printer = MismatchPrinter(MAX_MISMATCH_LINES)
    summary: dict[str, object] = {}
    reports: list[ParityReport] = []
    try:
        verify_prerequisites(arguments.node_hub)
        if arguments.phase in ("all", "a"):
            reports.append(run_phase("A", True, printer, lambda report: synthetic_stream_parity(
                resources, report, arguments.node_hub)))
            summary["phaseA"] = reports[-1].summary()
        if arguments.phase in ("all", "b"):
            reports.append(run_phase("B", False, printer, lambda report: real_data_parity(
                resources, report, arguments.node_hub)))
            summary["phaseB"] = reports[-1].summary()
    except ParityError as error:
        summary["error"] = bounded_message(error)
    finally:
        resources.release()
        printer.print_overflow()

    summary["ok"] = "error" not in summary and bool(reports) and all(report.ok for report in reports)
    print(json.dumps(summary, sort_keys=True), flush=True)
    return 0 if summary["ok"] else 1


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Node vs Rust observability hub parity test")
    parser.add_argument("--keep", action="store_true", help="keep temp dirs and dumps (they contain raw prompts)")
    parser.add_argument("--phase", choices=("all", "a", "b"), default="all")
    parser.add_argument("--node-hub", type=Path, default=DEFAULT_NODE_HUB)
    return parser.parse_args()


class RunResources:
    """Everything this run creates, released in reverse dependency order in `release()`."""

    def __init__(self, run_id: int, keep: bool) -> None:
        self.run_id = run_id
        self.keep = keep
        self.root = Path(tempfile.mkdtemp(prefix=f"obs-parity-{run_id}-"))
        self.node_processes: list[subprocess.Popen] = []
        self.containers: list[str] = []
        self.container_files: list[str] = []
        self.databases: list[str] = []

    def temp_dir(self, name: str) -> Path:
        path = self.root / name
        path.mkdir(parents=True, exist_ok=False)
        return path

    def log_path(self, name: str) -> Path:
        logs = self.root / "logs"
        logs.mkdir(exist_ok=True)
        return logs / name

    def create_database(self, name: str) -> str:
        require_throwaway(name)
        self.databases.append(name)
        psql(LIVE_DATABASE, f'CREATE DATABASE "{name}"')
        return name

    def release(self) -> None:
        for process in self.node_processes:
            best_effort("stop node hub", lambda: stop_process(process))
        for container in self.containers:
            best_effort("remove container", lambda: run_command(
                ["docker", "rm", "-f", container], "docker rm", SHORT_COMMAND_TIMEOUT))
        for remote_file in self.container_files:
            best_effort("remove container dump", lambda: run_command(
                ["docker", "exec", PG_CONTAINER, "rm", "-f", remote_file], "docker exec rm", SHORT_COMMAND_TIMEOUT))
        for database in self.databases:
            best_effort("drop database", lambda: drop_database(database))
        if self.keep:
            print(f"kept temp dir (contains raw prompts): {self.root}", flush=True)
        else:
            shutil.rmtree(self.root, ignore_errors=True)


def require_throwaway(database: str) -> None:
    if not THROWAWAY_DATABASE.match(database) or database == LIVE_DATABASE:
        raise ParityError(f"refusing to touch non-throwaway database {database!r}")


def psql(database: str, sql: str, timeout: int = SHORT_COMMAND_TIMEOUT) -> list[str]:
    command = ["docker", "exec", "-e", f"PGPASSWORD={PG_PASSWORD}", PG_CONTAINER, "psql", "-X", "-q", "-A", "-t",
               "-v", "ON_ERROR_STOP=1", "-U", PG_USER, "-d", database, "-c", sql]
    output = run_command(command, "psql", timeout)
    return [line.strip() for line in output.splitlines() if line.strip()]


def run_command(command: list[str], label: str, timeout: int, cwd: Path | None = None) -> str:
    """Foreground command with a timeout; on failure only a bounded stderr tail is printed."""
    try:
        result = subprocess.run(command, cwd=cwd, capture_output=True, text=True, encoding="utf-8",
                                errors="replace", timeout=timeout, check=False)
    except subprocess.TimeoutExpired as error:
        raise ParityError(f"{label} timed out after {timeout}s") from error
    if result.returncode != 0:
        print_tail(f"{label} stderr", result.stderr)
        raise ParityError(f"{label} failed with exit code {result.returncode}")
    return result.stdout


def print_tail(title: str, text: str) -> None:
    lines = text.splitlines()[-LOG_TAIL_LINES:]
    print(f"--- {title} (last {len(lines)} lines) ---", file=sys.stderr)
    for line in lines:
        print(line, file=sys.stderr)
    sys.stderr.flush()


def stop_process(process: subprocess.Popen) -> None:
    if process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=10)


def drop_database(database: str) -> None:
    require_throwaway(database)
    psql(LIVE_DATABASE, f'DROP DATABASE IF EXISTS "{database}" WITH (FORCE)')


def best_effort(action: str, work: Callable[[], object]) -> None:
    try:
        work()
    except Exception as error:  # cleanup must continue; report bounded reason only
        print(f"cleanup warning: {action}: {bounded_message(error)}", file=sys.stderr, flush=True)


def bounded_message(error: BaseException) -> str:
    return f"{type(error).__name__}: {str(error)[:300]}"


# Mental model:
# node sources present → docker container running → image, volume, binary present → ports free
def verify_prerequisites(node_hub: Path) -> None:
    if not (node_hub / "src" / "server.mjs").is_file() or not (node_hub / "node_modules").is_dir():
        raise ParityError(f"node hub sources/node_modules missing under {node_hub}")
    if shutil.which("node") is None:
        raise ParityError("node is not on PATH")
    running = run_command(["docker", "inspect", "-f", "{{.State.Running}}", PG_CONTAINER], "docker inspect",
                          SHORT_COMMAND_TIMEOUT)
    if running.strip() != "true":
        raise ParityError(f"container {PG_CONTAINER} is not running")
    run_command(["docker", "image", "inspect", RUST_IMAGE], "docker image inspect", SHORT_COMMAND_TIMEOUT)
    run_command(["docker", "volume", "inspect", CARGO_TARGET_VOLUME], "docker volume inspect", SHORT_COMMAND_TIMEOUT)
    run_command(["docker", "run", "--rm", "-v", f"{CARGO_TARGET_VOLUME}:/work/target:ro", RUST_IMAGE,
                 "test", "-x", RUST_BINARY], "rust binary check", SHORT_COMMAND_TIMEOUT)
    for port in (PHASE_A_RUST_PORT, PHASE_A_NODE_PORT, PHASE_B_RUST_PORT, PHASE_B_NODE_PORT):
        if port_is_listening(port):
            raise ParityError(f"port {port} is already in use")


def port_is_listening(port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.settimeout(0.5)
        return probe.connect_ex(("127.0.0.1", port)) == 0


def run_phase(name: str, mask_created_at: bool, printer: MismatchPrinter,
              work: Callable[[ParityReport], None]) -> ParityReport:
    report = ParityReport(name, mask_created_at, printer)
    print(f"[phase {name}] starting", flush=True)
    try:
        work(report)
    except Exception as error:  # phase failure is reported in the summary; cleanup still runs in main()
        report.error = bounded_message(error)
        print(f"[phase {name}] error: {report.error}", flush=True)
    return report


@dataclass(frozen=True)
class HubPair:
    node_url: str
    rust_url: str


# --------------------------------------------------------------------------- phase A

# Mental model:
# throwaway Postgres DB + empty SQLite dir → both hubs healthy → identical stream batches
# → replay batches (idempotency) → failing batches → edge probes → GET every route
def synthetic_stream_parity(resources: RunResources, report: ParityReport, node_hub: Path) -> None:
    database = resources.create_database(f"obs_parity_{resources.run_id}")
    hubs = HubPair(
        node_url=start_node_hub(resources, node_hub, database, PHASE_A_NODE_PORT, "a"),
        rust_url=start_rust_hub(resources, resources.temp_dir("phase-a-sqlite"), PHASE_A_RUST_PORT, "a"),
    )
    clock = OperationClock.recent()
    ids = synthetic_ids()
    batches = build_operation_batches(clock, ids)

    for number, batch in enumerate(batches, start=1):
        post_stream_batch(report, hubs, f"stream batch {number}/{len(batches)}", batch)
    for index in REPLAYED_BATCH_INDEXES:
        post_stream_batch(report, hubs, f"replay of stream batch {index + 1}", batches[index])

    for probe in failing_batch_probes(clock, ids) + acceptance_edge_probes(clock, ids) + route_probes(clock, ids):
        compare_probe(report, hubs, probe)


def start_node_hub(resources: RunResources, node_hub: Path, database: str, port: int, label: str) -> str:
    log_path = resources.log_path(f"node-{label}.log")
    environment = {
        **os.environ,
        "OBS_DATABASE_URL": f"postgres://{PG_USER}:{PG_PASSWORD}@127.0.0.1:{PG_HOST_PORT}/{database}",
        "OBS_HOST": "127.0.0.1",
        "OBS_PORT": str(port),
        "OBS_LOG_LEVEL": "warn",
    }
    with log_path.open("wb") as log:
        process = subprocess.Popen(["node", "src/server.mjs"], cwd=node_hub, env=environment,
                                   stdout=log, stderr=subprocess.STDOUT)
    resources.node_processes.append(process)
    base_url = f"http://127.0.0.1:{port}"
    wait_for_health(base_url, f"node hub {label}", is_alive=lambda: process.poll() is None,
                    log_tail=lambda: log_path.read_text(encoding="utf-8", errors="replace"))
    return base_url


def start_rust_hub(resources: RunResources, data_dir: Path, port: int, label: str) -> str:
    name = f"obs-parity-rust-{label}-{resources.run_id}"
    resources.containers.append(name)
    run_command([
        "docker", "run", "-d", "--name", name, "-p", f"127.0.0.1:{port}:{RUST_CONTAINER_PORT}",
        "-e", "OBS_HOST=0.0.0.0", "-e", f"OBS_DB_PATH=/data/{SQLITE_FILE}", "-e", "OBS_LOG_LEVEL=warn",
        "-v", f"{CARGO_TARGET_VOLUME}:/work/target:ro", "-v", f"{data_dir}:/data",
        RUST_IMAGE, RUST_BINARY, "serve",
    ], "docker run rust hub", SHORT_COMMAND_TIMEOUT)
    base_url = f"http://127.0.0.1:{port}"
    wait_for_health(base_url, f"rust hub {label}", is_alive=lambda: container_is_running(name),
                    log_tail=lambda: container_logs(name))
    return base_url


def wait_for_health(base_url: str, label: str, is_alive: Callable[[], bool], log_tail: Callable[[], str]) -> None:
    health = Probe("GET /health", "GET", "/health")
    for _ in range(HEALTH_ATTEMPTS):
        if send_probe(base_url, health).status == 200:
            return
        if not is_alive():
            print_tail(f"{label} log", log_tail())
            raise ParityError(f"{label} exited before becoming healthy")
        time.sleep(HEALTH_DELAY_SECONDS)
    print_tail(f"{label} log", log_tail())
    raise ParityError(f"{label} not healthy after {HEALTH_ATTEMPTS * HEALTH_DELAY_SECONDS:.0f}s")


def container_is_running(name: str) -> bool:
    try:
        return run_command(["docker", "inspect", "-f", "{{.State.Running}}", name], "docker inspect",
                           SHORT_COMMAND_TIMEOUT).strip() == "true"
    except ParityError:
        return False


def container_logs(name: str) -> str:
    result = subprocess.run(["docker", "logs", "--tail", str(LOG_TAIL_LINES), name], capture_output=True, text=True,
                            encoding="utf-8", errors="replace", timeout=SHORT_COMMAND_TIMEOUT, check=False)
    return result.stdout + result.stderr


def post_stream_batch(report: ParityReport, hubs: HubPair, label: str, operations: list[dict]) -> None:
    probe = Probe(f"POST /v1/operations [{label}]", "POST", "/v1/operations", MODE_EXACT,
                  operations_body(operations), "application/json")
    node = send_probe(hubs.node_url, probe)
    rust = send_probe(hubs.rust_url, probe)
    report.require_reference_success(probe.label, node)
    report.compare(probe.label, node, rust, probe.mode)


def compare_probe(report: ParityReport, hubs: HubPair, probe: Probe) -> None:
    node = send_probe(hubs.node_url, probe)
    rust = send_probe(hubs.rust_url, probe)
    report.compare(probe.label, node, rust, probe.mode)


def send_probe(base_url: str, probe: Probe) -> HubResponse:
    headers = {"content-type": probe.content_type} if probe.content_type else {}
    request = urllib.request.Request(base_url + probe.path, data=probe.body, method=probe.method, headers=headers)
    try:
        with _HTTP.open(request, timeout=HTTP_TIMEOUT_SECONDS) as response:
            return decode_response(response.status, response.read())
    except urllib.error.HTTPError as error:
        return decode_response(error.code, error.read())
    except (urllib.error.URLError, TimeoutError, ConnectionError, OSError):
        return HubResponse(status=-1, body={"__transport_error__": True})


def decode_response(status: int, raw: bytes) -> HubResponse:
    try:
        return HubResponse(status, json.loads(raw.decode("utf-8")))
    except (ValueError, UnicodeDecodeError):
        return HubResponse(status, {"__non_json_body__": True})


# --------------------------------------------------------------------------- phase B

# Mental model:
# freeze live DB into obs_snapshot → Node hub on the snapshot (startup maintenance prunes)
# → export the pruned snapshot → import into fresh SQLite (report ok) → Rust hub on it
# → compare per-project routes and sampled lookups
def real_data_parity(resources: RunResources, report: ParityReport, node_hub: Path) -> None:
    snapshot = resources.create_database(f"obs_snapshot_{resources.run_id}")
    copy_live_database(resources, snapshot)
    node_url = start_node_hub(resources, node_hub, snapshot, PHASE_B_NODE_PORT, "b")

    dump_dir = resources.temp_dir("phase-b-dump")
    export_snapshot(resources, snapshot, dump_dir)
    data_dir = resources.temp_dir("phase-b-sqlite")
    import_snapshot(resources, dump_dir, data_dir)
    hubs = HubPair(node_url=node_url, rust_url=start_rust_hub(resources, data_dir, PHASE_B_RUST_PORT, "b"))

    for probe in real_data_probes(snapshot):
        compare_probe(report, hubs, probe)


def copy_live_database(resources: RunResources, snapshot: str) -> None:
    """pg_dump reads the live DB (MVCC snapshot, no writes); the dump file stays inside the container."""
    remote_dump = f"/tmp/obs-parity-{resources.run_id}.sql"
    resources.container_files.append(remote_dump)
    base = ["docker", "exec", "-e", f"PGPASSWORD={PG_PASSWORD}", PG_CONTAINER]
    run_command(base + ["pg_dump", "-U", PG_USER, "-d", LIVE_DATABASE, "--no-owner", "--no-privileges",
                        "-f", remote_dump], "pg_dump live", LONG_COMMAND_TIMEOUT)
    run_command(base + ["psql", "-X", "-q", "-v", "ON_ERROR_STOP=1", "-U", PG_USER, "-d", snapshot,
                        "-f", remote_dump, "-o", "/dev/null"], "psql restore snapshot", LONG_COMMAND_TIMEOUT)
    run_command(["docker", "exec", PG_CONTAINER, "rm", "-f", remote_dump], "docker exec rm", SHORT_COMMAND_TIMEOUT)


def export_snapshot(resources: RunResources, snapshot: str, dump_dir: Path) -> None:
    command = export_command(snapshot, dump_dir)
    log_path = resources.log_path("export.log")
    output = run_command(command, "obs-export-postgres", LONG_COMMAND_TIMEOUT, cwd=REPO_ROOT)
    log_path.write_text(output, encoding="utf-8")
    if not (dump_dir / "manifest.json").is_file():
        raise ParityError("export produced no manifest.json")


def export_command(snapshot: str, dump_dir: Path) -> list[str]:
    if os.name == "nt":
        shell = shutil.which("pwsh") or shutil.which("powershell")
        if shell is None:
            raise ParityError("neither pwsh nor powershell is on PATH")
        return [shell, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File",
                str(REPO_ROOT / "scripts" / "obs-export-postgres.ps1"), "-DumpDir", str(dump_dir),
                "-Container", PG_CONTAINER, "-Database", snapshot, "-User", PG_USER]
    return ["bash", str(REPO_ROOT / "scripts" / "obs-export-postgres.sh"), str(dump_dir), PG_CONTAINER, snapshot,
            PG_USER]


def import_snapshot(resources: RunResources, dump_dir: Path, data_dir: Path) -> None:
    name = f"obs-parity-import-{resources.run_id}"
    resources.containers.append(name)
    output = run_command([
        "docker", "run", "--name", name, "-e", "OBS_LOG_LEVEL=warn", "-e", f"OBS_DB_PATH=/data/{SQLITE_FILE}",
        "-v", f"{CARGO_TARGET_VOLUME}:/work/target:ro", "-v", f"{dump_dir}:/dump:ro", "-v", f"{data_dir}:/data",
        RUST_IMAGE, RUST_BINARY, "import-postgres", "--dump", "/dump", "--db", f"/data/{SQLITE_FILE}",
    ], "obs-hub import-postgres", LONG_COMMAND_TIMEOUT)
    import_report = parse_import_report(output)
    if import_report.get("ok") is not True:
        mismatches = import_report.get("mismatches") or []
        raise ParityError(f"import report ok=false ({len(mismatches)} table mismatches)")
    print(f"[phase B] import ok: {len(import_report.get('tables') or [])} tables", flush=True)


def parse_import_report(stdout: str) -> dict:
    """The report is pretty JSON on stdout; log lines (if any) precede it."""
    match = re.search(r"^\{", stdout, flags=re.MULTILINE)
    if match is None:
        raise ParityError("import-postgres printed no JSON report")
    try:
        report, _ = json.JSONDecoder().raw_decode(stdout[match.start():])
    except ValueError as error:
        raise ParityError("import-postgres report is not valid JSON") from error
    if not isinstance(report, dict):
        raise ParityError("import-postgres report is not a JSON object")
    return report


def real_data_probes(snapshot: str) -> list[Probe]:
    limit = PHASE_B_SAMPLE_LIMIT
    projects = psql(snapshot, "SELECT project_id FROM projects ORDER BY project_id")
    errors = psql(snapshot, f"SELECT error_group_id FROM error_groups ORDER BY last_seen_at DESC, error_group_id LIMIT {limit}")
    prompts = psql(snapshot, f"SELECT prompt_id FROM prompt_nodes ORDER BY created_at DESC, prompt_id LIMIT {limit}")
    traces = psql(snapshot, "SELECT encode(trace_id,'hex') FROM executions "
                            f"ORDER BY started_at DESC, execution_id LIMIT {limit}")
    programs = psql(snapshot, f"SELECT project_id || ' ' || program_id FROM benchmark_programs ORDER BY program_id LIMIT {limit}")
    print(f"[phase B] samples: {len(projects)} projects, {len(errors)} error groups, {len(prompts)} prompts, "
          f"{len(traces)} traces, {len(programs)} programs", flush=True)

    probes = [Probe("GET /health", "GET", "/health")]
    probes += [Probe(f"GET /v1/projects/:id/{suffix} [project #{index}]", "GET", f"/v1/projects/{project}/{suffix}")
               for index, project in enumerate(projects, start=1)
               for suffix in ("errors", "context/bootstrap", "analytics?days=30", "benchmarks")]
    probes += [Probe(f"GET /v1/errors/:id [#{index}]", "GET", f"/v1/errors/{value}")
               for index, value in enumerate(errors, start=1)]
    probes += [Probe(f"GET /v1/prompts/:id [#{index}]", "GET", f"/v1/prompts/{value}")
               for index, value in enumerate(prompts, start=1)]
    probes += [Probe(f"GET /v1/traces/:hex [#{index}]", "GET", f"/v1/traces/{value}")
               for index, value in enumerate(traces, start=1)]
    probes += benchmark_program_probes(programs)
    return probes


def benchmark_program_probes(programs: list[str]) -> list[Probe]:
    probes = []
    for index, row in enumerate(programs, start=1):
        project, program = row.split(" ", 1)
        base = f"/v1/projects/{project}/benchmarks/{program}"
        probes.append(Probe(f"GET evaluation-data [program #{index}]", "GET", f"{base}/evaluation-data"))
        probes.append(Probe(f"GET report [program #{index}]", "GET", f"{base}/report"))
    return probes


if __name__ == "__main__":
    sys.exit(main())
