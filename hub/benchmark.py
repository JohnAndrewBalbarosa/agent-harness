from __future__ import annotations

import json
import math
from pathlib import Path
from typing import Any


VALID_MODES = {"non-inferiority", "superiority"}
VALID_DIRECTIONS = {"higher", "lower"}
VALID_MEASURES = {"proportion", "continuous"}


def load_catalog(path: str | Path) -> dict[str, Any]:
    catalog = json.loads(Path(path).read_text(encoding="utf-8"))
    if catalog.get("schema_version") != 1 or not isinstance(catalog.get("benchmarks"), list):
        raise ValueError("unsupported benchmark catalog")
    seen: set[str] = set()
    for item in catalog["benchmarks"]:
        key = item.get("id")
        if not key or key in seen:
            raise ValueError(f"invalid or duplicate benchmark id: {key}")
        seen.add(key)
        if item.get("direction") not in VALID_DIRECTIONS or item.get("measure") not in VALID_MEASURES:
            raise ValueError(f"invalid benchmark definition: {key}")
    return catalog


def _definition(catalog: dict[str, Any], benchmark_id: str) -> dict[str, Any]:
    return next((item for item in catalog["benchmarks"] if item["id"] == benchmark_id), None) or (_ for _ in ()).throw(KeyError(benchmark_id))


def _median(values: list[float]) -> float:
    ordered = sorted(values)
    mid = len(ordered) // 2
    return ordered[mid] if len(ordered) % 2 else (ordered[mid - 1] + ordered[mid]) / 2


def evaluate(payload: dict[str, Any], catalog: dict[str, Any]) -> dict[str, Any]:
    definition = _definition(catalog, payload["benchmark_id"])
    mode = payload.get("mode", catalog.get("defaults", {}).get("mode", "non-inferiority"))
    if mode not in VALID_MODES:
        raise ValueError(f"invalid hypothesis mode: {mode}")
    candidate = [float(value) for value in payload.get("candidate", [])]
    baseline = [float(value) for value in payload.get("baseline", [])]
    minimum = int(payload.get("minimum_samples", catalog.get("defaults", {}).get("minimum_samples", 30)))
    result: dict[str, Any] = {
        "benchmark_id": definition["id"], "profile": payload.get("profile", "personal"), "mode": mode,
        "candidate_samples": len(candidate), "baseline_samples": len(baseline), "minimum_samples": minimum,
        "decision": "insufficient_data", "gate": "open", "alpha": float(payload.get("alpha", catalog.get("defaults", {}).get("alpha", 0.05))),
    }
    if len(candidate) < minimum or ("target" not in payload and len(baseline) < minimum):
        return result

    direction = definition["direction"]
    measure = definition["measure"]
    margin = float(payload.get("margin", definition.get("margin", 0)))
    if "target" in payload:  # the baseline default must not be evaluated eagerly: baseline may be empty
        target = float(payload["target"])
    else:
        target = sum(baseline) / len(baseline) if measure == "proportion" else _median(baseline)
    if definition.get("margin_kind") == "relative":
        margin = abs(target) * margin
    boundary = target - margin if direction == "higher" else target + margin
    if mode == "superiority":
        boundary = target + margin if direction == "higher" else target - margin

    if measure == "proportion":
        from scipy.stats import binomtest
        successes = round(sum(candidate))
        if direction == "lower":
            successes = len(candidate) - successes
            boundary = 1 - boundary
        test = binomtest(successes, len(candidate), min(1, max(0, boundary)), alternative="greater")
        # A one-sided ("greater") test yields a CI with high == 1, which can never show a regression; use two-sided.
        interval = binomtest(successes, len(candidate)).proportion_ci(confidence_level=1 - result["alpha"])
        passed = test.pvalue < result["alpha"]
        observed = sum(candidate) / len(candidate)
        if direction == "lower":
            ci = [1 - interval.high, 1 - interval.low]
        else:
            ci = [interval.low, interval.high]
    else:
        import numpy as np
        from scipy.stats import bootstrap
        values = np.asarray(candidate, dtype=float)
        boot = bootstrap((values,), np.median, confidence_level=1 - result["alpha"], random_state=1729, method="basic")
        ci = [float(boot.confidence_interval.low), float(boot.confidence_interval.high)]
        observed = float(np.median(values))
        passed = ci[0] >= boundary if direction == "higher" else ci[1] <= boundary

    regression_boundary = target - abs(margin) if direction == "higher" else target + abs(margin)
    regression = ci[1] < regression_boundary if direction == "higher" else ci[0] > regression_boundary
    result.update({"target": target, "margin": margin, "boundary": boundary, "observed": observed, "confidence_interval": ci})
    if passed:
        result["decision"] = "pass"
    elif regression:
        result.update({"decision": "regression", "gate": "blocked"})
    else:
        result["decision"] = "inconclusive"
    return result

