"""Pure merge helpers for agent config files. Each returns a new value; inputs are never mutated (spec R7)."""
from __future__ import annotations

import copy
from typing import Mapping

HARNESS_COMMAND_MARKERS = ("harness-hook", "global-hook-router", "rtk hook ")


def merge_env(settings: Mapping, managed: Mapping[str, str], previously_managed: set[str]) -> dict:
    """Set managed env keys in a Claude settings.json document; drop keys this harness managed before but no longer wants."""
    merged = copy.deepcopy(dict(settings))
    env = {k: v for k, v in dict(merged.get("env") or {}).items() if k not in previously_managed or k in managed}
    env.update(managed)
    merged["env"] = env
    return merged


def merge_block(text: str, block: str, begin: str, end: str) -> str:
    """Replace the text between begin/end markers (inclusive) or append the marked block."""
    section = f"{begin}\n{block}\n{end}"
    start = text.find(begin)
    stop = text.find(end, start + len(begin)) if start != -1 else -1
    if start != -1 and stop != -1:
        return text[:start] + section + text[stop + len(end):]
    separator = "" if not text else ("\n" if text.endswith("\n") else "\n\n")
    return f"{text}{separator}{section}\n"


def _is_harness(entry: Mapping) -> bool:
    return any(marker in str(hook.get("command", "")) for hook in entry.get("hooks", []) for marker in HARNESS_COMMAND_MARKERS)


def merge_codex_hooks(existing: Mapping, generated: Mapping) -> dict:
    """Codex hooks.json: generated harness entries first, then every existing non-harness entry, per event."""
    existing_hooks = dict(existing.get("hooks") or {})
    generated_hooks = dict(generated.get("hooks") or {})
    merged: dict = {}
    for event in list(generated_hooks) + [e for e in existing_hooks if e not in generated_hooks]:
        kept = [copy.deepcopy(entry) for entry in existing_hooks.get(event, []) if not _is_harness(entry)]
        entries = copy.deepcopy(generated_hooks.get(event, [])) + kept
        if entries:
            merged[event] = entries
    return {**copy.deepcopy(dict(existing)), "hooks": merged}
