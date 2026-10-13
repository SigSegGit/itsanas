@echo off
rem Double-click, or `scripts\release.cmd -Pr 271`: merge, tag, wait, sign.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0release.ps1" %*
echo.
pause
