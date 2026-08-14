# AETHERIS Windows migration/build package

1. Extract the ZIP to a short ASCII path such as `C:\AETHERIS-build`. Do not run scripts from inside the ZIP.
2. Open PowerShell as Administrator in the extracted directory.
3. Run `powershell -ExecutionPolicy Bypass -File .\packaging\windows\setup-build-environment.ps1`.
4. Close the terminal after setup so Windows refreshes `PATH`.
5. Double-click `packaging\windows\build-installer.cmd`.
6. Find the installer at `packaging\windows\output\AETHERIS-Setup-0.1.0-x64.exe`.

The environment setup requires internet access and downloads several GB of Rust, Node.js, Visual Studio Build Tools, Inno Setup and PostgreSQL. End-user computers only need the resulting Setup EXE.

See `packaging\windows\README.md` for Chinese documentation and data-directory details.

