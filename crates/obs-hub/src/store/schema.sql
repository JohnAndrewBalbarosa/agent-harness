-- SQLite translation of observability-hub migrations 001-007 (Postgres 17).
-- Type mapping: uuid -> TEXT (lowercase), bytea -> BLOB, timestamptz -> TEXT (UTC, microseconds, 'Z'),
-- jsonb -> TEXT (json_valid), boolean -> INTEGER 0/1, smallint/integer/bigint -> INTEGER, double -> REAL.
-- Regex CHECKs (projects.slug, event_definitions.code, sha256 columns) are enforced in Rust validators.
-- Dropped on purpose: partitions, per-project schemas, project_schemas, one_day_log_rollups.

CREATE TABLE IF NOT EXISTS schema_migrations (
  version INTEGER PRIMARY KEY,
  applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z')
);

CREATE TABLE IF NOT EXISTS projects (
  project_id TEXT PRIMARY KEY,
  display_name TEXT NOT NULL CHECK (length(display_name) BETWEEN 1 AND 160),
  slug TEXT NOT NULL UNIQUE,
  status INTEGER NOT NULL DEFAULT 1 CHECK (status IN (0,1)),
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z'),
  last_seen_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z')
);

CREATE TABLE IF NOT EXISTS project_aliases (
  project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  alias_kind INTEGER NOT NULL CHECK (alias_kind IN (1,2,3)),
  alias_hash BLOB NOT NULL CHECK (length(alias_hash)=32),
  is_current INTEGER NOT NULL DEFAULT 1 CHECK (is_current IN (0,1)),
  first_seen_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z'),
  last_seen_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z'),
  PRIMARY KEY (project_id, alias_kind, alias_hash)
);

CREATE TABLE IF NOT EXISTS project_context_versions (
  context_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  version_no INTEGER NOT NULL CHECK (version_no>0),
  language_code TEXT NOT NULL DEFAULT 'tl-en' CHECK (length(language_code)<=16),
  vision_summary TEXT NOT NULL DEFAULT '' CHECK (length(vision_summary)<=2000),
  architecture_summary TEXT NOT NULL DEFAULT '' CHECK (length(architecture_summary)<=2000),
  current_goal_summary TEXT NOT NULL DEFAULT '' CHECK (length(current_goal_summary)<=2000),
  constraints TEXT NOT NULL DEFAULT '[]' CHECK (json_type(constraints)='array'),
  source_prompt_id TEXT,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z'),
  UNIQUE(project_id,version_no)
);

CREATE TABLE IF NOT EXISTS agent_instances (
  agent_instance_id TEXT PRIMARY KEY,
  instance_key TEXT NOT NULL UNIQUE CHECK (length(instance_key)<=80),
  source_kind INTEGER NOT NULL CHECK (source_kind IN (1,2,3)),
  display_name TEXT NOT NULL CHECK (length(display_name)<=160),
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z'),
  last_seen_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z')
);

CREATE TABLE IF NOT EXISTS sessions (
  session_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  agent_instance_id TEXT NOT NULL REFERENCES agent_instances(agent_instance_id),
  session_kind INTEGER NOT NULL CHECK (session_kind IN (1,2,3,4)),
  session_fingerprint BLOB NOT NULL CHECK (length(session_fingerprint)=32),
  status INTEGER NOT NULL DEFAULT 1 CHECK (status IN (1,2,3)),
  started_at TEXT NOT NULL,
  ended_at TEXT,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z'),
  UNIQUE(project_id,agent_instance_id,session_fingerprint)
);

