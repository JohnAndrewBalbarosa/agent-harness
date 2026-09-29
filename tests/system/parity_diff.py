"""JSON parity comparison between the reference Node hub and the Rust hub.

Legend:
  mismatch  unexpected difference; fails the run.
  expected  accepted deviation (row order among ties / collation, timestamp precision);
            counted and reported, never fails the run.

Privacy: responses carry raw prompts. Only probe labels, JSON paths, and value
type/length descriptors are ever printed — never values.
"""

from __future__ import annotations

import calendar
import re
from collections import Counter
from dataclasses import dataclass, field
from typing import Any

MODE_AUTO = "auto"
MODE_EXACT = "exact"
MODE_DB_ERROR = "db_error"
MODE_FRAMEWORK_ERROR = "framework_error"

EXPECTED_ROW_ORDER = "row_order"
EXPECTED_TIMESTAMP_PRECISION = "timestamp_precision"

MASKED_INSERT_TIME_KEY = "created_at"
TIMESTAMP_PATTERN = re.compile(r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})(?:\.(\d+))?Z$")
IDENTIFIER_KEY = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


class _Absent:
    """Marks an object key present on only one side."""


ABSENT = _Absent()


@dataclass(frozen=True)
class HubResponse:
    status: int
    body: Any


@dataclass
class Diff:
    mismatches: list[tuple[str, str, str]] = field(default_factory=list)
    expected: list[str] = field(default_factory=list)

    @staticmethod
    def mismatch(path: str, node: Any, rust: Any) -> "Diff":
        return Diff(mismatches=[(path, describe(node), describe(rust))])

    def merge(self, other: "Diff") -> "Diff":
        self.mismatches.extend(other.mismatches)
        self.expected.extend(other.expected)
        return self


class MismatchPrinter:
    """Prints at most `limit` mismatch lines for the whole run, then one overflow count."""

    def __init__(self, limit: int) -> None:
        self.limit = limit
        self.printed = 0
        self.suppressed = 0

    def emit(self, phase: str, label: str, path: str, node_desc: str, rust_desc: str) -> None:
        if self.printed >= self.limit:
            self.suppressed += 1
            return
        self.printed += 1
        print(f"[phase {phase}] MISMATCH {label} {path}: node={node_desc} rust={rust_desc}", flush=True)

    def print_overflow(self) -> None:
        if self.suppressed:
            print(f"... {self.suppressed} more mismatch lines suppressed", flush=True)


class ParityReport:
    """Per-phase counters: compared requests, unexpected mismatches, expected deviations."""

    def __init__(self, phase: str, mask_created_at: bool, printer: MismatchPrinter) -> None:
        self.phase = phase
        self.mask_created_at = mask_created_at
        self.printer = printer
        self.routes = 0
        self.mismatch_count = 0
        self.expected: Counter[str] = Counter()
        self.error: str | None = None

    # Mental model:
    # status must match → pick comparison mode → deep diff → record mismatches + expected deviations
    def compare(self, label: str, node: HubResponse, rust: HubResponse, mode: str) -> None:
        self.routes += 1
        if node.status != rust.status:
            self.record(label, "$status", f"status {node.status}", f"status {rust.status}")
            return

        diff = diff_by_mode(resolve_mode(mode, node), node.body, rust.body, self.mask_created_at)
        for path, node_desc, rust_desc in diff.mismatches:
            self.record(label, path, node_desc, rust_desc)
        self.expected.update(diff.expected)

    def require_reference_success(self, label: str, node: HubResponse) -> None:
        """A stream batch the reference hub rejects means the test data itself is wrong."""
        if node.status != 200:
            self.record(label, "$reference", f"status {node.status}", "expected 200 from reference hub")

    def record(self, label: str, path: str, node_desc: str, rust_desc: str) -> None:
        self.mismatch_count += 1
        self.printer.emit(self.phase, label, path, node_desc, rust_desc)

    def summary(self) -> dict[str, Any]:
        result: dict[str, Any] = {
            "routes": self.routes,
            "mismatches": self.mismatch_count,
            "expected": sum(self.expected.values()),
            "expectedByKind": dict(sorted(self.expected.items())),
        }
        if self.error:
            result["error"] = self.error
        return result

    @property
    def ok(self) -> bool:
        return self.error is None and self.mismatch_count == 0


def resolve_mode(mode: str, node: HubResponse) -> str:
    """Auto: Fastify 5xx bodies carry driver-specific messages; everything else must match exactly."""
    if mode != MODE_AUTO:
        return mode
    return MODE_FRAMEWORK_ERROR if node.status >= 500 else MODE_EXACT


def diff_by_mode(mode: str, node: Any, rust: Any, mask: bool) -> Diff:
    if mode == MODE_DB_ERROR:
        return diff_error_shape(node, rust, free_text_key="error", mask=mask)
    if mode == MODE_FRAMEWORK_ERROR:
        return diff_error_shape(node, rust, free_text_key="message", mask=mask)
    return diff_values(node, rust, "$", mask)


