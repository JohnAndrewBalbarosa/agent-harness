CREATE TABLE IF NOT EXISTS prompt_change_assessments (
  assessment_id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  prompt_id uuid REFERENCES prompt_nodes(prompt_id) ON DELETE SET NULL,
  delivery_stream varchar(240) NOT NULL,
  change_kind varchar(24) NOT NULL CHECK (change_kind IN ('non-code','docs','media','code')),
  declared_class varchar(24) NOT NULL CHECK (declared_class IN ('non-code','routine','moderate','substantial','high-risk')),
  effective_class varchar(24) NOT NULL CHECK (effective_class IN ('non-code','routine','moderate','substantial','high-risk')),
  classification_reason varchar(500) NOT NULL DEFAULT '',
  estimated_lines integer NOT NULL DEFAULT 0 CHECK (estimated_lines >= 0),
  estimated_files integer NOT NULL DEFAULT 0 CHECK (estimated_files >= 0),
  actual_lines_added integer NOT NULL DEFAULT 0 CHECK (actual_lines_added >= 0),
  actual_lines_deleted integer NOT NULL DEFAULT 0 CHECK (actual_lines_deleted >= 0),
  actual_files integer NOT NULL DEFAULT 0 CHECK (actual_files >= 0),
  risk_flags jsonb NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(risk_flags)='array'),
  source_urls jsonb NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(source_urls)='array'),
  input_tokens bigint CHECK (input_tokens IS NULL OR input_tokens >= 0),
  output_tokens bigint CHECK (output_tokens IS NULL OR output_tokens >= 0),
  cached_input_tokens bigint CHECK (cached_input_tokens IS NULL OR cached_input_tokens >= 0),
  reasoning_tokens bigint CHECK (reasoning_tokens IS NULL OR reasoning_tokens >= 0),
  status varchar(24) NOT NULL DEFAULT 'planned' CHECK (status IN ('planned','success','stopped','failed')),
  increments_counter boolean NOT NULL DEFAULT false,
  package_id uuid,
  created_at timestamptz NOT NULL,
  completed_at timestamptz,
  UNIQUE(project_id,prompt_id,delivery_stream)
);

CREATE TABLE IF NOT EXISTS change_packages (
  package_id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  delivery_stream varchar(240) NOT NULL,
  delivery_mode varchar(24) NOT NULL CHECK (delivery_mode IN ('local','push','pull-request')),
  commit_sha varchar(64),
  pr_url text,
  prompt_count integer NOT NULL CHECK (prompt_count > 0),
  created_at timestamptz NOT NULL
);

ALTER TABLE prompt_change_assessments
  DROP CONSTRAINT IF EXISTS prompt_change_assessments_package_id_fkey;
ALTER TABLE prompt_change_assessments
  ADD CONSTRAINT prompt_change_assessments_package_id_fkey
  FOREIGN KEY(package_id) REFERENCES change_packages(package_id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS prompt_change_assessments_counter
  ON prompt_change_assessments(project_id,delivery_stream,increments_counter,package_id);

INSERT INTO schema_migrations(version) VALUES (4) ON CONFLICT DO NOTHING;