CREATE TABLE IF NOT EXISTS prompt_nodes (
  prompt_id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
  parent_prompt_id TEXT REFERENCES prompt_nodes(prompt_id),
  ordinal INTEGER NOT NULL CHECK (ordinal>0),
  prompt_hmac BLOB NOT NULL CHECK (length(prompt_hmac)=32),
  language_code TEXT NOT NULL DEFAULT 'tl-en' CHECK (length(language_code)<=16),
  summary TEXT NOT NULL DEFAULT '' CHECK (length(summary)<=1200),
  intent TEXT NOT NULL DEFAULT '' CHECK (length(intent)<=1200),
  target_system TEXT NOT NULL DEFAULT '' CHECK (length(target_system)<=400),
  constraints TEXT NOT NULL DEFAULT '[]' CHECK (json_type(constraints)='array'),
  success_criteria TEXT NOT NULL DEFAULT '[]' CHECK (json_type(success_criteria)='array'),
  result_summary TEXT NOT NULL DEFAULT '' CHECK (length(result_summary)<=1200),
  status INTEGER NOT NULL DEFAULT 1 CHECK (status IN (1,2,3,4)),
  created_at TEXT NOT NULL,
  enriched_at TEXT,
  completed_at TEXT,
  raw_prompt TEXT NOT NULL DEFAULT '',
  raw_result TEXT NOT NULL DEFAULT '',
  UNIQUE(session_id,ordinal)
);

CREATE TABLE IF NOT EXISTS code_versions (
  code_version_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  commit_sha TEXT CHECK (commit_sha IS NULL OR length(commit_sha)<=64),
  branch_name TEXT CHECK (branch_name IS NULL OR length(branch_name)<=240),
  dirty_fingerprint BLOB,
  build_version TEXT CHECK (build_version IS NULL OR length(build_version)<=120),
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z'),
  UNIQUE(project_id,commit_sha,dirty_fingerprint)
);

CREATE TABLE IF NOT EXISTS executions (
  execution_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  session_id TEXT REFERENCES sessions(session_id) ON DELETE SET NULL,
  prompt_id TEXT REFERENCES prompt_nodes(prompt_id) ON DELETE SET NULL,
  parent_execution_id TEXT REFERENCES executions(execution_id),
  code_version_id TEXT REFERENCES code_versions(code_version_id),
  execution_kind INTEGER NOT NULL CHECK (execution_kind IN (1,2,3,4,5)),
  attempt_no INTEGER NOT NULL DEFAULT 1 CHECK (attempt_no>0),
  trace_id BLOB NOT NULL CHECK (length(trace_id)=16),
  status INTEGER NOT NULL DEFAULT 1 CHECK (status IN (1,2,3,4)),
  outcome INTEGER CHECK (outcome IN (1,2,3)),
  started_at TEXT NOT NULL,
  ended_at TEXT,
  duration_ms INTEGER CHECK (duration_ms IS NULL OR duration_ms>=0),
  result_summary TEXT NOT NULL DEFAULT '' CHECK (length(result_summary)<=1200)
);

CREATE TABLE IF NOT EXISTS change_attributions (
  attribution_id TEXT PRIMARY KEY,
  execution_id TEXT NOT NULL REFERENCES executions(execution_id) ON DELETE CASCADE,
  code_version_id TEXT REFERENCES code_versions(code_version_id) ON DELETE CASCADE,
  artifact_kind INTEGER NOT NULL CHECK (artifact_kind IN (1,2,3,4,5)),
  locator_hash BLOB NOT NULL CHECK (length(locator_hash)=32),
  locator_label TEXT NOT NULL DEFAULT '' CHECK (length(locator_label)<=500),
  content_hash BLOB,
  metadata TEXT NOT NULL DEFAULT '{}' CHECK (json_type(metadata)='object'),
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z')
);

CREATE TABLE IF NOT EXISTS components (
  component_id INTEGER PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  component_kind INTEGER NOT NULL DEFAULT 1,
  name TEXT NOT NULL CHECK (length(name)<=160),
  UNIQUE(project_id,name)
);

CREATE TABLE IF NOT EXISTS event_definitions (
  event_definition_id INTEGER PRIMARY KEY,
  code TEXT NOT NULL UNIQUE,
  category INTEGER NOT NULL CHECK (category BETWEEN 1 AND 16),
  default_severity INTEGER NOT NULL CHECK (default_severity BETWEEN 1 AND 5),
  description TEXT NOT NULL DEFAULT '' CHECK (length(description)<=500)
);

