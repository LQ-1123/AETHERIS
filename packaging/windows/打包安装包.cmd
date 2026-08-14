@echo off
setlocal
cd /d "%~dp0\..\.."
set "PATH=%USERPROFILE%\.cargo\bin;C:\Program Files\nodejs;%PATH%"
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File ".\packaging\windows\build.ps1"
if errorlevel 1 (
  echo.
  echo 打包失败，请查看上面的错误信息。
  pause
  exit /b 1
)
echo.
echo 打包完成：packaging\windows\output\AETHERIS-Setup-0.1.0-x64.exe
pause

