@echo off
rem agent-harness operator CLI: harness agents | harness preflight <agent> [--init]
setlocal
if not defined HARNESS_PYTHON if exist "%~dp0..\var\python.path" set /p HARNESS_PYTHON=<"%~dp0..\var\python.path"
if not defined HARNESS_PYTHON set "HARNESS_PYTHON=python"
set "PYTHONUTF8=1"
set "PYTHONPATH=%~dp0.."
"%HARNESS_PYTHON%" -m core.cli %*
exit /b %errorlevel%