CREATE TABLE IF NOT EXISTS events (
  event_id TEXT NOT NULL,
  execution_id TEXT NOT NULL REFERENCES executions(execution_id) ON DELETE CASCADE,
  component_id INTEGER REFERENCES components(component_id),
  event_definition_id INTEGER NOT NULL REFERENCES event_definitions(event_definition_id),
  sequence_no INTEGER NOT NULL CHECK (sequence_no>=0),
  severity INTEGER NOT NULL CHECK (severity BETWEEN 1 AND 5),
  outcome INTEGER CHECK (outcome IN (1,2,3)),
  trace_id BLOB NOT NULL CHECK (length(trace_id)=16),
  span_id BLOB CHECK (span_id IS NULL OR length(span_id)=8),
  parent_span_id BLOB CHECK (parent_span_id IS NULL OR length(parent_span_id)=8),
  message_summary TEXT NOT NULL DEFAULT '' CHECK (length(message_summary)<=500),
  attributes TEXT NOT NULL DEFAULT '{}' CHECK (json_type(attributes)='object'),
  occurred_at TEXT NOT NULL,
  ingested_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z'),
  raw_payload TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(raw_payload)),
  PRIMARY KEY(event_id,occurred_at),
  UNIQUE(execution_id,sequence_no,occurred_at)
);

CREATE TABLE IF NOT EXISTS error_groups (
  error_group_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  fingerprint BLOB NOT NULL CHECK (length(fingerprint)=32),
  exception_class TEXT NOT NULL DEFAULT '' CHECK (length(exception_class)<=240),
  normalized_summary TEXT NOT NULL DEFAULT '' CHECK (length(normalized_summary)<=500),
  first_seen_at TEXT NOT NULL,
  last_seen_at TEXT NOT NULL,
  occurrence_count INTEGER NOT NULL DEFAULT 1,
  status INTEGER NOT NULL DEFAULT 1 CHECK (status IN (1,2,3)),
  UNIQUE(project_id,fingerprint)
);

