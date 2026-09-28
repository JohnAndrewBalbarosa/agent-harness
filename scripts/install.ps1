[CmdletBinding()]
param(
  [string]$Destination = (Join-Path ([Environment]::GetFolderPath('UserProfile')) '.agent-harness'),
  [string]$EnvFile,
  [ValidateSet('Auto', 'ValidateOnly')][string]$Prerequisites = 'Auto',
  [switch]$WithHub,
  [switch]$CompileDryRun,
  [switch]$SkipCompile,
  [switch]$SkipServices,
  [switch]$SkipOrbVenv,
  [switch]$DryRun
)
# agent-harness installer. Code directories are mirrored on every run; owner-owned files are never overwritten:
# <Destination>\.env, <Destination>\policy\AGENTS.md, <Destination>\var\ and every per-tool var\ and .venv\
# (spool.sqlite3, hmac.key, the status orb venv). Runtime state in the repo checkout is never copied.

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$Destination = [IO.Path]::GetFullPath($Destination)
$installId = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
$codeDirs = @('core', 'adapters', 'compile', 'tools')

function Write-Event([string]$Code, [string]$Outcome, [hashtable]$Data = @{}) {
  $event = [ordered]@{ timestamp = [DateTime]::UtcNow.ToString('o'); component = 'installer'; operation = $Code; correlation_id = $installId; outcome = $Outcome; data = $Data }
  Write-Output ($event | ConvertTo-Json -Compress -Depth 6)
}

function Invoke-Step([string]$Code, [scriptblock]$Action) {
  if ($DryRun) { Write-Event $Code 'planned'; return }
  & $Action
  Write-Event $Code 'succeeded'
}

function Ensure-Command([string]$Command, [string]$PackageId) {
  if (Get-Command $Command -ErrorAction SilentlyContinue) { return }
  if ($Prerequisites -eq 'ValidateOnly' -or $DryRun) { Write-Event 'prerequisite.missing' 'warning' @{ command = $Command; package = $PackageId }; return }
  if (-not (Get-Command winget -ErrorAction SilentlyContinue)) { throw "Missing $Command and winget is unavailable; install $PackageId manually." }
  & winget install --id $PackageId --exact --accept-package-agreements --accept-source-agreements --silent
  if ($LASTEXITCODE -ne 0) { throw "winget failed for $PackageId with exit code $LASTEXITCODE" }
}

function Get-EnvFileValue([string]$Key) {
  foreach ($file in @((Join-Path $Destination '.env'), $EnvFile)) {
    if (-not $file -or -not (Test-Path -LiteralPath $file)) { continue }
    $line = Get-Content -LiteralPath $file | Where-Object { $_ -match "^\s*$Key\s*=" } | Select-Object -Last 1
    if ($line) { return [Environment]::ExpandEnvironmentVariables(($line -split '=', 2)[1].Trim().Trim('"', "'")) }
  }
  return $null
}

function Resolve-Python {
  $candidate = [Environment]::ExpandEnvironmentVariables('%APPDATA%\uv\python\cpython-3.14.7-windows-x86_64-none\python.exe')
  if ($env:HARNESS_PYTHON -and (Test-Path -LiteralPath $env:HARNESS_PYTHON)) { return $env:HARNESS_PYTHON }
  $configured = Get-EnvFileValue 'HARNESS_PYTHON'
  if ($configured -and (Test-Path -LiteralPath $configured)) { return $configured }
  if (Test-Path -LiteralPath $candidate) { return $candidate }
  $python = Get-Command python -ErrorAction SilentlyContinue
  if ($python) { return $python.Source }
  throw 'Python 3.11+ is required (set HARNESS_PYTHON or install uv Python).'
}

Write-Event 'install.start' 'started' @{ destination = $Destination; with_hub = [bool]$WithHub; dry_run = [bool]$DryRun }
foreach ($item in @(@('node', 'OpenJS.NodeJS.LTS'), @('uv', 'astral-sh.uv'))) { Ensure-Command $item[0] $item[1] }
if ($WithHub) { Ensure-Command 'docker' 'Docker.DockerDesktop' }
$python = Resolve-Python

Invoke-Step 'runtime.code' {
  New-Item -ItemType Directory -Path $Destination -Force | Out-Null
  foreach ($dir in $codeDirs) {
    # /MIR removes stale code; /XD keeps state dirs out of both the copy and the purge.
    & robocopy (Join-Path $repoRoot $dir) (Join-Path $Destination $dir) /MIR /XD var .venv .venv-* __pycache__ .git /XF *.pyc /NFL /NDL /NJH /NJS /NP | Out-Null
    if ($LASTEXITCODE -ge 8) { throw "robocopy failed for $dir with exit code $LASTEXITCODE" }
  }
  $global:LASTEXITCODE = 0
  Copy-Item -LiteralPath (Join-Path $repoRoot '.env.example') -Destination (Join-Path $Destination '.env.example') -Force
}

