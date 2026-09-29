#!/usr/bin/env bash
# Read-only export of the observability Postgres into JSON lines for `obs-hub import-postgres`.
#
# Usage: scripts/obs-export-postgres.sh <dump_dir> [container] [database] [user]
#   dump_dir   output folder (created); receives <table>.jsonl + manifest.json
#   container  default codex-observability-postgres
#   database   default observability
#   user       default observability
#
# Mental model: one psql session opens a REPEATABLE READ READ ONLY transaction, writes the
# manifest (row counts + column types) first so it fixes the snapshot, then COPYs every table
# from that same snapshot into files inside the container; `docker cp` brings them out and the
# container scratch folder is removed. Postgres itself is never written.
#
# COPY uses CSV format with control-character quote/delimiter so the JSON text is emitted
# verbatim (text format would double every backslash). row_to_json escapes all control
# characters, so \x01/\x02 never occur inside a line.
set -euo pipefail

DUMP_DIR="${1:?usage: obs-export-postgres.sh <dump_dir> [container] [database] [user]}"
CONTAINER="${2:-codex-observability-postgres}"
DATABASE="${3:-observability}"
DB_USER="${4:-observability}"

TABLES=(
  projects project_aliases project_context_versions agent_instances sessions prompt_nodes
  code_versions executions change_attributions components event_definitions events
  error_groups error_occurrences event_payloads daily_rollups change_packages
  prompt_change_assessments benchmark_references benchmark_programs
  benchmark_contract_versions benchmark_metrics benchmark_observations benchmark_runs
  benchmark_results herdr_runtime_instances herdr_pane_bindings agent_state_events
  herdr_runtime_events turn_usage project_handoffs pane_evidence_snapshots
  prompt_native_turns tool_uses
)

REMOTE_DIR="/tmp/obs-export-$(date -u +%Y%m%dT%H%M%SZ)-$$"

manifest_table_entry() {
  local table="$1"
  printf "json_build_object('name','%s','row_count',(SELECT count(*) FROM public.%s),'columns',(SELECT json_agg(json_build_object('name',column_name,'type',udt_name) ORDER BY ordinal_position) FROM information_schema.columns WHERE table_schema='public' AND table_name='%s'))" \
    "$table" "$table" "$table"
}

build_export_sql() {
  local entries=() table
  for table in "${TABLES[@]}"; do
    entries+=("$(manifest_table_entry "$table")")
  done
  local joined
  joined="$(IFS=,; echo "${entries[*]}")"

  echo '\set ON_ERROR_STOP on'
  echo "SET client_encoding = 'UTF8';"
  echo "SET TimeZone = 'UTC';"
  echo 'BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;'
  echo "\\o ${REMOTE_DIR}/manifest.json"
  echo "SELECT json_build_object('format','obs-postgres-export/v1','snapshot','repeatable_read','exported_at',to_char(clock_timestamp() AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'),'database',current_database(),'server_version',current_setting('server_version'),'tables',json_build_array(${joined}))::text;"
  for table in "${TABLES[@]}"; do
    echo "\\o ${REMOTE_DIR}/${table}.jsonl"
    echo "COPY (SELECT row_to_json(t) FROM public.${table} t) TO STDOUT WITH (FORMAT csv, QUOTE E'\\x01', DELIMITER E'\\x02');"
  done
  echo '\o'
  echo 'COMMIT;'
}

cleanup_remote_dir() {
  docker exec "$CONTAINER" rm -rf "$REMOTE_DIR" >/dev/null 2>&1 || true
}

expected_row_count() {
  local table="$1"
  # Manifest is a single line; pull `"name" : "<table>", "row_count" : N` without jq.
  grep -o "\"name\" *: *\"${table}\", *\"row_count\" *: *[0-9]*" "$DUMP_DIR/manifest.json" | grep -o '[0-9]*$' || echo '?'
}

mkdir -p "$DUMP_DIR"
trap cleanup_remote_dir EXIT
docker exec "$CONTAINER" mkdir -p "$REMOTE_DIR"
build_export_sql | docker exec -i -e PGCLIENTENCODING=UTF8 "$CONTAINER" \
  psql -X -q -A -t -v ON_ERROR_STOP=1 -U "$DB_USER" -d "$DATABASE" -f - >/dev/null
docker cp "$CONTAINER:$REMOTE_DIR/." "$DUMP_DIR" >/dev/null

for table in "${TABLES[@]}"; do
  lines=$(wc -l < "$DUMP_DIR/${table}.jsonl" | tr -d ' ')
  echo "${table} exported=${lines} manifest=$(expected_row_count "$table")"
done
