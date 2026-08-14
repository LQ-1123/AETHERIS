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
echo Done: packaging\windows\output\AETHERIS-Setup-0.1.0-x64.exe
pause

