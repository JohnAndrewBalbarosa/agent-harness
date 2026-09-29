[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)][string]$DumpDir,
  [string]$Container = 'codex-observability-postgres',
  [string]$Database = 'observability',
  [string]$User = 'observability'
)
# Read-only export of the observability Postgres into JSON lines for `obs-hub import-postgres`.
# Output: <DumpDir>\<table>.jsonl (UTF-8, one row_to_json object per line) + <DumpDir>\manifest.json.
#
# Mental model: one psql session opens a REPEATABLE READ READ ONLY transaction, writes the
# manifest (row counts + column types) first so it fixes the snapshot, then COPYs every table
# from that same snapshot into files inside the container; `docker cp` brings them out and the
# container scratch folder is removed. Files never pass through a PowerShell pipeline, so their
# bytes are not re-encoded. Postgres itself is never written.
#
# COPY uses CSV format with control-character quote/delimiter so the JSON text is emitted
# verbatim (text format would double every backslash). row_to_json escapes all control
# characters, so \x01/\x02 never occur inside a line.
$ErrorActionPreference = 'Stop'

$Tables = @(
  'projects', 'project_aliases', 'project_context_versions', 'agent_instances', 'sessions', 'prompt_nodes',
  'code_versions', 'executions', 'change_attributions', 'components', 'event_definitions', 'events',
  'error_groups', 'error_occurrences', 'event_payloads', 'daily_rollups', 'change_packages',
  'prompt_change_assessments', 'benchmark_references', 'benchmark_programs',
  'benchmark_contract_versions', 'benchmark_metrics', 'benchmark_observations', 'benchmark_runs',
  'benchmark_results', 'herdr_runtime_instances', 'herdr_pane_bindings', 'agent_state_events',
  'herdr_runtime_events', 'turn_usage', 'project_handoffs', 'pane_evidence_snapshots',
  'prompt_native_turns', 'tool_uses'
)

$RemoteDir = "/tmp/obs-export-$([DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ'))-$PID"

function Get-ManifestTableEntry([string]$Table) {
  "json_build_object('name','$Table','row_count',(SELECT count(*) FROM public.$Table),'columns',(SELECT json_agg(json_build_object('name',column_name,'type',udt_name) ORDER BY ordinal_position) FROM information_schema.columns WHERE table_schema='public' AND table_name='$Table'))"
}

function Get-ExportSql {
  $entries = ($Tables | ForEach-Object { Get-ManifestTableEntry $_ }) -join ','
  $lines = [System.Collections.Generic.List[string]]::new()
  $lines.Add('\set ON_ERROR_STOP on')
  $lines.Add("SET client_encoding = 'UTF8';")
  $lines.Add("SET TimeZone = 'UTC';")
  $lines.Add('BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;')
  $lines.Add("\o $RemoteDir/manifest.json")
  $lines.Add("SELECT json_build_object('format','obs-postgres-export/v1','snapshot','repeatable_read','exported_at',to_char(clock_timestamp() AT TIME ZONE 'UTC','YYYY-MM-DD`"T`"HH24:MI:SS.MS`"Z`"'),'database',current_database(),'server_version',current_setting('server_version'),'tables',json_build_array($entries))::text;")
  foreach ($table in $Tables) {
    $lines.Add("\o $RemoteDir/$table.jsonl")
    $lines.Add("COPY (SELECT row_to_json(t) FROM public.$table t) TO STDOUT WITH (FORMAT csv, QUOTE E'\x01', DELIMITER E'\x02');")
  }
  $lines.Add('\o')
  $lines.Add('COMMIT;')
  ($lines -join "`n") + "`n"
}

function Invoke-Docker([string[]]$Arguments) {
  & docker @Arguments | Out-Null
  if ($LASTEXITCODE -ne 0) { throw "docker $($Arguments[0]) failed with exit code $LASTEXITCODE" }
}

function Get-JsonlLineCount([string]$Path) {
  $count = 0
  foreach ($null in [System.IO.File]::ReadLines($Path)) { $count++ }
  $count
}

$DumpDir = [IO.Path]::GetFullPath($DumpDir)
New-Item -ItemType Directory -Force -Path $DumpDir | Out-Null

try {
  Invoke-Docker @('exec', $Container, 'mkdir', '-p', $RemoteDir)

  $previousEncoding = $OutputEncoding
  $OutputEncoding = [System.Text.UTF8Encoding]::new($false)
  try {
    Get-ExportSql | & docker exec -i -e PGCLIENTENCODING=UTF8 $Container psql -X -q -A -t -v ON_ERROR_STOP=1 -U $User -d $Database -f - | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "psql export failed with exit code $LASTEXITCODE" }
  } finally {
    $OutputEncoding = $previousEncoding
  }

  Invoke-Docker @('cp', "${Container}:$RemoteDir/.", $DumpDir)
} finally {
  & docker exec $Container rm -rf $RemoteDir 2>$null | Out-Null
}

$manifest = Get-Content -LiteralPath (Join-Path $DumpDir 'manifest.json') -Raw -Encoding utf8 | ConvertFrom-Json
foreach ($entry in $manifest.tables) {
  $exported = Get-JsonlLineCount (Join-Path $DumpDir "$($entry.name).jsonl")
  Write-Output "$($entry.name) exported=$exported manifest=$($entry.row_count)"
}
