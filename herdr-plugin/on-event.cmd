@echo off
rem agent-harness Herdr plugin entry: on-event <agent-detected|services>. Runs from HARNESS_HOME (parent of this folder).
setlocal
if not defined HARNESS_PYTHON if exist "%~dp0..\var\python.path" set /p HARNESS_PYTHON=<"%~dp0..\var\python.path"
if not defined HARNESS_PYTHON set "HARNESS_PYTHON=python"
set "PYTHONUTF8=1"
set "PYTHONPATH=%~dp0.."
pushd "%~dp0.."
if /i "%~1"=="services" (
  "%HARNESS_PYTHON%" -m core.services.bootstrap
) else (
  "%HARNESS_PYTHON%" -m core.herdr_events %*
)
popd
exit /b 0
