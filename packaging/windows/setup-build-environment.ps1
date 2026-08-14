$ErrorActionPreference = 'Stop'

if (-not (Get-Command winget.exe -ErrorAction SilentlyContinue)) {
    throw '未找到 winget。请先从 Microsoft Store 安装或更新“应用安装程序”，然后重新运行。'
}

function Install-Package([string]$Id, [string]$Name, [string[]]$Extra = @()) {
    Write-Host "正在安装/检查 $Name ..." -ForegroundColor Cyan
    $arguments = @('install', '--id', $Id, '--exact', '--accept-package-agreements',
        '--accept-source-agreements', '--disable-interactivity') + $Extra
    & winget.exe @arguments
    if ($LASTEXITCODE -ne 0) { throw "$Name 安装失败（退出码 $LASTEXITCODE）" }
}

Install-Package 'Rustlang.Rustup' 'Rust 工具链'
Install-Package 'OpenJS.NodeJS.LTS' 'Node.js LTS'
Install-Package 'JRSoftware.InnoSetup' 'Inno Setup 6'
Install-Package 'PostgreSQL.PostgreSQL.17' 'PostgreSQL 17 Windows x64'
Install-Package 'Microsoft.VisualStudio.2022.BuildTools' 'Visual Studio 2022 Build Tools' @(
    '--override', '--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended'
)

$env:Path = "$env:USERPROFILE\.cargo\bin;C:\Program Files\nodejs;$env:Path"
if (Test-Path "$env:USERPROFILE\.cargo\bin\rustup.exe") {
    & "$env:USERPROFILE\.cargo\bin\rustup.exe" toolchain install 1.97.1 --profile minimal
    if ($LASTEXITCODE -ne 0) { throw 'Rust 1.97.1 下载或安装失败' }
}

Write-Host ''
Write-Host '构建环境已准备完成。请关闭此窗口，然后双击“打包安装包.cmd”。' -ForegroundColor Green
Read-Host '按 Enter 退出'

