$ErrorActionPreference = 'Stop'
$ToolRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$VenvPython = Join-Path $ToolRoot '.venv\Scripts\python.exe'
$Vendor = Join-Path $ToolRoot 'vendor\codex-cli-notify'

if (-not (Test-Path -LiteralPath $VenvPython)) {
    python -m venv (Join-Path $ToolRoot '.venv')
}

& $VenvPython -m pip install --disable-pip-version-check --requirement (Join-Path $ToolRoot 'requirements.txt')

$Pinned = (& git -C $Vendor rev-parse HEAD).Trim()
if ($Pinned -ne 'ff01555b99d41ff95322d06ab92d1f2650141cfc') {
    throw "Unexpected codex-cli-notify commit: $Pinned"
}

& $VenvPython (Join-Path $Vendor 'scripts\install.py') --scope global

$NotifierConfig = Join-Path $env:USERPROFILE '.codex\codex-cli-notify.json'
$Notify = Get-Content -LiteralPath $NotifierConfig -Raw | ConvertFrom-Json
$Notify.events = @('PermissionRequest', 'Stop')
$Notify | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $NotifierConfig -Encoding utf8

& $VenvPython (Join-Path $ToolRoot 'manage_hooks.py') install

Write-Output ''
Write-Output 'Installed. Restart Codex, run /hooks, and trust Codex CLI Notify plus Codex Status Orb.'
Write-Output "Diagnostics: & '$ToolRoot\doctor.ps1'"
