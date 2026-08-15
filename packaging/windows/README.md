# Windows 一体化安装包

生成的 `AETHERIS-Setup-0.2.0-x64.exe` 内含 Viewer、PACS 服务端和 PostgreSQL，目标电脑无需安装开发环境。安装时会初始化数据库、TLS 证书和管理员账号，并创建桌面/开始菜单快捷方式。

## 构建条件

- Windows 10/11 x64
- Rust 1.97.1（含 MSVC target）、Node.js 20+、Visual Studio Build Tools
- Inno Setup 6
- 解压后的 PostgreSQL 16/17 Windows x64 binaries（目录中应有 `bin\initdb.exe`）

首次使用时，以管理员身份运行：

```powershell
powershell -ExecutionPolicy Bypass -File .\packaging\windows\setup-build-environment.ps1
```

安装 PostgreSQL 时如果官方安装器要求设置服务密码，可以设置一个临时构建机密码；该服务及密码不会进入最终 AETHERIS 安装包。环境安装完成后关闭窗口，双击 `打包安装包.cmd`。也可以在仓库根目录手工运行：

```powershell
.\packaging\windows\build.ps1
```

脚本会自动选择 `C:\Program Files\PostgreSQL` 下版本最高的安装目录。使用自行解压的 PostgreSQL 时可传入 `-PostgresDir C:\build\pgsql`。

安装器输出在 `packaging\windows\output`。构建过程需要开发工具；最终安装包在用户电脑上不需要这些工具。

## 运行数据

程序文件位于 `C:\Program Files\AETHERIS`，数据库、影像、配置和日志位于 `C:\ProgramData\AETHERIS`。数据库仅监听 `127.0.0.1:55432`，HTTP 和 DIMSE 默认也只监听本机。卸载程序默认保留 `ProgramData` 下的医疗数据，避免误删影像；如需彻底清除，应先备份后由管理员手工删除。
