$ErrorActionPreference = 'Stop'
$toolRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$logDir = Join-Path $toolRoot 'var\logs'
New-Item -ItemType Directory -Path $logDir -Force | Out-Null
$startupLog = Join-Path $logDir 'startup.lifecycle.jsonl'
$correlationId = [guid]::NewGuid().ToString()
function Write-StartupLog($severity, $operation, $outcome, $details = @{}) {
  [ordered]@{ timestamp=[DateTime]::UtcNow.ToString('o'); severity=$severity; component='observability-hub'; operation=$operation; outcome=$outcome; correlationId=$correlationId; details=$details } |
    ConvertTo-Json -Compress -Depth 4 | Add-Content -LiteralPath $startupLog -Encoding utf8
}
trap { $message=$_.Exception.Message; Write-StartupLog 'error' 'hub.start' 'failed' @{ error=$message }; [Console]::Error.WriteLine($message); exit 1 }
Write-StartupLog 'info' 'hub.start' 'started'
try {
  $existingHealth = Invoke-WebRequest -Uri 'http://127.0.0.1:4319/health' -UseBasicParsing -TimeoutSec 2
  if ($existingHealth.StatusCode -eq 200) {
    Write-StartupLog 'info' 'hub.health' 'already_running'
    Write-Output "Observability hub is already running. Log=$startupLog"
    exit 0
  }
} catch {}
if (-not ($dockerCommand = Get-Command docker -ErrorAction SilentlyContinue)) { throw 'Docker CLI was not found on PATH.' }
function Test-DockerEngine {
  $startInfo = [Diagnostics.ProcessStartInfo]::new($dockerCommand.Source, 'info --format "{{.ServerVersion}}"')
  $startInfo.UseShellExecute = $false
  $startInfo.RedirectStandardOutput = $true
  $startInfo.RedirectStandardError = $true
  $startInfo.CreateNoWindow = $true
  $probe = [Diagnostics.Process]::new()
  $probe.StartInfo = $startInfo
  if (-not $probe.Start()) { return $false }
  if (-not $probe.WaitForExit(5000)) { $probe.Kill(); $probe.WaitForExit(); return $false }
  return $probe.ExitCode -eq 0
}
if (-not (Test-DockerEngine)) {
  Write-StartupLog 'warn' 'docker.engine.health' 'unavailable'
  $previousPreference = $ErrorActionPreference
  $ErrorActionPreference = 'Continue'
  try { & docker desktop start --detach --timeout 120 *> $null; $desktopExitCode = $LASTEXITCODE }
  finally { $ErrorActionPreference = $previousPreference }
  if ($desktopExitCode -ne 0) { throw "Docker Desktop start failed with exit code $desktopExitCode." }
  Write-StartupLog 'info' 'docker.desktop.start' 'requested'
  $deadline = [DateTime]::UtcNow.AddSeconds(120)
  while (-not (Test-DockerEngine) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Seconds 2 }
  if (-not (Test-DockerEngine)) { throw 'Docker engine did not become ready within 120 seconds.' }
  Write-StartupLog 'info' 'docker.engine.health' 'ready'
} else { Write-StartupLog 'info' 'docker.engine.health' 'ready' }
Write-StartupLog 'info' 'docker.compose.up' 'started'
docker compose -f (Join-Path $toolRoot 'docker-compose.yml') up -d
if ($LASTEXITCODE -ne 0) { throw "Docker Compose failed with exit code $LASTEXITCODE." }
Write-StartupLog 'info' 'docker.compose.up' 'succeeded'
Start-Sleep -Seconds 3
$databaseHealth = docker inspect --format '{{.State.Health.Status}}' codex-observability-postgres 2>$null
if ($databaseHealth -ne 'healthy') {
  throw "PostgreSQL failed the single bounded readiness check: $databaseHealth"
}
Write-StartupLog 'info' 'postgres.health' 'healthy'
$process = Start-Process -FilePath 'powershell.exe' -ArgumentList @(
  '-NoProfile',
  '-ExecutionPolicy', 'Bypass',
  '-File', (Join-Path $toolRoot 'run-server.ps1')
) -WorkingDirectory $toolRoot -WindowStyle Hidden -PassThru
Write-StartupLog 'info' 'hub.server.start' 'launched' @{ processId=$process.Id }
Write-Output "Observability hub started. PID=$($process.Id) Log=$startupLog"
