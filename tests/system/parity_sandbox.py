"""Sandboxed harness roots for the one-time hook parity run (Python router = side A, Rust exe = side B).

Each sandbox is a copy of the harness source with every background launch neutralised: the service bootstrap, the
status orb GUI, the spool flusher, the Herdr subscriber and the toast script only append their name to
`<sandbox>/var/launched.txt`. The orb consumer is gated on its venv interpreter, so `.venv` is a directory junction to
the installed orb venv (read-only use). Junctions are removed with `os.rmdir` on the link itself before any tree
deletion; their targets are never touched.
"""
from __future__ import annotations

import os
import secrets
import shutil
import stat
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
COPIED_DIRS = ("core", "adapters", "tools", "compile", "policy")
SKIPPED_NAMES = {"var", "__pycache__", ".pytest_cache", "node_modules", ".git"}
LIVE_ORB_VENV = Path.home() / ".agent-harness" / "tools" / "codex-status-orb" / ".venv"
INLINE_FLUSH = "        if flush_now:\n            flush(conn)"
RUST_EXE = REPO / "target-windows" / "harness-hook.exe"
DEAD_ENDPOINT = "http://127.0.0.1:9"
SCRUBBED_PREFIXES = ("AH_", "OBS_", "HERDR_", "CODEX_STATUS_ORB_", "AGENT_HARNESS_", "CODEX_OBS_", "CONTEXT_WARN",
                     "PYTHON", "HARNESS_", "ORB_", "CLAUDE_CONFIG", "CODEX_HOME", "CODEX_CLI_NOTIFY", "OTLP_")


@dataclass
class Sandbox:
    side: str
    root: Path
    junctions: list[Path] = field(default_factory=list)

    @property
    def orb_data(self) -> Path:
        return self.root / "orb-data"

    @property
    def spool(self) -> Path:
        return self.root / "tools" / "observability-client" / "var" / "spool.sqlite3"

    @property
    def launched(self) -> Path:
        return self.root / "var" / "launched.txt"


@dataclass
class SandboxPair:
    base: Path
    python_side: Sandbox
    rust_side: Sandbox
    codex_home: Path

    @property
    def sides(self) -> tuple[Sandbox, Sandbox]:
        return self.python_side, self.rust_side


# Mental model: temp base → two identical source copies → stubs → shared hmac key → orb junction → Rust exe in B.
def build_sandbox_pair(python_sync_flush: bool = False) -> SandboxPair:
    base = Path(tempfile.mkdtemp(prefix="hook-parity-"))
    hmac_key = secrets.token_bytes(32)
    pair = SandboxPair(base, Sandbox("A", base / "A"), Sandbox("B", base / "B"), base / "codex-home" / ".codex")
    try:
        pair.codex_home.mkdir(parents=True)
        for sandbox in pair.sides:
            copy_harness_source(sandbox.root)
            install_launch_stubs(sandbox.root)
            write_shared_hmac_key(sandbox.root, hmac_key)
            link_orb_venv(sandbox)
            sandbox.orb_data.mkdir()
        install_rust_binary(pair.rust_side.root)
        if not python_sync_flush:
            disable_inline_flush(pair.python_side.root)
    except BaseException:
        for problem in remove_sandbox_pair(pair):
            print(f"CLEANUP {problem}")
        raise
    return pair


def copy_harness_source(root: Path) -> None:
    for name in COPIED_DIRS:
        shutil.copytree(REPO / name, root / name, ignore=skip_runtime_state)
    (root / "var").mkdir(exist_ok=True)


def skip_runtime_state(_directory: str, names: list[str]) -> set[str]:
    return {name for name in names if name in SKIPPED_NAMES or name.startswith(".venv")}


def install_launch_stubs(root: Path) -> None:
    (root / "core" / "services" / "bootstrap.py").write_text(BOOTSTRAP_STUB, encoding="utf-8")
    for relative, label in (("tools/codex-status-orb/orb.py", "orb"),
                            ("tools/observability-client/flush_spool.py", "flush_spool"),
                            ("tools/herdr-bridge/subscriber.py", "herdr_subscriber")):
        (root / relative).write_text(SCRIPT_STUB.format(label=label), encoding="utf-8")
    (root / "core" / "notify" / "toast.ps1").write_text(TOAST_STUB, encoding="utf-8")


def write_shared_hmac_key(root: Path, key: bytes) -> None:
    client_var = root / "tools" / "observability-client" / "var"
    client_var.mkdir(parents=True, exist_ok=True)
    (client_var / "hmac.key").write_bytes(key)


def link_orb_venv(sandbox: Sandbox) -> None:
    if not (LIVE_ORB_VENV / "Scripts" / "python.exe").exists():
        raise SystemExit(f"orb venv not found: {LIVE_ORB_VENV}")
    link = sandbox.root / "tools" / "codex-status-orb" / ".venv"
    done = subprocess.run(["cmd", "/c", "mklink", "/J", str(link), str(LIVE_ORB_VENV)], capture_output=True)
    if os.path.lexists(link):
        sandbox.junctions.append(link)  # registered before validation so cleanup always sees it
    if done.returncode != 0 or not is_junction(link):
        raise SystemExit(f"mklink /J failed for {link} (exit {done.returncode})")


