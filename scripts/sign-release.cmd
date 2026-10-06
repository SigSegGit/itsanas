@echo off
rem Double-click to sign and publish the release CI just built.
rem A .ps1 opens in Notepad when double-clicked, and the execution policy
rem refuses unsigned scripts; this wrapper does neither and keeps the window
rem open at the end so the result can be read.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0sign-release.ps1" %*
echo.
pause
