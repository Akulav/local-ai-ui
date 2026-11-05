@echo off
rem ---------------------------------------------------------------------------
rem  Lumen - one-click build & run (made by Akulav)
rem
rem    run.bat          install missing tools, build the exe and the installer, copy them to
rem                     dist\ (dist\Lumen\Lumen.exe + dist\Lumen_X.Y.Z_x64-setup.exe), launch
rem    run.bat build    same, but do not launch
rem    run.bat quick    portable exe only (faster, no installer), then launch
rem    run.bat dev      start the app in dev mode (hot reload for ui\)
rem    run.bat clean    remove build outputs (keeps dist\Lumen\LumenData)
rem
rem  Missing tools (Node.js, Rust, VS C++ Build Tools) are installed with winget.
rem  The Build Tools installer asks for administrator rights once (UAC prompt).
rem ---------------------------------------------------------------------------
setlocal
cd /d "%~dp0"
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\build.ps1" %*
set ERR=%ERRORLEVEL%
if not "%ERR%"=="0" (
  echo.
  echo Build failed with code %ERR%. See the messages above.
  pause
)
exit /b %ERR%
