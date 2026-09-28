from __future__ import annotations

import importlib.util
from pathlib import Path


ROUTER_PATH = Path(__file__).resolve().parents[1] / "router.py"
SPEC = importlib.util.spec_from_file_location("global_hook_router", ROUTER_PATH)
assert SPEC and SPEC.loader
router = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(router)


def test_herdr_pane_owns_notifications() -> None:
    assert router.notification_authority({"HERDR_ENV": "1", "HERDR_PANE_ID": "w1:p1"}) == "herdr"


def test_outside_session_uses_legacy_fallback() -> None:
    assert router.notification_authority({}) == "legacy-fallback"


def test_incomplete_herdr_environment_uses_fallback() -> None:
    assert router.notification_authority({"HERDR_ENV": "1"}) == "legacy-fallback"