CREATE TABLE IF NOT EXISTS error_occurrences (
  event_id TEXT NOT NULL,
  event_occurred_at TEXT NOT NULL,
  error_group_id TEXT NOT NULL REFERENCES error_groups(error_group_id) ON DELETE CASCADE,
  handled INTEGER NOT NULL CHECK (handled IN (0,1)),
  retryable INTEGER NOT NULL CHECK (retryable IN (0,1)),
  stack_hash BLOB,
  top_frame TEXT NOT NULL DEFAULT '' CHECK (length(top_frame)<=500),
  PRIMARY KEY(event_id,event_occurred_at),
  FOREIGN KEY(event_id,event_occurred_at) REFERENCES events(event_id,occurred_at) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS event_payloads (
  event_id TEXT NOT NULL,
  event_occurred_at TEXT NOT NULL,
  encoding INTEGER NOT NULL DEFAULT 1,
  redaction_version INTEGER NOT NULL,
  compressed_payload BLOB NOT NULL,
  expires_at TEXT NOT NULL,
  PRIMARY KEY(event_id,event_occurred_at),
  FOREIGN KEY(event_id,event_occurred_at) REFERENCES events(event_id,occurred_at) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS daily_rollups (
  project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  day TEXT NOT NULL,
  event_definition_id INTEGER NOT NULL REFERENCES event_definitions(event_definition_id),
  severity INTEGER NOT NULL,
  outcome INTEGER,
  event_count INTEGER NOT NULL,
  duration_sum_ms INTEGER NOT NULL DEFAULT 0,
  duration_max_ms INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(project_id,day,event_definition_id,severity,outcome)
);

CREATE INDEX IF NOT EXISTS sessions_project_time ON sessions(project_id,started_at DESC);
CREATE INDEX IF NOT EXISTS prompts_session_time ON prompt_nodes(session_id,created_at DESC);
CREATE INDEX IF NOT EXISTS executions_prompt_time ON executions(prompt_id,started_at DESC);
CREATE INDEX IF NOT EXISTS executions_project_time ON executions(project_id,started_at DESC);
CREATE INDEX IF NOT EXISTS events_execution_time ON events(execution_id,occurred_at,sequence_no);
CREATE INDEX IF NOT EXISTS events_severity_time ON events(severity,occurred_at DESC);
CREATE INDEX IF NOT EXISTS events_trace ON events(trace_id,occurred_at DESC);
CREATE INDEX IF NOT EXISTS events_occurred ON events(occurred_at);
CREATE INDEX IF NOT EXISTS error_groups_project_last ON error_groups(project_id,last_seen_at DESC);
CREATE INDEX IF NOT EXISTS error_occurrences_group ON error_occurrences(error_group_id,event_occurred_at DESC);
CREATE INDEX IF NOT EXISTS attributions_code_version ON change_attributions(code_version_id);

CREATE TABLE IF NOT EXISTS change_packages (
  package_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  delivery_stream TEXT NOT NULL CHECK (length(delivery_stream)<=240),
  delivery_mode TEXT NOT NULL CHECK (delivery_mode IN ('local','push','pull-request')),
  commit_sha TEXT CHECK (commit_sha IS NULL OR length(commit_sha)<=64),
  pr_url TEXT,
  prompt_count INTEGER NOT NULL CHECK (prompt_count > 0),
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS prompt_change_assessments (
  assessment_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  prompt_id TEXT REFERENCES prompt_nodes(prompt_id) ON DELETE SET NULL,
  delivery_stream TEXT NOT NULL CHECK (length(delivery_stream)<=240),
  change_kind TEXT NOT NULL CHECK (change_kind IN ('non-code','docs','media','code')),
  declared_class TEXT NOT NULL CHECK (declared_class IN ('non-code','routine','moderate','substantial','high-risk')),
  effective_class TEXT NOT NULL CHECK (effective_class IN ('non-code','routine','moderate','substantial','high-risk')),
  classification_reason TEXT NOT NULL DEFAULT '' CHECK (length(classification_reason)<=500),
  estimated_lines INTEGER NOT NULL DEFAULT 0 CHECK (estimated_lines >= 0),
  estimated_files INTEGER NOT NULL DEFAULT 0 CHECK (estimated_files >= 0),
  actual_lines_added INTEGER NOT NULL DEFAULT 0 CHECK (actual_lines_added >= 0),
  actual_lines_deleted INTEGER NOT NULL DEFAULT 0 CHECK (actual_lines_deleted >= 0),
  actual_files INTEGER NOT NULL DEFAULT 0 CHECK (actual_files >= 0),
  risk_flags TEXT NOT NULL DEFAULT '[]' CHECK (json_type(risk_flags)='array'),
  source_urls TEXT NOT NULL DEFAULT '[]' CHECK (json_type(source_urls)='array'),
  input_tokens INTEGER CHECK (input_tokens IS NULL OR input_tokens >= 0),
  output_tokens INTEGER CHECK (output_tokens IS NULL OR output_tokens >= 0),
  cached_input_tokens INTEGER CHECK (cached_input_tokens IS NULL OR cached_input_tokens >= 0),
  reasoning_tokens INTEGER CHECK (reasoning_tokens IS NULL OR reasoning_tokens >= 0),
  status TEXT NOT NULL DEFAULT 'planned' CHECK (status IN ('planned','success','stopped','failed')),
  increments_counter INTEGER NOT NULL DEFAULT 0 CHECK (increments_counter IN (0,1)),
  package_id TEXT REFERENCES change_packages(package_id) ON DELETE SET NULL,
  created_at TEXT NOT NULL,
  completed_at TEXT,
  UNIQUE(project_id,prompt_id,delivery_stream)
);

CREATE INDEX IF NOT EXISTS prompt_change_assessments_counter
  ON prompt_change_assessments(project_id,delivery_stream,increments_counter,package_id);

CREATE TABLE IF NOT EXISTS benchmark_references (
  reference_id TEXT PRIMARY KEY,
  reference_key TEXT NOT NULL CHECK (length(reference_key)<=160),
  reference_type TEXT NOT NULL CHECK (reference_type IN ('normative_standard','recognized_benchmark','peer_baseline','internal_slo')),
  issuer TEXT NOT NULL CHECK (length(issuer)<=240),
  title TEXT NOT NULL CHECK (length(title)<=500),
  version TEXT NOT NULL CHECK (length(version)<=120),
  source_url TEXT NOT NULL,
  published_at TEXT,
  retrieved_at TEXT NOT NULL,
  content_sha256 TEXT NOT NULL,
  review_due_at TEXT,
  status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','superseded','withdrawn')),
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z'),
  UNIQUE(reference_key,version)
);

CREATE TABLE IF NOT EXISTS benchmark_programs (
  program_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  name TEXT NOT NULL CHECK (length(name)<=240),
  measured_subject TEXT NOT NULL DEFAULT 'project' CHECK (measured_subject IN ('project','workflow','service')),
  status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','paused','retired')),
  approval_prompt_id TEXT REFERENCES prompt_nodes(prompt_id) ON DELETE SET NULL,
  activated_at TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z'),
  UNIQUE(project_id,name)
);

CREATE TABLE IF NOT EXISTS benchmark_contract_versions (
  contract_version_id TEXT PRIMARY KEY,
  program_id TEXT NOT NULL REFERENCES benchmark_programs(program_id) ON DELETE CASCADE,
  version_no INTEGER NOT NULL CHECK (version_no > 0),
  source_path TEXT NOT NULL,
  snapshot_toml TEXT NOT NULL,
  snapshot_sha256 TEXT NOT NULL,
  approved_by TEXT NOT NULL DEFAULT 'user' CHECK (length(approved_by)<=120),
  approved_at TEXT NOT NULL,
  superseded_at TEXT,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z'),
  UNIQUE(program_id,version_no),
  UNIQUE(program_id,snapshot_sha256)
);

CREATE TABLE IF NOT EXISTS benchmark_metrics (
  metric_id TEXT PRIMARY KEY,
  contract_version_id TEXT NOT NULL REFERENCES benchmark_contract_versions(contract_version_id) ON DELETE CASCADE,
  reference_id TEXT REFERENCES benchmark_references(reference_id) ON DELETE RESTRICT,
  metric_key TEXT NOT NULL CHECK (length(metric_key)<=160),
  title TEXT NOT NULL CHECK (length(title)<=240),
  unit TEXT NOT NULL CHECK (length(unit)<=80),
  aggregation TEXT NOT NULL DEFAULT 'mean' CHECK (aggregation IN ('mean','median','sum','min','max','latest','proportion')),
  direction TEXT NOT NULL CHECK (direction IN ('higher','lower','exact')),
  evaluation_method TEXT NOT NULL CHECK (evaluation_method IN ('conformance','exact_binomial','one_sample_t','external')),
  hypothesis_mode TEXT CHECK (hypothesis_mode IN ('non_inferiority','superiority','equivalence')),
  null_hypothesis TEXT NOT NULL DEFAULT '',
  alternative_hypothesis TEXT NOT NULL DEFAULT '',
  target_value REAL,
  margin REAL NOT NULL DEFAULT 0 CHECK (margin >= 0),
  lower_bound REAL,
  upper_bound REAL,
  alpha REAL NOT NULL DEFAULT 0.05 CHECK (alpha > 0 AND alpha < 1),
  confidence_level REAL NOT NULL DEFAULT 0.95 CHECK (confidence_level > 0 AND confidence_level < 1),
  minimum_samples INTEGER NOT NULL DEFAULT 1 CHECK (minimum_samples > 0),
  window_days INTEGER CHECK (window_days IS NULL OR window_days > 0),
  required INTEGER NOT NULL DEFAULT 1 CHECK (required IN (0,1)),
  core_extractor TEXT CHECK (core_extractor IS NULL OR length(core_extractor)<=64),
  evidence_requirements TEXT NOT NULL DEFAULT '[]' CHECK (json_type(evidence_requirements)='array'),
  comparability_rules TEXT NOT NULL DEFAULT '{}' CHECK (json_type(comparability_rules)='object'),
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z'),
  UNIQUE(contract_version_id,metric_key),
  CHECK (evaluation_method='conformance' OR hypothesis_mode IS NOT NULL),
  CHECK (evaluation_method='external' OR target_value IS NOT NULL OR (lower_bound IS NOT NULL AND upper_bound IS NOT NULL))
);

CREATE TABLE IF NOT EXISTS benchmark_observations (
  observation_id TEXT PRIMARY KEY,
  program_id TEXT NOT NULL REFERENCES benchmark_programs(program_id) ON DELETE CASCADE,
  metric_key TEXT NOT NULL CHECK (length(metric_key)<=160),
  execution_id TEXT REFERENCES executions(execution_id) ON DELETE SET NULL,
  prompt_id TEXT REFERENCES prompt_nodes(prompt_id) ON DELETE SET NULL,
  value REAL NOT NULL,
  observed_at TEXT NOT NULL,
  provenance_kind TEXT NOT NULL CHECK (provenance_kind IN ('hook','import','manual','external_suite')),
  provenance_locator TEXT NOT NULL DEFAULT '',
  evidence_sha256 TEXT,
  environment_fingerprint TEXT,
  comparable INTEGER NOT NULL DEFAULT 1 CHECK (comparable IN (0,1)),
  validation_status TEXT NOT NULL DEFAULT 'accepted' CHECK (validation_status IN ('accepted','rejected','pending')),
  attributes TEXT NOT NULL DEFAULT '{}' CHECK (json_type(attributes)='object'),
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z')
);

CREATE INDEX IF NOT EXISTS benchmark_observations_lookup
  ON benchmark_observations(program_id,metric_key,observed_at);

CREATE TABLE IF NOT EXISTS benchmark_runs (
  run_id TEXT PRIMARY KEY,
  program_id TEXT NOT NULL REFERENCES benchmark_programs(program_id) ON DELETE CASCADE,
  contract_version_id TEXT NOT NULL REFERENCES benchmark_contract_versions(contract_version_id) ON DELETE RESTRICT,
  window_start TEXT,
  window_end TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('completed','partial','failed')),
  scipy_version TEXT CHECK (scipy_version IS NULL OR length(scipy_version)<=80),
  environment_fingerprint TEXT,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS benchmark_results (
  result_id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL REFERENCES benchmark_runs(run_id) ON DELETE CASCADE,
  metric_id TEXT NOT NULL REFERENCES benchmark_metrics(metric_id) ON DELETE RESTRICT,
  sample_count INTEGER NOT NULL DEFAULT 0 CHECK (sample_count >= 0),
  observed_value REAL,
  statistic REAL,
  p_value REAL CHECK (p_value IS NULL OR (p_value >= 0 AND p_value <= 1)),
  confidence_low REAL,
  confidence_high REAL,
  verdict TEXT NOT NULL CHECK (verdict IN ('meets','exceeds','below','inconclusive','insufficient_evidence','not_comparable','not_applicable','error')),
  readiness_gaps TEXT NOT NULL DEFAULT '[]' CHECK (json_type(readiness_gaps)='array'),
  details TEXT NOT NULL DEFAULT '{}' CHECK (json_type(details)='object'),
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z'),
  UNIQUE(run_id,metric_id)
);

CREATE TABLE IF NOT EXISTS herdr_runtime_instances (
  runtime_id TEXT PRIMARY KEY,
  machine_label TEXT NOT NULL DEFAULT 'local',
  session_name TEXT NOT NULL DEFAULT 'default',
  version TEXT NOT NULL,
  api_schema_hash TEXT NOT NULL,
  endpoint_generation INTEGER,
  status TEXT NOT NULL DEFAULT 'healthy',
  last_seen_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z'),
  attributes TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(attributes)),
  UNIQUE(machine_label, session_name)
);

CREATE TABLE IF NOT EXISTS herdr_pane_bindings (
  binding_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  native_session TEXT NOT NULL CHECK (length(native_session)<=240),
  profile_name TEXT NOT NULL CHECK (profile_name IN ('personal','cy','feu')),
  machine_label TEXT NOT NULL DEFAULT 'local',
  herdr_session TEXT NOT NULL DEFAULT 'default',
  workspace_id TEXT,
  tab_id TEXT,
  pane_id TEXT,
  runtime_scope TEXT NOT NULL CHECK (runtime_scope IN ('herdr','outside_herdr')),
  cwd TEXT NOT NULL DEFAULT '',
  active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0,1)),
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z'),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z'),
  UNIQUE(native_session, profile_name)
);

