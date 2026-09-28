CREATE TABLE IF NOT EXISTS prompt_native_turns (
  project_id uuid NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  native_session varchar(240) NOT NULL,
  native_turn varchar(240) NOT NULL,
  prompt_id uuid NOT NULL REFERENCES prompt_nodes(prompt_id) ON DELETE CASCADE,
  PRIMARY KEY (project_id,native_session,native_turn)
);

CREATE TABLE IF NOT EXISTS turn_usage (
  usage_id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  native_session varchar(240) NOT NULL,
  native_turn varchar(240),
  prompt_id uuid REFERENCES prompt_nodes(prompt_id) ON DELETE SET NULL,
  input_tokens bigint CHECK (input_tokens >= 0),
  output_tokens bigint CHECK (output_tokens >= 0),
  cached_input_tokens bigint CHECK (cached_input_tokens >= 0),
  reasoning_tokens bigint CHECK (reasoning_tokens >= 0),
  availability varchar(32) NOT NULL CHECK (availability IN ('exact','unavailable')),
  source varchar(120) NOT NULL,
  source_event_id varchar(240),
  recorded_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE(project_id,native_session,native_turn,source_event_id)
);
CREATE INDEX IF NOT EXISTS turn_usage_project_time ON turn_usage(project_id,recorded_at DESC);
CREATE INDEX IF NOT EXISTS turn_usage_prompt ON turn_usage(prompt_id) WHERE prompt_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS tool_uses (
  tool_record_id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  prompt_id uuid REFERENCES prompt_nodes(prompt_id) ON DELETE SET NULL,
  execution_id uuid REFERENCES executions(execution_id) ON DELETE SET NULL,
  native_session varchar(240) NOT NULL,
  native_turn varchar(240),
  tool_use_id varchar(240),
  tool_name varchar(160) NOT NULL,
  origin varchar(24) NOT NULL CHECK (origin IN ('built-in','mcp','harness','unknown')),
  parent_tool_use_id varchar(240),
  started_at timestamptz,
  completed_at timestamptz NOT NULL,
  duration_ms bigint CHECK (duration_ms >= 0),
  failed boolean,
  cache_hit boolean,
  cache_status varchar(16) NOT NULL DEFAULT 'unknown' CHECK (cache_status IN ('hit','miss','unknown')),
  UNIQUE (project_id,native_session,tool_use_id)
);
CREATE INDEX IF NOT EXISTS tool_uses_project_time ON tool_uses(project_id,completed_at DESC);
CREATE INDEX IF NOT EXISTS tool_uses_prompt ON tool_uses(prompt_id) WHERE prompt_id IS NOT NULL;
