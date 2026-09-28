$ErrorActionPreference = 'Continue'
while ($true) {
    Clear-Host
    Write-Host 'Shared Codex Runtime' -ForegroundColor Cyan
    Write-Host 'Canonical priority: blocked > needs_input > ready > working > idle'
    Write-Host ''
    & python.exe (Join-Path $PSScriptRoot '..\cli.py') dashboard
    Write-Host ''
    Write-Host 'Refresh: Enter    Exit: Ctrl+C' -ForegroundColor DarkGray
    Read-Host | Out-Null
}
