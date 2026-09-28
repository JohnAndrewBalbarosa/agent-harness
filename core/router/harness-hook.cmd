@echo off
rem agent-harness entry point: harness-hook <agent> [<native-event>]  (hook payload on stdin)
setlocal
if not defined HARNESS_PYTHON if exist "%~dp0..\..\var\python.path" set /p HARNESS_PYTHON=<"%~dp0..\..\var\python.path"
if not defined HARNESS_PYTHON set "HARNESS_PYTHON=%APPDATA%\uv\python\cpython-3.14.7-windows-x86_64-none\python.exe"
if not exist "%HARNESS_PYTHON%" set "HARNESS_PYTHON=python"
set "PYTHONUTF8=1"
set "PYTHONPATH=%~dp0..\.."
rem python -m puts the working directory first on sys.path: run from HARNESS_HOME so an agent project's own
rem 'core' package can never shadow the harness (the agent's cwd still arrives in the hook payload).
pushd "%~dp0..\.."
"%HARNESS_PYTHON%" -m core.router.harness_hook %* 2>nul
popd
exit /b 0
