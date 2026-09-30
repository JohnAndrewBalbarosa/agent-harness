@echo off
if not exist "%~dp0..\..\bin\interval-supervisor.exe" goto python_fallback
"%~dp0..\..\bin\interval-supervisor.exe" %*
exit /b %ERRORLEVEL%
:python_fallback
if not defined HARNESS_PYTHON set "HARNESS_PYTHON=%APPDATA%\uv\python\cpython-3.14.7-windows-x86_64-none\python.exe"
"%HARNESS_PYTHON%" "%~dp0interval_supervisor.py" %*