def disable_inline_flush(root: Path) -> None:
    """Side A only: obs.py `enqueue(flush_now=True)` posts inline to the dead endpoint; on Windows every refused
    localhost connect costs ~2 s and the split-retry makes the obs consumer hit its 20 s kill, dropping operations.
    Rust removed that inline flush on purpose (spec deviation), so A gets the same queue-only behaviour."""
    obs = root / "tools" / "observability-client" / "obs.py"
    text = obs.read_text(encoding="utf-8")
    if INLINE_FLUSH not in text:
        raise SystemExit("obs.py inline flush snippet not found; update disable_inline_flush")
    obs.write_text(text.replace(INLINE_FLUSH, "        if False:  # parity sandbox: inline flush disabled\n"
                                              "            flush(conn)"), encoding="utf-8")


def install_rust_binary(root: Path) -> None:
    if not RUST_EXE.exists():
        raise SystemExit(f"Rust binary missing: {RUST_EXE}")
    (root / "bin").mkdir()
    shutil.copy2(RUST_EXE, root / "bin" / "harness-hook.exe")


def is_junction(path: Path) -> bool:
    return bool(getattr(os.path, "isjunction", lambda _p: False)(path)) or path.is_symlink()


# Mental model: environment = scrubbed parent env + identical sandbox-relative keys; agent-specific CODEX_HOME.
def hook_environment(sandbox: Sandbox, codex_home: Path, agent: str) -> dict[str, str]:
    env = {key: value for key, value in os.environ.items() if not key.upper().startswith(SCRUBBED_PREFIXES)}
    env.update({
        "AGENT_HARNESS_HOME": str(sandbox.root),
        "AH_LOG_DIR": str(sandbox.root / "var" / "logs"),
        "AH_NOTIFY_DRY_RUN": "1",
        "CODEX_STATUS_ORB_DATA": str(sandbox.orb_data),
        "OBS_ENDPOINT": DEAD_ENDPOINT,
        "HARNESS_PYTHON": sys.executable,
        "PYTHONPATH": str(sandbox.root),
        "PYTHONUTF8": "1",
        "PYTHONDONTWRITEBYTECODE": "1",
    })
    if agent == "codex":
        env["CODEX_HOME"] = str(codex_home)
    return env


def hook_command(sandbox: Sandbox, agent: str, native_event: str | None) -> list[str]:
    tail = [agent] + ([native_event] if native_event else [])
    if sandbox.side == "A":
        return [sys.executable, "-m", "core.router.harness_hook", *tail]
    return [str(sandbox.root / "bin" / "harness-hook.exe"), *tail]


# Mental model: junctions first (verified, rmdir on the link only) → then trees, never through a live junction.
def remove_sandbox_pair(pair: SandboxPair) -> list[str]:
    problems: list[str] = []
    for sandbox in pair.sides:
        problems.extend(remove_junctions(sandbox))
    if problems:
        return problems + [f"left in place: {pair.base}"]
    shutil.rmtree(pair.base, onexc=clear_readonly_and_retry)
    if pair.base.exists():
        problems.append(f"partially removed: {pair.base}")
    return problems


def clear_readonly_and_retry(function, path, _error) -> None:
    """git object files are read-only on Windows; never follows links (junctions were removed first)."""
    try:
        os.chmod(path, stat.S_IWRITE)
        function(path)
    except OSError:
        pass


def remove_junctions(sandbox: Sandbox) -> list[str]:
    problems = []
    for link in sandbox.junctions:
        if not os.path.lexists(link):
            continue
        if not is_junction(link):
            problems.append(f"not a junction, refusing to delete: {link}")
            continue
        os.rmdir(link)
        if os.path.lexists(link):
            problems.append(f"junction still present: {link}")
    return problems


BOOTSTRAP_STUB = '''"""Parity sandbox stub: records the launch instead of starting services."""
from __future__ import annotations

import subprocess
import sys
from pathlib import Path
from typing import Sequence

ROOT = Path(__file__).resolve().parents[2]
DETACHED = sum(int(getattr(subprocess, n, 0)) for n in ("CREATE_NO_WINDOW", "DETACHED_PROCESS", "CREATE_NEW_PROCESS_GROUP"))


def launch_detached(command: Sequence[str | Path], cwd: Path) -> None:
    subprocess.Popen([str(part) for part in command], cwd=str(cwd), stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                     stderr=subprocess.DEVNULL, close_fds=True, creationflags=DETACHED)


def main() -> int:
    (ROOT / "var").mkdir(exist_ok=True)
    with open(ROOT / "var" / "launched.txt", "a", encoding="utf-8") as stream:
        stream.write("bootstrap\\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
'''

SCRIPT_STUB = '''"""Parity sandbox stub for {label}: records the launch and exits."""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
(ROOT / "var").mkdir(exist_ok=True)
with open(ROOT / "var" / "launched.txt", "a", encoding="utf-8") as stream:
    stream.write("{label}\\n")
'''

TOAST_STUB = '''param([string]$Title = "", [string]$Message = "", [switch]$Silent)
$root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
Add-Content -LiteralPath (Join-Path $root "var\\launched.txt") -Value "toast" -Encoding utf8
exit 0
'''
