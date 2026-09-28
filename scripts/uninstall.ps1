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
foreach ($dir in @('core', 'adapters', 'compile', 'tools', 'hub')) {
  $target = Join-Path $Destination $dir
  if (Test-Path -LiteralPath $target) { Remove-Item -LiteralPath $target -Recurse -Force }
}
if ($Purge) { Remove-Item -LiteralPath $Destination -Recurse -Force }
[pscustomobject]@{ destination = $Destination; purged = [bool]$Purge; agent_configs = 'unchanged; see var\backups\compile-*' } | ConvertTo-Json -Compress
