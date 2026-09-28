$ErrorActionPreference = 'Continue'
$ToolRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$Python = Join-Path $ToolRoot '.venv\Scripts\python.exe'

Get-CimInstance Win32_Process | Where-Object {
    $_.CommandLine -like "*$ToolRoot*orb.py*"
} | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }

if (Test-Path -LiteralPath $Python) {
    & $Python (Join-Path $ToolRoot 'manage_hooks.py') uninstall
    & $Python (Join-Path $ToolRoot 'vendor\codex-cli-notify\scripts\uninstall.py') --scope global --remove-config
}

Write-Output 'Codex Status Orb and notifier hooks removed. Tool source and recovery backups were preserved.'
