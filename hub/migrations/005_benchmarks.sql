CREATE TABLE IF NOT EXISTS benchmark_definitions (
  benchmark_id varchar(160) PRIMARY KEY,
  suite varchar(120) NOT NULL,
  definition jsonb NOT NULL,
  source_url varchar(1200),
  effective_at timestamptz NOT NULL DEFAULT now(),
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS benchmark_observations (
  observation_id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(project_id),
  execution_id uuid REFERENCES executions(execution_id),
  benchmark_id varchar(160) NOT NULL,
  profile varchar(40) NOT NULL,
  value double precision NOT NULL,
  accepted boolean NOT NULL DEFAULT true,
  observed_at timestamptz NOT NULL
);
CREATE INDEX IF NOT EXISTS benchmark_observations_window ON benchmark_observations(project_id,profile,benchmark_id,observed_at DESC);

CREATE TABLE IF NOT EXISTS benchmark_decisions (
  decision_id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(project_id),
  benchmark_id varchar(160) NOT NULL,
  profile varchar(40) NOT NULL,
  hypothesis_mode varchar(32) NOT NULL CHECK (hypothesis_mode IN ('non-inferiority','superiority')),
  sample_count integer NOT NULL CHECK (sample_count >= 0),
  decision varchar(32) NOT NULL CHECK (decision IN ('pass','inconclusive','regression','insufficient_data')),
  gate_status varchar(24) NOT NULL CHECK (gate_status IN ('open','blocked')),
  result jsonb NOT NULL,
  decided_at timestamptz NOT NULL
);
CREATE INDEX IF NOT EXISTS benchmark_decisions_latest ON benchmark_decisions(project_id,profile,benchmark_id,decided_at DESC);

