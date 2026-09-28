import { MongoClient } from "mongodb";

const uri = process.env.OBS_MONGODB_URI || "mongodb://127.0.0.1:27017/?directConnection=true";
const databaseName = process.env.OBS_MONGODB_DATABASE || "codex_agent_memory";
const client = new MongoClient(uri, { serverSelectionTimeoutMS: 2500, connectTimeoutMS: 2500 });
let databasePromise;

export class TechnologyMemoryUnavailable extends Error {}

const bounded = (value, max = 500) => String(value ?? "").trim().slice(0, max);
export const technologyKey = value => bounded(value, 240).toLowerCase().replace(/\s+/g, "-");

async function database() {
  if (!databasePromise) {
    databasePromise = client.connect().then(async connected => {
      const db = connected.db(databaseName);
      await Promise.all([
        db.collection("technology_memory").createIndex({ key: 1 }, { unique: true }),
        db.collection("project_components").createIndex({ projectId: 1, technologyKey: 1, selectedAt: -1 }),
        db.collection("discovery_cache").createIndex({ queryKey: 1, projectId: 1 }, { unique: true }),
        db.collection("discovery_cache").createIndex({ expiresAt: 1 }, { expireAfterSeconds: 0 }),
      ]);
      return db;
    }).catch(error => {
      databasePromise = undefined;
      throw new TechnologyMemoryUnavailable(bounded(error?.message || error, 300));
    });
  }
  return databasePromise;
}

export async function initializeTechnologyMemory() {
  await database();
}

export function rankCandidates(candidates) {
  return [...candidates].map((candidate, index) => ({
    ...candidate,
    key: technologyKey(candidate.key || candidate.name),
    fitTier: Math.max(0, Math.min(3, Number(candidate.fitTier) || 0)),
    stars: Math.max(0, Number(candidate.stars) || 0),
    originalOrder: index,
  })).filter(candidate => candidate.key && candidate.eligible !== false)
    .sort((left, right) => right.fitTier - left.fitTier || right.stars - left.stars || left.originalOrder - right.originalOrder)
    .map(({ originalOrder, ...candidate }, rank) => ({ ...candidate, rank: rank + 1 }));
}

export async function applyTechnologyOperation(operation) {
  const db = await database();
  const d = operation.data;
  const occurredAt = new Date(operation.occurredAt);
  if (operation.kind === "technology.remember") {
    const key = technologyKey(d.key || d.name);
    if (!key || !["tested", "untested"].includes(d.status)) throw new Error("invalid technology memory record");
    await db.collection("technology_memory").updateOne({ key }, {
      $set: {
        key, name: bounded(d.name || key, 160), status: d.status,
        aliases: Array.isArray(d.aliases) ? d.aliases.map(value => bounded(value, 160)).filter(Boolean).slice(0, 20) : [],
        sourceUrls: Array.isArray(d.sourceUrls) ? d.sourceUrls.map(value => bounded(value, 1000)).filter(Boolean).slice(0, 20) : [],
        lastUsedAt: occurredAt, updatedAt: occurredAt,
        ...(d.status === "tested" ? { testedAt: occurredAt } : {}),
      },
      $setOnInsert: { firstUsedAt: occurredAt, createdAt: occurredAt },
    }, { upsert: true });
    return;
  }
  if (operation.kind === "technology.mark-tested") {
    const key = technologyKey(d.key);
    const result = await db.collection("technology_memory").updateOne({ key }, { $set: { status: "tested", testedAt: occurredAt, updatedAt: occurredAt } });
    if (!result.matchedCount) throw new Error("technology is not remembered yet");
    return;
  }
  if (operation.kind === "reuse.cache") {
    const projectId = bounded(d.projectId || "global", 80);
    const queryKey = technologyKey(d.queryKey || d.businessNeed);
    const ranked = rankCandidates(Array.isArray(d.candidates) ? d.candidates : []);
    const known = await db.collection("technology_memory").find({ key: { $in: ranked.map(item => item.key) } }).toArray();
    const status = new Map(known.map(item => [item.key, item.status]));
    const candidates = ranked.map(item => ({ ...item, testStatus: status.get(item.key) || "unknown" }));
    await db.collection("discovery_cache").updateOne({ projectId, queryKey }, { $set: {
      projectId, queryKey, businessNeed: bounded(d.businessNeed, 1000), candidates,
      searchedAt: occurredAt, updatedAt: occurredAt,
      expiresAt: new Date(occurredAt.getTime() + 7 * 24 * 60 * 60 * 1000),
    } }, { upsert: true });
    return;
  }
  if (operation.kind === "reuse.record") {
    const technology = technologyKey(d.technologyKey || d.name);
    if (!technology) throw new Error("technology key is required");
    await db.collection("project_components").updateOne({ usageId: bounded(d.usageId, 80) }, { $setOnInsert: {
      usageId: bounded(d.usageId, 80), projectId: bounded(d.projectId || "global", 80), technologyKey: technology,
      name: bounded(d.name || technology, 160), businessNeed: bounded(d.businessNeed, 1000),
      version: bounded(d.version, 160), commit: bounded(d.commit, 80), integrationMode: bounded(d.integrationMode || "package", 40),
      sourceUrl: bounded(d.sourceUrl, 1000), repositoryUrl: bounded(d.repositoryUrl, 1000), license: bounded(d.license, 80),
      fitTier: Math.max(0, Math.min(3, Number(d.fitTier) || 0)), stars: Math.max(0, Number(d.stars) || 0),
      verificationOutcome: bounded(d.verificationOutcome || "pending", 40), promptId: bounded(d.promptId, 80),
      executionId: bounded(d.executionId, 80), codeVersionId: bounded(d.codeVersionId, 80), selectedAt: occurredAt,
    } }, { upsert: true });
    return;
  }
  throw new Error(`unsupported technology operation: ${operation.kind}`);
}

export async function listTechnologies(key = "") {
  const db = await database();
  const filter = key ? { key: technologyKey(key) } : {};
  return db.collection("technology_memory").find(filter, { projection: { _id: 0 } }).sort({ name: 1 }).limit(500).toArray();
}

export async function listProjectComponents(projectId) {
  const db = await database();
  return db.collection("project_components").find({ projectId: bounded(projectId || "global", 80) }, { projection: { _id: 0 } }).sort({ selectedAt: -1 }).limit(500).toArray();
}

export async function getDiscovery(projectId, queryKey) {
  const db = await database();
  return db.collection("discovery_cache").findOne({ projectId: bounded(projectId || "global", 80), queryKey: technologyKey(queryKey) }, { projection: { _id: 0 } });
}