CREATE INDEX IF NOT EXISTS herdr_pane_bindings_project_active
  ON herdr_pane_bindings(project_id, active, updated_at DESC);

CREATE TABLE IF NOT EXISTS agent_state_events (
  state_event_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  native_session TEXT NOT NULL,
  native_turn TEXT,
  profile_name TEXT NOT NULL,
  previous_state TEXT,
  canonical_state TEXT NOT NULL CHECK (canonical_state IN ('idle','working','needs_input','ready','blocked')),
  projected_state TEXT NOT NULL CHECK (projected_state IN ('idle','working','blocked','done')),
  reason TEXT NOT NULL,
  authority TEXT NOT NULL,
  signal_source TEXT NOT NULL,
  sequence_no INTEGER NOT NULL DEFAULT 0,
  conflict INTEGER NOT NULL DEFAULT 0 CHECK (conflict IN (0,1)),
  attributes TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(attributes)),
  occurred_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z')
);

CREATE INDEX IF NOT EXISTS agent_state_events_priority
  ON agent_state_events(project_id, canonical_state, occurred_at DESC);

CREATE TABLE IF NOT EXISTS herdr_runtime_events (
  herdr_event_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  native_session TEXT,
  pane_id TEXT,
  event_name TEXT NOT NULL CHECK (length(event_name)<=160),
  sequence_no INTEGER NOT NULL DEFAULT 0,
  payload TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(payload)),
  occurred_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z')
);

