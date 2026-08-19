@echo off
setlocal
cd /d "%~dp0\..\.."
set "PATH=%USERPROFILE%\.cargo\bin;C:\Program Files\nodejs;%PATH%"
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File ".\packaging\windows\build.ps1"
if errorlevel 1 (
  echo.
  echo Build failed. See the error above.
  pause
  exit /b 1
)
echo.
echo Done. See packaging\windows\output for the versioned installer.
pause
