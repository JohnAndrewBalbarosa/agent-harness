"""Downscale screenshots before an agent reads them (measured 2026-09-28: images were 85% of the tool output fed back
into Claude's context). Uses Windows PowerShell's built-in System.Drawing: no extra install.

`harness image <file> [--max-edge 1024]` prints the path the agent should read (the original if already small).
"""
from __future__ import annotations

import hashlib
import os
import subprocess
from pathlib import Path

IMAGE_SUFFIXES = {".png", ".jpg", ".jpeg", ".bmp", ".gif", ".webp"}
DEFAULT_MAX_EDGE = 1024
HIDDEN = int(getattr(subprocess, "CREATE_NO_WINDOW", 0))
_PS = "powershell.exe"


SCRIPT = Path(__file__).with_name("image.ps1")


def _powershell(*args: str) -> str:
    env = {k: v for k, v in os.environ.items() if k.upper() != "PSMODULEPATH"}  # pwsh parents break PS 5.1 modules
    done = subprocess.run([_PS, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", str(SCRIPT), *args],
                          capture_output=True, text=True, timeout=60, creationflags=HIDDEN, env=env)
    if done.returncode != 0:
        raise RuntimeError(f"System.Drawing failed: {(done.stderr or done.stdout).strip()[-300:]}")
    return done.stdout.strip()


def size(path: Path) -> tuple[int, int]:
    width, height = _powershell("-Mode", "size", "-Source", str(path)).split()
    return int(width), int(height)


def resize(source: Path, target: Path, width: int, height: int) -> Path:
    target.parent.mkdir(parents=True, exist_ok=True)
    _powershell("-Mode", "resize", "-Source", str(source), "-Target", str(target), "-Width", str(width), "-Height", str(height))
    return target


def downscale(source: Path, max_edge: int = DEFAULT_MAX_EDGE, cache_dir: Path | None = None) -> Path:
    if source.suffix.lower() not in IMAGE_SUFFIXES or not source.is_file():
        raise ValueError(f"not an image file: {source}")
    width, height = size(source)
    if max(width, height) <= max_edge:
        return source
    scale = max_edge / max(width, height)
    stat = source.stat()
    key = hashlib.sha1(f"{source.resolve()}|{stat.st_size}|{stat.st_mtime_ns}|{max_edge}".encode()).hexdigest()[:16]
    target = (cache_dir or source.parent) / f"{source.stem}.{key}.{max_edge}px.png"
    if target.exists():
        return target
    return resize(source, target, max(1, round(width * scale)), max(1, round(height * scale)))