CREATE INDEX IF NOT EXISTS herdr_runtime_events_project_time
  ON herdr_runtime_events(project_id, occurred_at DESC);

CREATE TABLE IF NOT EXISTS turn_usage (
  usage_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  native_session TEXT NOT NULL CHECK (length(native_session)<=240),
  native_turn TEXT,
  prompt_id TEXT REFERENCES prompt_nodes(prompt_id) ON DELETE SET NULL,
  input_tokens INTEGER CHECK (input_tokens IS NULL OR input_tokens >= 0),
  output_tokens INTEGER CHECK (output_tokens IS NULL OR output_tokens >= 0),
  cached_input_tokens INTEGER CHECK (cached_input_tokens IS NULL OR cached_input_tokens >= 0),
  reasoning_tokens INTEGER CHECK (reasoning_tokens IS NULL OR reasoning_tokens >= 0),
  availability TEXT NOT NULL CHECK (availability IN ('exact','unavailable')),
  source TEXT NOT NULL CHECK (length(source)<=120),
  source_event_id TEXT,
  recorded_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z'),
  UNIQUE(project_id, native_session, native_turn, source_event_id)
);

CREATE INDEX IF NOT EXISTS turn_usage_project_time ON turn_usage(project_id,recorded_at DESC);
CREATE INDEX IF NOT EXISTS turn_usage_prompt ON turn_usage(prompt_id) WHERE prompt_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS project_handoffs (
  handoff_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  version_no INTEGER NOT NULL,
  base_version INTEGER,
  handoff_kind TEXT NOT NULL CHECK (handoff_kind IN ('progress','unfinished','observation','decision','architecture','constraint')),
  profile_name TEXT NOT NULL,
  native_session TEXT,
  content TEXT NOT NULL CHECK (length(content) <= 12000),
  is_active INTEGER NOT NULL DEFAULT 1 CHECK (is_active IN (0,1)),
  requires_promotion INTEGER NOT NULL DEFAULT 0 CHECK (requires_promotion IN (0,1)),
  promoted_at TEXT,
  conflict INTEGER NOT NULL DEFAULT 0 CHECK (conflict IN (0,1)),
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z'),
  UNIQUE(project_id, version_no)
);

