@echo off
rem Downloads the default models (1.6 GB) next to dictate.exe. Double-click to run.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0download-models.ps1" %*
echo.
pause
