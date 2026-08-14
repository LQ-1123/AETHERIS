# AETHERIS Windows 迁移与打包

这个目录是 Windows 构建迁移包，不包含本机数据库、DICOM 影像、密码、编译缓存或 macOS 产物。

## 在一台全新的 Windows 10/11 x64 电脑上

1. 将 ZIP 解压到纯英文且路径较短的位置，例如 `C:\AETHERIS-build`。不要直接在 ZIP 内运行脚本。
2. 右键打开 Windows Terminal/PowerShell，选择“以管理员身份运行”。
3. 进入解压目录，执行：

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\packaging\windows\setup-build-environment.ps1
   ```

4. 环境脚本结束后关闭终端，以便刷新 PATH。
5. 双击 `packaging\windows\build-installer.cmd`（同时保留了中文名称的入口）。
6. 成品位于 `packaging\windows\output\AETHERIS-Setup-0.1.0-x64.exe`。

环境准备阶段需要连接互联网并下载 Rust、Node.js、Visual Studio Build Tools、Inno Setup 和 PostgreSQL，总下载量可能达到数 GB。Windows 用户最终只需要第 6 步生成的安装包，不需要这些开发工具。

更详细的构建、安装和数据目录说明见 `packaging\windows\README.md`。
