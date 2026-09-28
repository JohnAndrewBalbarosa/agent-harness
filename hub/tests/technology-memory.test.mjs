import assert from "node:assert/strict";
import test from "node:test";

import { rankCandidates, technologyKey } from "../src/technology-memory.mjs";

test("business fit outranks GitHub stars and test status is not a rank input", () => {
  const ranked = rankCandidates([
    { name: "Popular Generic", fitTier: 1, stars: 100000, testStatus: "tested" },
    { name: "Exact Logic", fitTier: 3, stars: 12, testStatus: "untested" },
  ]);
  assert.deepEqual(ranked.map(item => item.name), ["Exact Logic", "Popular Generic"]);
  assert.deepEqual(ranked.map(item => item.testStatus), ["untested", "tested"]);
});

test("stars break ties only after fit and ineligible candidates are removed", () => {
  const ranked = rankCandidates([
    { name: "Low", fitTier: 2, stars: 5 },
    { name: "Excluded", fitTier: 3, stars: 500, eligible: false },
    { name: "High", fitTier: 2, stars: 50 },
  ]);
  assert.deepEqual(ranked.map(item => item.name), ["High", "Low"]);
  assert.equal(ranked[0].rank, 1);
});

test("technology keys are stable", () => {
  assert.equal(technologyKey("  MongoDB Community  "), "mongodb-community");
});
