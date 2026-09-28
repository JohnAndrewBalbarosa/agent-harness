from __future__ import annotations

import msvcrt
import time
from pathlib import Path

import obs


LOCK = Path(__file__).resolve().parent / "var" / "flush-spool.lock"
MAX_SECONDS = 120


def pending_count() -> int:
    conn = obs.db()
    try:
        return int(conn.execute("SELECT COUNT(*) FROM queue").fetchone()[0])
    finally:
        conn.close()


def main() -> int:
    LOCK.parent.mkdir(parents=True, exist_ok=True)
    with LOCK.open("a+b") as lock:
        try:
            lock.seek(0)
            msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
        except OSError:
            return 0
        deadline = time.monotonic() + MAX_SECONDS
        stalled_attempts = 0
        try:
            while time.monotonic() < deadline:
                before = pending_count()
                if before == 0:
                    break
                obs.flush()
                if pending_count() >= before:
                    stalled_attempts += 1
                    if stalled_attempts >= 5:
                        break
                    time.sleep(0.25)
                else:
                    stalled_attempts = 0
        finally:
            lock.seek(0)
            msvcrt.locking(lock.fileno(), msvcrt.LK_UNLCK, 1)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
