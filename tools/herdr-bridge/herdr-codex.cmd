@echo off
setlocal
python.exe "%~dp0launcher.py" %*
exit /b %ERRORLEVEL%