Invoke-Step 'runtime.env' {
  $envPath = Join-Path $Destination '.env'
  if (-not (Test-Path -LiteralPath $envPath)) {
    $source = if ($EnvFile) { $EnvFile } else { Join-Path $repoRoot '.env.example' }
    Copy-Item -LiteralPath $source -Destination $envPath
  }
}

Invoke-Step 'runtime.policy' {
  New-Item -ItemType Directory -Path (Join-Path $Destination 'policy') -Force | Out-Null
  # Harness-owned notes are refreshed every run; AGENTS.md is owner-owned and only created once.
  Copy-Item -LiteralPath (Join-Path $repoRoot 'policy\RTK.md') -Destination (Join-Path $Destination 'policy\RTK.md') -Force
  $policy = Join-Path $Destination 'policy\AGENTS.md'
  if (-not (Test-Path -LiteralPath $policy)) {
    New-Item -ItemType Directory -Path (Split-Path $policy -Parent) -Force | Out-Null
    $text = [IO.File]::ReadAllText((Join-Path $repoRoot 'policy\AGENTS.template.md')).Replace('{{HARNESS_HOME}}', $Destination)
    [IO.File]::WriteAllText($policy, $text, (New-Object Text.UTF8Encoding($false)))
  }
}

if (-not $SkipOrbVenv) {
  Invoke-Step 'orb.venv' {
    $orb = Join-Path $Destination 'tools\codex-status-orb'
    $venvPython = Join-Path $orb '.venv\Scripts\python.exe'
    if (-not (Test-Path -LiteralPath $venvPython)) {
      & $python -m venv (Join-Path $orb '.venv')
      if ($LASTEXITCODE -ne 0) { throw 'Status orb venv creation failed.' }
    }
    & $venvPython -m pip install --disable-pip-version-check --quiet --requirement (Join-Path $orb 'requirements.txt')
    if ($LASTEXITCODE -ne 0) { throw 'Status orb dependency installation failed.' }
  }
}

if ($WithHub) {
  Invoke-Step 'hub.install' {
    $hub = Join-Path $Destination 'hub'
    if (-not (Test-Path -LiteralPath $hub)) { Copy-Item -LiteralPath (Join-Path $repoRoot 'hub') -Destination $hub -Recurse }
    & npm ci --prefix $hub --no-audit --no-fund
    if ($LASTEXITCODE -ne 0) { throw 'Hub dependency installation failed.' }
    $hubEnv = Join-Path $hub '.env'
    if (-not (Test-Path -LiteralPath $hubEnv)) {
      $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
      function New-Secret { $bytes = New-Object byte[] 32; $rng.GetBytes($bytes); ([BitConverter]::ToString($bytes) -replace '-', '').ToLowerInvariant() }
      @('OBS_POSTGRES_DB=observability', 'OBS_POSTGRES_USER=observability', "OBS_POSTGRES_PASSWORD=$(New-Secret)",
        'OBS_MONGODB_DATABASE=agent_memory', 'OBS_MONGODB_USER=observability', "OBS_MONGODB_PASSWORD=$(New-Secret)",
        "OBS_HMAC_KEY=$(New-Secret)") | Set-Content -LiteralPath $hubEnv -Encoding ascii
      & icacls $hubEnv /inheritance:r /grant:r "$env:USERNAME`:(R,W)" | Out-Null
    }
    if (-not $SkipServices) {
      Push-Location $hub
      try { & docker compose --env-file .env up -d; if ($LASTEXITCODE -ne 0) { throw 'Docker Compose startup failed.' } } finally { Pop-Location }
    }
  }
}

if (-not $SkipCompile) {
  Invoke-Step 'compile' {
    $env:PYTHONPATH = $Destination
    $arguments = @('-m', 'compile.run', '--env-file', (Join-Path $Destination '.env'))
    if ($CompileDryRun) { $arguments += '--dry-run' }
    Push-Location $Destination
    try { & $python @arguments | Write-Host; if ($LASTEXITCODE -ne 0) { throw 'harness compile failed.' } } finally { Pop-Location }
  }
}

if (-not $SkipServices) {
  Invoke-Step 'usage.receiver' {
    $env:PYTHONPATH = $Destination
    Push-Location $Destination
    try { & $python -m core.usage.receiver ensure } finally { Pop-Location }
  }
}

if (-not $DryRun) {
  New-Item -ItemType Directory -Path (Join-Path $Destination 'var') -Force | Out-Null
  # The hook shim runs this interpreter (cmd cannot parse .env).
  [IO.File]::WriteAllText((Join-Path $Destination 'var\python.path'), $python, (New-Object Text.UTF8Encoding($false)))
  [pscustomobject]@{ install_id = $installId; destination = $Destination; python = $python; repo = $repoRoot } |
    ConvertTo-Json | Set-Content -LiteralPath (Join-Path $Destination 'var\install-state.json') -Encoding utf8
}
Write-Event 'install.finish' 'succeeded' @{ restart_agents = $true }
