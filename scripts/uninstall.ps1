[CmdletBinding()]
param(
  [string]$Destination = (Join-Path ([Environment]::GetFolderPath('UserProfile')) '.agent-harness'),
  [switch]$Purge
)
# Removes installed agent-harness code. Owner files (.env, policy\, var\ incl. compile backups) stay unless -Purge.
# Agent config entries written by `harness compile` are NOT reverted here: restore them from
# <Destination>\var\backups\compile-<timestamp>\ (or run compile after removing the instance from .env).
$ErrorActionPreference = 'Stop'
$Destination = [IO.Path]::GetFullPath($Destination)
if (-not (Test-Path -LiteralPath (Join-Path $Destination 'var\install-state.json'))) {
  throw 'agent-harness install state was not found; refusing an unscoped uninstall.'
}
# Stop harness processes running from this install (usage receiver, console monitor, spool flush, orb) first:
# Windows cannot delete files they hold open.
$stopped = @(Get-CimInstance Win32_Process | Where-Object { $_.ProcessId -ne $PID -and $_.CommandLine -and $_.CommandLine.Contains($Destination) })
$stopped | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
if ($stopped) { Start-Sleep -Milliseconds 500 }
foreach ($dir in @('core', 'adapters', 'compile', 'tools', 'hub', 'herdr-plugin')) {
  $target = Join-Path $Destination $dir
  if (Test-Path -LiteralPath $target) { Remove-Item -LiteralPath $target -Recurse -Force }
}
if ($Purge) { Remove-Item -LiteralPath $Destination -Recurse -Force }
[pscustomobject]@{ destination = $Destination; stopped_processes = $stopped.Count; purged = [bool]$Purge; agent_configs = 'unchanged; see var\backups\compile-*' } | ConvertTo-Json -Compress
