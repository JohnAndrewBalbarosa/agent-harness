$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
foreach ($script in @('install.ps1', 'doctor.ps1', 'uninstall.ps1', 'verify-secrets.ps1')) {
  $path = Join-Path $root "scripts\$script"
  [scriptblock]::Create((Get-Content -LiteralPath $path -Raw)) | Out-Null
}

function Assert([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
# Scripts must run on Windows PowerShell 5.1 (.NET Framework): no .NET Core-only APIs.
foreach ($script in Get-ChildItem -LiteralPath (Join-Path $root 'scripts') -Filter '*.ps1') {
  $hit = Select-String -LiteralPath $script.FullName -Pattern 'RandomNumberGenerator\]::(GetInt32|GetBytes|Fill)|\[Convert\]::ToHexString' | Select-Object -First 1
  Assert (-not $hit) "PowerShell 7-only API in $($script.Name): $hit"
}

$work = Join-Path ([IO.Path]::GetTempPath()) ('agent-harness-test-' + [guid]::NewGuid().ToString('N'))
$destination = Join-Path $work 'Juan dela Cruz\.agent-harness'
$homes = Join-Path $work 'homes'
New-Item -ItemType Directory -Path (Join-Path $homes '.codex'), (Join-Path $homes '.claude') -Force | Out-Null
$envFile = Join-Path $work 'test.env'
@(
  "HARNESS_HOME=$destination",
  "INSTANCES=codex:personal=$homes\.codex;claude:default=$homes\.claude",
  'OTLP_PORT=4399'
) | Set-Content -LiteralPath $envFile -Encoding utf8
# Hermetic: inside a Herdr pane, hooks from this temp install would start Herdr daemons that lock its files.
$savedHerdr = @(Get-ChildItem env: | Where-Object Name -like 'HERDR*')
$savedHerdr | ForEach-Object { Remove-Item -LiteralPath ("env:" + $_.Name) }
try {
  # Dry run writes nothing.
  $events = @(& (Join-Path $root 'scripts\install.ps1') -Destination $destination -EnvFile $envFile -Prerequisites ValidateOnly -DryRun -SkipCompile -SkipServices -SkipOrbVenv |
    ForEach-Object { $_ | ConvertFrom-Json })
  Assert (-not (Test-Path -LiteralPath $destination)) 'Dry run wrote the runtime.'
  Assert ([bool]($events | Where-Object { $_.operation -eq 'install.finish' -and $_.outcome -eq 'succeeded' })) 'Dry run did not finish.'

  # Real install (compile skipped: covered by tests/integration/test_compile_temp_home.py).
  & (Join-Path $root 'scripts\install.ps1') -Destination $destination -EnvFile $envFile -Prerequisites ValidateOnly -SkipCompile -SkipServices -SkipOrbVenv | Out-Null
  Assert (-not (Test-Path -LiteralPath (Join-Path $destination 'tools\observability-client\var'))) 'Installer copied the repo checkout runtime state (tools\observability-client\var).'
  foreach ($item in @('herdr-plugin\herdr-plugin.toml', 'core\harness.cmd', 'policy\RTK.md', 'core\router\harness-hook.cmd','adapters\claude\adapter.py', 'compile\run.py', 'tools\observability-client\obs.py', '.env', 'policy\AGENTS.md')) {
    Assert (Test-Path -LiteralPath (Join-Path $destination $item)) "Missing after install: $item"
  }
  $policy = Get-Content -LiteralPath (Join-Path $destination 'policy\AGENTS.md') -Raw
  Assert ($policy.Contains($destination)) 'Policy was not rendered with HARNESS_HOME.'
  Assert (-not $policy.Contains('{{HARNESS_HOME}}')) 'Policy still contains placeholders.'

  # Re-install keeps owner-owned files and state.
  Add-Content -LiteralPath (Join-Path $destination 'policy\AGENTS.md') -Value 'OWNER EDIT'
  New-Item -ItemType Directory -Path (Join-Path $destination 'var') -Force | Out-Null
  Set-Content -LiteralPath (Join-Path $destination 'var\keep.txt') -Value 'state'
  $obsVar = Join-Path $destination 'tools\observability-client\var'
  New-Item -ItemType Directory -Path $obsVar, (Join-Path $destination 'tools\codex-status-orb\.venv') -Force | Out-Null
  Set-Content -LiteralPath (Join-Path $obsVar 'hmac.key') -Value 'OWNER-KEY' -NoNewline
  Set-Content -LiteralPath (Join-Path $destination 'tools\codex-status-orb\.venv\marker.txt') -Value 'venv'
  & (Join-Path $root 'scripts\install.ps1') -Destination $destination -EnvFile $envFile -Prerequisites ValidateOnly -SkipCompile -SkipServices -SkipOrbVenv | Out-Null
  Assert ((Get-Content -LiteralPath (Join-Path $destination 'policy\AGENTS.md') -Raw).Contains('OWNER EDIT')) 'Re-install overwrote the owner policy.'
  Assert (Test-Path -LiteralPath (Join-Path $destination 'var\keep.txt')) 'Re-install removed var/ state.'
  Assert ((Get-Content -LiteralPath (Join-Path $obsVar 'hmac.key') -Raw) -eq 'OWNER-KEY') 'Re-install replaced or removed the observability client state (hmac.key / spool).'
  Assert (Test-Path -LiteralPath (Join-Path $destination 'tools\codex-status-orb\.venv\marker.txt')) 'Re-install removed the status orb venv.'
  Assert (Test-Path -LiteralPath (Join-Path $destination 'tools\observability-client\obs.py')) 'Re-install lost tool code.'

  # The installed router runs from the installed tree (paths with spaces), even when the agent's working
  # directory contains its own 'core' package (python -m would otherwise import that one).
  $project = Join-Path $work 'project with core'
  New-Item -ItemType Directory -Path (Join-Path $project 'core\router') -Force | Out-Null
  Set-Content -LiteralPath (Join-Path $project 'core\__init__.py') -Value 'raise SystemExit("wrong core imported")'
  Push-Location $project
  try { $out = '{"hook_event_name":"Stop","session_id":"t"}' | & cmd /c "`"$(Join-Path $destination 'core\router\harness-hook.cmd')`" codex" }
  finally { Pop-Location }
  Assert (($out -join '').Contains('"continue": true')) "Installed router did not answer Codex Stop: $out"

  # The shim runs the interpreter the installer resolved (var\python.path), not a hardcoded path.
  $pinned = Join-Path $destination 'var\python.path'
  Assert (Test-Path -LiteralPath $pinned) 'Installer did not record the resolved python (var\python.path).'
  $stub = Join-Path $work 'stub python.cmd'
  Set-Content -LiteralPath $stub -Value '@echo {"stub": true}' -Encoding ascii
  $realPython = Get-Content -LiteralPath $pinned -Raw
  Set-Content -LiteralPath $pinned -Value $stub -NoNewline -Encoding ascii
  $savedPython = $env:HARNESS_PYTHON; Remove-Item Env:HARNESS_PYTHON -ErrorAction SilentlyContinue
  try { $out = '{}' | & cmd /c "`"$(Join-Path $destination 'core\router\harness-hook.cmd')`" codex" }
  finally { if ($savedPython) { $env:HARNESS_PYTHON = $savedPython }; Set-Content -LiteralPath $pinned -Value $realPython -NoNewline -Encoding ascii }
  Assert (($out -join '').Contains('"stub": true')) "Shim ignored var\python.path: $out"

  # Doctor reports a healthy install (services are not started in tests, so they are informational).
  $report = & (Join-Path $root 'scripts\doctor.ps1') -Destination $destination | ConvertFrom-Json
  Assert ($report.status -eq 'healthy') "Doctor reported: $($report | ConvertTo-Json -Compress)"

  # Uninstall removes code but keeps owner files.
  & (Join-Path $root 'scripts\uninstall.ps1') -Destination $destination | Out-Null
  Assert (-not (Test-Path -LiteralPath (Join-Path $destination 'core'))) 'Uninstall left code behind.'
  Assert (Test-Path -LiteralPath (Join-Path $destination 'policy\AGENTS.md')) 'Uninstall removed the owner policy.'

  # Portable sources: no absolute user-profile paths outside internal dev docs.
  $sources = Get-ChildItem -LiteralPath $root -Recurse -File | Where-Object {
    $_.FullName -notmatch '\\(var|\.git|\.superpowers|node_modules|__pycache__|docs\\superpowers)\\' -and $_.Extension -notin '.png', '.mp3' -and $_.FullName -ne $PSCommandPath }
  $leaks = @($sources | Select-String -Pattern '[A-Za-z]:\\Users\\(?![%{<]|Juan dela Cruz|x\\)[A-Za-z0-9._-]+\\' | Select-Object -First 3)
  Assert (-not $leaks) "Non-portable path detected: $($leaks -join ' | ')"

  [pscustomobject]@{ syntax = 'passed'; dry_run = 'passed'; install = 'passed'; reinstall = 'passed'; router = 'passed'; portable_sources = 'passed' } | ConvertTo-Json -Compress
} finally {
  $savedHerdr | ForEach-Object { Set-Item -LiteralPath ("env:" + $_.Name) -Value $_.Value }
  # Consumers may leave detached helpers (e.g. the obs spool flush) running from the temp install.
  Get-CimInstance Win32_Process | Where-Object { $_.ProcessId -ne $PID -and $_.CommandLine -and $_.CommandLine.Contains($work) } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
  if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
}
