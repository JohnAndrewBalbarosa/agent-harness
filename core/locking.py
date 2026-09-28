"""Cross-process exclusive lock on a sidecar `<path>.lock` file (msvcrt on Windows, fcntl elsewhere)."""
from __future__ import annotations

import os
from contextlib import contextmanager
from pathlib import Path
from typing import Iterator


@contextmanager
def file_lock(path: Path) -> Iterator[None]:
    fd = os.open(path.with_name(path.name + ".lock"), os.O_RDWR | os.O_CREAT, 0o600)
    try:
        if os.name == "nt":
            import msvcrt
            os.lseek(fd, 0, os.SEEK_SET)
            msvcrt.locking(fd, msvcrt.LK_LOCK, 1)  # retries for up to ~10 s, then raises OSError
            try:
                yield
            finally:
                os.lseek(fd, 0, os.SEEK_SET)
                msvcrt.locking(fd, msvcrt.LK_UNLCK, 1)
        else:
            import fcntl
            fcntl.flock(fd, fcntl.LOCK_EX)
            try:
                yield
            finally:
                fcntl.flock(fd, fcntl.LOCK_UN)
    finally:
        os.close(fd)
