BEGIN IMMEDIATE;
-- Markdown metadata only: bodies remain in their source files and native agent records.
CREATE TABLE IF NOT EXISTS markdown_documents (
  document_id INTEGER PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  relative_path TEXT NOT NULL CHECK(length(relative_path) BETWEEN 1 AND 2048),
  UNIQUE(project_id,relative_path)
);

CREATE TABLE IF NOT EXISTS markdown_document_versions (
  version_id INTEGER PRIMARY KEY,
  document_id INTEGER NOT NULL REFERENCES markdown_documents(document_id) ON DELETE CASCADE,
  content_hash BLOB NOT NULL CHECK(length(content_hash)=32),
  byte_size INTEGER NOT NULL CHECK(byte_size>=0 AND byte_size<=2097152),
  UNIQUE(document_id,content_hash)
);

CREATE TABLE IF NOT EXISTS markdown_context_batches (
  batch_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  native_session TEXT NOT NULL CHECK(length(native_session)<=240),
  native_turn TEXT CHECK(native_turn IS NULL OR length(native_turn)<=240),
  agent TEXT NOT NULL CHECK(length(agent)<=80),
  instance TEXT NOT NULL CHECK(length(instance)<=80),
  context_epoch TEXT NOT NULL CHECK(length(context_epoch)<=80),
  reason TEXT NOT NULL CHECK(length(reason)<=80),
  duration_ms REAL NOT NULL CHECK(duration_ms>=0),
  recorded_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS markdown_context_observations (
  batch_id TEXT NOT NULL REFERENCES markdown_context_batches(batch_id) ON DELETE CASCADE,
  version_id INTEGER NOT NULL REFERENCES markdown_document_versions(version_id),
  source TEXT NOT NULL CHECK(source IN ('explicit_read','tool_read','native_evidence')),
  coverage TEXT NOT NULL CHECK(coverage IN ('full','partial','unknown')),
  outcome TEXT NOT NULL CHECK(outcome IN ('emitted','observed','already_observed','budget_exhausted')),
  start_character INTEGER CHECK(start_character IS NULL OR start_character>=0),
  end_character INTEGER CHECK(end_character IS NULL OR end_character>=start_character),
  supplied_characters INTEGER NOT NULL CHECK(supplied_characters>=0 AND supplied_characters<=100000),
  PRIMARY KEY(batch_id,version_id)
);

CREATE INDEX IF NOT EXISTS markdown_context_batches_project_time ON markdown_context_batches(project_id,recorded_at);
CREATE INDEX IF NOT EXISTS markdown_context_observations_version ON markdown_context_observations(version_id);

INSERT OR IGNORE INTO schema_migrations(version) VALUES (8);
COMMIT;
