import json
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "hub"))
from benchmark import evaluate, load_catalog


class BenchmarkTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.catalog = load_catalog(ROOT / "benchmarks" / "catalog" / "hybrid-2026.json")

    def test_catalog_has_unique_valid_definitions(self):
        self.assertEqual(13, len(self.catalog["benchmarks"]))
        self.assertEqual("non-inferiority", self.catalog["defaults"]["mode"])
        self.assertEqual(5, len([item for item in self.catalog["benchmarks"] if item["category"] == "delivery"]))

    def test_small_sample_is_never_gated(self):
        result = evaluate({"benchmark_id": "agent.acceptance_rate", "candidate": [1] * 29, "baseline": [1] * 30}, self.catalog)
        self.assertEqual("insufficient_data", result["decision"])
        self.assertEqual("open", result["gate"])

    def test_confirmed_regression_blocks(self):
        result = evaluate({"benchmark_id": "agent.acceptance_rate", "candidate": [0] * 30, "baseline": [1] * 30}, self.catalog)
        self.assertEqual("regression", result["decision"])
        self.assertEqual("blocked", result["gate"])

    def test_superiority_failure_without_regression_is_inconclusive(self):
        values = [1] * 24 + [0] * 6
        result = evaluate({"benchmark_id": "agent.acceptance_rate", "mode": "superiority", "candidate": values, "baseline": values}, self.catalog)
        self.assertIn(result["decision"], {"inconclusive", "pass"})

    def test_superiority_regression_still_blocks(self):
        result = evaluate({"benchmark_id": "agent.acceptance_rate", "mode": "superiority", "candidate": [0] * 30, "target": 0.9}, self.catalog)
        self.assertEqual("regression", result["decision"])
        self.assertEqual("blocked", result["gate"])

    def test_continuous_bootstrap_returns_bounded_decision(self):
        result = evaluate({"benchmark_id": "agent.completion_latency_ms", "candidate": [90 + index % 3 for index in range(30)], "target": 100}, self.catalog)
        self.assertEqual(30, result["candidate_samples"])
        self.assertIn(result["decision"], {"pass", "inconclusive", "regression"})
        self.assertEqual(2, len(result["confidence_interval"]))


if __name__ == "__main__":
    unittest.main()
