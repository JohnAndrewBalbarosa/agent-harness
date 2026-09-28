"""`services` consumer (session.start): detach the service bootstrap and return at once."""
from __future__ import annotations

import os
import sys

from core.services.bootstrap import ROOT, launch_detached


def main() -> int:
    if sys.stdin is not None:
        sys.stdin.buffer.read()
    try:
        launch_detached([sys.executable, "-m", "core.services.bootstrap"], ROOT)
    except OSError as error:
        print(f"services consumer: {type(error).__name__}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    os.environ.setdefault("PYTHONUTF8", "1")
    sys.exit(main())
