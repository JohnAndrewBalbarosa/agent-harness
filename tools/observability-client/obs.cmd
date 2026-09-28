@echo off
set "OBS_PYTHON=%~dp0.venv-benchmark\Scripts\python.exe"
if not exist "%OBS_PYTHON%" if defined HARNESS_PYTHON set "OBS_PYTHON=%HARNESS_PYTHON%"
if not exist "%OBS_PYTHON%" set "OBS_PYTHON=%APPDATA%\uv\python\cpython-3.14.7-windows-x86_64-none\python.exe"
"%OBS_PYTHON%" "%~dp0obs.py" %*
