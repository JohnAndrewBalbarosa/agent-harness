$ErrorActionPreference = 'Stop'
& python.exe (Join-Path $PSScriptRoot '..\cli.py') focus-priority
exit $LASTEXITCODE

