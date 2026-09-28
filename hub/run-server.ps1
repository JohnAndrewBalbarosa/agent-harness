$ErrorActionPreference = 'Stop'
$toolRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$logDir = Join-Path $toolRoot 'var\logs'
New-Item -ItemType Directory -Path $logDir -Force | Out-Null
$startedAt = [DateTime]::UtcNow
$runStamp = $startedAt.ToString('yyyyMMddTHHmmssfffZ')
$stdout = Join-Path $logDir "server.$runStamp.stdout.log"
$stderr = Join-Path $logDir "server.$runStamp.stderr.log"
$lifecycle = Join-Path $logDir 'server.lifecycle.jsonl'
if (-not $env:OBS_DATABASE_URL) {
  $containerEnvironment = docker inspect --format '{{json .Config.Env}}' codex-observability-postgres 2>$null | ConvertFrom-Json
  if ($LASTEXITCODE -ne 0 -or -not $containerEnvironment) { throw 'Unable to read the local observability PostgreSQL configuration.' }
  $databaseValues = @{}
  foreach ($entry in $containerEnvironment) {
    $parts = $entry -split '=', 2
    if ($parts.Count -eq 2) { $databaseValues[$parts[0]] = $parts[1] }
  }
  foreach ($required in @('POSTGRES_USER', 'POSTGRES_PASSWORD', 'POSTGRES_DB')) {
    if (-not $databaseValues[$required]) { throw "Missing $required in the observability PostgreSQL container." }
  }
  $databaseUser = [Uri]::EscapeDataString($databaseValues.POSTGRES_USER)
  $databasePassword = [Uri]::EscapeDataString($databaseValues.POSTGRES_PASSWORD)
  $databaseName = [Uri]::EscapeDataString($databaseValues.POSTGRES_DB)
  $env:OBS_DATABASE_URL = "postgresql://${databaseUser}:${databasePassword}@127.0.0.1:55432/${databaseName}"
}
$server = Start-Process -FilePath 'node.exe' -ArgumentList @(
  (Join-Path $toolRoot 'src\server.mjs')
) -WorkingDirectory $toolRoot -RedirectStandardOutput $stdout -RedirectStandardError $stderr -WindowStyle Hidden -PassThru
$server.WaitForExit()
$endedAt = [DateTime]::UtcNow
$record = [ordered]@{
  schema_version = 1
  timestamp = $endedAt.ToString('o')
  severity = if ($server.ExitCode -eq 0) { 'info' } else { 'error' }
  event = 'observability.server.exited'
  component = 'observability-hub'
  operation = 'serve'
  outcome = if ($server.ExitCode -eq 0) { 'stopped' } else { 'failed' }
  details = [ordered]@{
    pid = $server.Id
    exit_code = $server.ExitCode
    duration_ms = [math]::Round(($endedAt - $startedAt).TotalMilliseconds)
  }
}
Add-Content -LiteralPath $lifecycle -Value ($record | ConvertTo-Json -Compress -Depth 5)
exit $server.ExitCode