def diff_error_shape(node: Any, rust: Any, free_text_key: str, mask: bool) -> Diff:
    """Same keys and values except one free-text key, which must be a non-empty string on both sides."""
    if not isinstance(node, dict) or not isinstance(rust, dict):
        return diff_values(node, rust, "$", mask)

    diff = diff_values(without_key(node, free_text_key), without_key(rust, free_text_key), "$", mask)
    node_text = node.get(free_text_key, ABSENT)
    rust_text = rust.get(free_text_key, ABSENT)
    if not is_non_empty_string(node_text) or not is_non_empty_string(rust_text):
        diff.merge(Diff.mismatch(f"$.{free_text_key}", node_text, rust_text))
    return diff


def without_key(document: dict, key: str) -> dict:
    return {name: value for name, value in document.items() if name != key}


def is_non_empty_string(value: Any) -> bool:
    return isinstance(value, str) and bool(value)


def diff_values(node: Any, rust: Any, path: str, mask: bool) -> Diff:
    if is_number(node) and is_number(rust):
        return Diff() if float(node) == float(rust) else Diff.mismatch(path, node, rust)
    if isinstance(node, str) and isinstance(rust, str):
        return diff_strings(node, rust, path)
    if isinstance(node, dict) and isinstance(rust, dict):
        return diff_objects(node, rust, path, mask)
    if isinstance(node, list) and isinstance(rust, list):
        return diff_arrays(node, rust, path, mask)
    if type(node) is type(rust) and node == rust:
        return Diff()
    return Diff.mismatch(path, node, rust)


def is_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def diff_strings(node: str, rust: str, path: str) -> Diff:
    if node == rust:
        return Diff()
    if TIMESTAMP_PATTERN.match(node) and TIMESTAMP_PATTERN.match(rust):
        return diff_timestamps(node, rust, path)
    return Diff.mismatch(path, node, rust)


def diff_timestamps(node: str, rust: str, path: str) -> Diff:
    node_ms = epoch_millis(node)
    rust_ms = epoch_millis(rust)
    if node_ms == rust_ms:
        return Diff(expected=[EXPECTED_TIMESTAMP_PRECISION])
    return Diff.mismatch(path, node, rust)


def epoch_millis(timestamp: str) -> int:
    match = TIMESTAMP_PATTERN.match(timestamp)
    assert match is not None
    whole_seconds = calendar.timegm(
        (int(match[1][0:4]), int(match[1][5:7]), int(match[1][8:10]),
         int(match[1][11:13]), int(match[1][14:16]), int(match[1][17:19]), 0, 0, 0)
    )
    fraction = ((match[2] or "") + "000")[:3]
    return whole_seconds * 1000 + int(fraction)


def diff_objects(node: dict, rust: dict, path: str, mask: bool) -> Diff:
    diff = Diff()
    for key in sorted(set(node) | set(rust)):
        if mask and key == MASKED_INSERT_TIME_KEY:
            continue
        child_path = object_child_path(path, key)
        diff.merge(diff_values(node.get(key, ABSENT), rust.get(key, ABSENT), child_path, mask))
    return diff


def object_child_path(path: str, key: str) -> str:
    return f"{path}.{key}" if IDENTIFIER_KEY.match(key) else f"{path}[{key!r}]"


# Mental model:
# ordered element-by-element diff → if that fails, try pairing rows as a multiset
# (ties / collation order) → only when pairing also fails report the ordered mismatches
def diff_arrays(node: list, rust: list, path: str, mask: bool) -> Diff:
    ordered = ordered_array_diff(node, rust, path, mask)
    if not ordered.mismatches:
        return ordered

    paired = multiset_pairing(node, rust, mask)
    if paired is not None:
        return paired.merge(Diff(expected=[EXPECTED_ROW_ORDER]))
    return ordered


def ordered_array_diff(node: list, rust: list, path: str, mask: bool) -> Diff:
    if len(node) != len(rust):
        return Diff.mismatch(path, node, rust)
    diff = Diff()
    for index, (node_item, rust_item) in enumerate(zip(node, rust)):
        diff.merge(diff_values(node_item, rust_item, f"{path}[{index}]", mask))
    return diff


def multiset_pairing(node: list, rust: list, mask: bool) -> Diff | None:
    """Greedy one-to-one pairing of equal rows; None when some row has no partner."""
    if len(node) != len(rust):
        return None
    unmatched = list(range(len(rust)))
    collected = Diff()
    for node_item in node:
        partner = find_partner(node_item, rust, unmatched, mask)
        if partner is None:
            return None
        index, pair_diff = partner
        unmatched.remove(index)
        collected.merge(pair_diff)
    return collected


def find_partner(node_item: Any, rust: list, unmatched: list[int], mask: bool) -> tuple[int, Diff] | None:
    for index in unmatched:
        pair_diff = diff_values(node_item, rust[index], "$", mask)
        if not pair_diff.mismatches:
            return index, pair_diff
    return None


def describe(value: Any) -> str:
    """Type/length only — never the value itself."""
    if value is ABSENT:
        return "absent"
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "bool"
    if is_number(value):
        return "number"
    if isinstance(value, str):
        return f"string(len={len(value)})"
    if isinstance(value, list):
        return f"array(len={len(value)})"
    if isinstance(value, dict):
        return f"object(keys={len(value)})"
    return type(value).__name__
