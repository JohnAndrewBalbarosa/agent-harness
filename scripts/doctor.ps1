[CmdletBinding()]
param(
  [string]$Destination = (Join-Path ([Environment]::GetFolderPath('UserProfile')) '.agent-harness'),
  [switch]$CheckCompile
)
# Read-only health report for an installed agent-harness. Prints one JSON object; exit 1 if a required check fails.
$ErrorActionPreference = 'Stop'
$Destination = [IO.Path]::GetFullPath($Destination)
$checks = [ordered]@{}

function Test-Url([string]$Url) {
  try { return (Invoke-WebRequest -Uri $Url -UseBasicParsing -TimeoutSec 2).StatusCode -eq 200 } catch { return $false }
}

$state = Join-Path $Destination 'var\install-state.json'
$checks.installed = Test-Path -LiteralPath $state
$python = if ($checks.installed) { (Get-Content -LiteralPath $state -Raw | ConvertFrom-Json).python } else { 'python' }
$shim = Join-Path $Destination 'core\router\harness-hook.cmd'
$checks.router_shim = Test-Path -LiteralPath $shim
if ($checks.router_shim) {
  $out = '{"hook_event_name":"Stop","session_id":"doctor"}' | & cmd /c "`"$shim`" codex"
  $checks.router_answers = ($out -join '').Contains('"continue": true')
}
$envFile = Join-Path $Destination '.env'
$checks.env_file = Test-Path -LiteralPath $envFile
$port = 4320; $hub = 'http://127.0.0.1:4319'
if ($checks.env_file) {
  foreach ($line in Get-Content -LiteralPath $envFile) {
    if ($line -match '^\s*OTLP_PORT\s*=\s*(\d+)') { $port = [int]$Matches[1] }
    if ($line -match '^\s*OBS_ENDPOINT\s*=\s*(\S+)') { $hub = $Matches[1].Trim('"') }
  }
}
$checks.usage_receiver = Test-Url "http://127.0.0.1:$port/health"
$checks.observability_hub = Test-Url "$hub/health"
$checks.policy = Test-Path -LiteralPath (Join-Path $Destination 'policy\AGENTS.md')
if ($CheckCompile -and $checks.env_file) {
  $env:PYTHONPATH = $Destination
  Push-Location $Destination
  try { $summary = & $python -m compile.run --env-file $envFile --dry-run | Select-Object -Last 1; $checks.compile_pending = "$summary" } finally { Pop-Location }
}
$required = @('installed', 'router_shim', 'router_answers', 'env_file', 'policy')
$failed = @($required | Where-Object { -not $checks[$_] })
$checks.status = if ($failed) { 'unhealthy' } else { 'healthy' }
$checks.failed = $failed
$checks | ConvertTo-Json -Compress
if ($failed) { exit 1 }
