$ErrorActionPreference = 'Continue'
$ToolRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
& (Join-Path $ToolRoot '.venv\Scripts\python.exe') (Join-Path $ToolRoot 'doctor.py')
exit $LASTEXITCODE
