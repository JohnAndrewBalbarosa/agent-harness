@echo off
rem Subcommands ported to the Rust CLI (obs-rs, crates/obs-cli) run there; everything else stays in obs.py.
set "OBS_RS=%~dp0..\..\bin\obs-rs.exe"
if exist "%OBS_RS%" (
  if /i "%~1"=="errors" goto run_rust
  if /i "%~1"=="diagnose" goto run_rust
  if /i "%~1"=="trace" goto run_rust
  if /i "%~1"=="analytics" goto run_rust
  if /i "%~1"=="change" goto run_rust
  if /i "%~1"=="flush" goto run_rust
  if /i "%~1"=="query" goto run_rust
  if /i "%~1"=="prompt" if /i "%~2"=="show" goto run_rust
  if /i "%~1"=="project" if /i "%~2"=="register" goto run_rust
)
set "OBS_PYTHON=%~dp0.venv-benchmark\Scripts\python.exe"
if not exist "%OBS_PYTHON%" if defined HARNESS_PYTHON set "OBS_PYTHON=%HARNESS_PYTHON%"
if not exist "%OBS_PYTHON%" set "OBS_PYTHON=%APPDATA%\uv\python\cpython-3.14.7-windows-x86_64-none\python.exe"
"%OBS_PYTHON%" "%~dp0obs.py" %*
exit /b %ERRORLEVEL%

:run_rust
rem OBS_HUB_DIR keeps obs-rs on this directory's spool (change counter lives there).
set "OBS_HUB_DIR=%~dp0"
"%OBS_RS%" %*
exit /b %ERRORLEVEL%
