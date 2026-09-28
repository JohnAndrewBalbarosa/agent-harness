"""Deterministic token-usage report from each agent's own records (no estimates).

Claude: ~/.claude/projects/**/*.jsonl, deduplicated by message.id; tool results measured in bytes (bytes, not tokens).
Codex:  <CODEX_HOME>/state_5.sqlite threads + each rollout's final cumulative `token_count` (read-only).
Cache reads dominate long sessions: the whole context is re-sent on every request.
"""
from __future__ import annotations

import collections
import json
import sqlite3
from pathlib import Path, PureWindowsPath
from typing import Mapping

TOP = 8


def _lines(path: Path):
    try:
        with path.open(encoding="utf-8", errors="replace") as handle:
            yield from handle
    except OSError:
        return


def claude(projects: Path) -> dict:
    seen: set[str] = set()
    totals = collections.Counter()
    by_project = collections.Counter()
    tool_bytes: dict[str, collections.Counter] = collections.defaultdict(collections.Counter)
    for transcript in projects.rglob("*.jsonl"):
        project = transcript.relative_to(projects).parts[0].rsplit("-", 1)[-1] or "home"
        names: dict[str, str] = {}
        for line in _lines(transcript):
            try:
                entry = json.loads(line)
            except ValueError:
                continue
            if isinstance(entry.get("cwd"), str) and entry["cwd"]:
                project = PureWindowsPath(entry["cwd"]).name or project
            message = entry.get("message") if isinstance(entry.get("message"), dict) else {}
            for part in message.get("content") if isinstance(message.get("content"), list) else []:
                if not isinstance(part, dict):
                    continue
                if part.get("type") == "tool_use":
                    names[str(part.get("id"))] = str(part.get("name", "?"))
                elif part.get("type") == "tool_result":
                    counter = tool_bytes[names.get(str(part.get("tool_use_id")), "?")]
                    counter["calls"] += 1
                    counter["bytes"] += len(json.dumps(part.get("content"), ensure_ascii=False))
            usage, mid = message.get("usage"), message.get("id")
            if entry.get("type") != "assistant" or not isinstance(usage, dict) or not mid or mid in seen:
                continue
            seen.add(mid)
            row = {"input": usage.get("input_tokens") or 0, "cache_read": usage.get("cache_read_input_tokens") or 0,
                   "cache_create": usage.get("cache_creation_input_tokens") or 0, "output": usage.get("output_tokens") or 0}
            totals.update(row)
            by_project[project] += sum(row.values())
    return {"requests": len(seen), "totals": {k: totals[k] for k in ("input", "cache_read", "cache_create", "output")},
            "by_project": dict(by_project), "tool_result_bytes": {k: dict(v) for k, v in tool_bytes.items()}}


def _rollout_totals(path: Path) -> tuple[dict, int, int] | None:
    last, seen, max_context = None, set(), 0
    for line in _lines(path):
        if '"token_count"' not in line:
            continue
        try:
            info = json.loads(line)["payload"]["info"]
        except (ValueError, KeyError, TypeError):
            continue
        if not isinstance(info, dict):
            continue
        total = info.get("total_token_usage") or {}
        if total.get("total_tokens") in seen:
            continue
        seen.add(total.get("total_tokens"))
        last = total
        max_context = max(max_context, int((info.get("last_token_usage") or {}).get("input_tokens") or 0))
    return (last, len(seen), max_context) if last else None


def codex(homes: Mapping[str, Path]) -> dict | None:
    totals = collections.Counter()
    by_project = collections.Counter()
    max_context = 0
    found = False
    for home in homes.values():
        database = home / "state_5.sqlite"
        if not database.exists():
            continue
        found = True
        connection = sqlite3.connect(f"file:{database}?mode=ro", uri=True)
        try:
            rows = connection.execute("select rollout_path, cwd from threads where tokens_used > 0").fetchall()
        finally:
            connection.close()
        for rollout, cwd in rows:
            result = _rollout_totals(Path(rollout))
            if not result:
                continue
            last, requests, context = result
            cached = int(last.get("cached_input_tokens") or 0)
            inputs = int(last.get("input_tokens") or 0)
            totals.update({"uncached": inputs - cached, "cached": cached, "output": int(last.get("output_tokens") or 0),
                           "reasoning": int(last.get("reasoning_output_tokens") or 0), "requests": requests})
            by_project[PureWindowsPath(cwd).name or str(cwd)] += inputs
            max_context = max(max_context, context)
    if not found:
        return None
    return {"totals": {k: totals[k] for k in ("uncached", "cached", "output", "reasoning", "requests")},
            "by_project": dict(by_project), "max_context": max_context}


def _top(counter: Mapping[str, int], total: int) -> list[str]:
    return [f"    {v / total * 100:5.1f}%  {v:>14,}  {k}" for k, v in sorted(counter.items(), key=lambda kv: -kv[1])[:TOP]]


def render(data: Mapping[str, dict | None]) -> str:
    out: list[str] = []
    claude_data = data.get("claude")
    if claude_data and claude_data["requests"]:
        t = claude_data["totals"]
        total = sum(t.values()) or 1
        out += [f"Claude: {claude_data['requests']:,} requests, {total:,} tokens",
                f"  cache_read {t['cache_read'] / total * 100:.1f}%  cache_create {t['cache_create'] / total * 100:.1f}%  "
                f"input {t['input']:,}  output {t['output']:,}", "  by project:"] + _top(claude_data["by_project"], total)
        tools = {k: v.get("bytes", 0) for k, v in claude_data["tool_result_bytes"].items()}
        if tools:
            out += ["  tool results fed back (bytes):"] + _top(tools, sum(tools.values()) or 1)
    codex_data = data.get("codex")
    if codex_data and codex_data["totals"]["requests"]:
        t = codex_data["totals"]
        inputs = (t["uncached"] + t["cached"]) or 1
        out += [f"Codex: {t['requests']:,} requests, input {inputs:,} (cached {t['cached'] / inputs * 100:.1f}%), "
                f"output {t['output']:,} (reasoning {t['reasoning']:,}), max context {codex_data['max_context']:,}",
                "  by project (input):"] + _top(codex_data["by_project"], inputs)
    return "\n".join(out) or "no usage records found"
