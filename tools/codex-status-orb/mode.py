"""Orb run mode. Headless keeps every behavior (sounds, session recovery, process checks, health) without a window."""
from __future__ import annotations

from typing import Mapping, Sequence


def headless_requested(argv: Sequence[str], env: Mapping[str, str]) -> bool:
    return "--headless" in argv or str(env.get("ORB_UI", "")).strip() == "0"