CREATE INDEX IF NOT EXISTS project_handoffs_bootstrap
  ON project_handoffs(project_id, is_active, conflict, version_no DESC);

CREATE TABLE IF NOT EXISTS pane_evidence_snapshots (
  snapshot_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  native_session TEXT NOT NULL,
  pane_id TEXT,
  trigger_code TEXT NOT NULL,
  content_sha256 TEXT NOT NULL,
  line_count INTEGER NOT NULL CHECK (line_count BETWEEN 0 AND 80),
  byte_count INTEGER NOT NULL CHECK (byte_count BETWEEN 0 AND 32768),
  redacted_content TEXT NOT NULL,
  explain TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(explain)),
  captured_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f','now') || '000Z'),
  UNIQUE(project_id, native_session, content_sha256)
);

CREATE TABLE IF NOT EXISTS prompt_native_turns (
  project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  native_session TEXT NOT NULL,
  native_turn TEXT NOT NULL,
  prompt_id TEXT NOT NULL REFERENCES prompt_nodes(prompt_id) ON DELETE CASCADE,
  PRIMARY KEY (project_id,native_session,native_turn)
);

CREATE TABLE IF NOT EXISTS tool_uses (
  tool_record_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  prompt_id TEXT REFERENCES prompt_nodes(prompt_id) ON DELETE SET NULL,
  execution_id TEXT REFERENCES executions(execution_id) ON DELETE SET NULL,
  native_session TEXT NOT NULL,
  native_turn TEXT,
  tool_use_id TEXT,
  tool_name TEXT NOT NULL CHECK (length(tool_name)<=160),
  origin TEXT NOT NULL CHECK (origin IN ('built-in','mcp','harness','unknown')),
  parent_tool_use_id TEXT,
  started_at TEXT,
  completed_at TEXT NOT NULL,
  duration_ms INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
  failed INTEGER CHECK (failed IS NULL OR failed IN (0,1)),
  cache_hit INTEGER CHECK (cache_hit IS NULL OR cache_hit IN (0,1)),
  cache_status TEXT NOT NULL DEFAULT 'unknown' CHECK (cache_status IN ('hit','miss','unknown')),
  UNIQUE (project_id,native_session,tool_use_id)
);

CREATE INDEX IF NOT EXISTS tool_uses_project_time ON tool_uses(project_id,completed_at DESC);
CREATE INDEX IF NOT EXISTS tool_uses_prompt ON tool_uses(prompt_id) WHERE prompt_id IS NOT NULL;

CREATE VIEW IF NOT EXISTS turn_activity AS
SELECT p.project_id,
       pn.prompt_id,
       pn.session_id,
       pn.ordinal,
       pn.raw_prompt,
       pn.raw_result,
       tu.native_turn,
       tu.input_tokens,
       tu.output_tokens,
       tu.cached_input_tokens,
       tu.reasoning_tokens,
       tu.availability AS usage_availability,
       tu.source AS usage_source,
       pn.created_at,
       pn.completed_at
FROM prompt_nodes pn
JOIN sessions s ON s.session_id = pn.session_id
JOIN projects p ON p.project_id = s.project_id
LEFT JOIN turn_usage tu ON tu.prompt_id = pn.prompt_id;

INSERT OR IGNORE INTO schema_migrations(version) VALUES (1),(2),(3),(4),(5),(6),(7);
