param(
    [string]$PostgresDir = ''
)
$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$stage = Join-Path $PSScriptRoot 'stage'
$tauriConfig = Join-Path $root 'apps\viewer\src-tauri\tauri.conf.json'
$appVersion = (Get-Content $tauriConfig -Raw | ConvertFrom-Json).version
if (-not $appVersion) { throw '无法从 tauri.conf.json 读取版本号。' }
if (-not $PostgresDir) {
    $PostgresDir = Get-ChildItem 'C:\Program Files\PostgreSQL' -Directory -ErrorAction SilentlyContinue |
        Sort-Object { [int]$_.Name } -Descending |
        Select-Object -First 1 -ExpandProperty FullName
}
if (-not $PostgresDir) { throw '没有找到 PostgreSQL。请先运行 setup-build-environment.ps1，或传入 -PostgresDir。' }
if (-not (Test-Path (Join-Path $PostgresDir 'bin\initdb.exe'))) { throw 'PostgresDir 必须指向解压后的 Windows x64 PostgreSQL 目录' }

Push-Location $root
try {
    cargo build --release -p pacsd -p aetheris-launcher
    if ($LASTEXITCODE -ne 0) { throw '服务端或启动器编译失败' }
    Push-Location (Join-Path $root 'apps\viewer')
    try {
        npm ci
        npm run tauri build -- --no-bundle
        if ($LASTEXITCODE -ne 0) { throw 'Viewer 编译失败' }
    } finally { Pop-Location }

    if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
    New-Item -ItemType Directory $stage | Out-Null
    Copy-Item (Join-Path $root 'target\release\pacsd.exe') $stage
    Copy-Item (Join-Path $root 'target\release\aetheris-launcher.exe') $stage
    Copy-Item (Join-Path $root 'apps\viewer\src-tauri\target\release\pacs-viewer.exe') (Join-Path $stage 'AETHERIS.exe')
    $pgStage = Join-Path $stage 'postgres'
    New-Item -ItemType Directory $pgStage | Out-Null
    foreach ($directory in @('bin', 'lib', 'share')) {
        Copy-Item (Join-Path $PostgresDir $directory) $pgStage -Recurse
    }
    Get-ChildItem $PostgresDir -File | Where-Object { $_.Name -match '^(COPYRIGHT|LICENSE|README)' } |
        Copy-Item -Destination $pgStage
    Copy-Item (Join-Path $root 'apps\viewer\ai-plugins') $stage -Recurse

    # pacsd 通过 vcpkg 链接 libarchive（compress-tools），运行时需要其 DLL 同在 exe 目录
    if ($env:VCPKG_INSTALLATION_ROOT -and (Test-Path (Join-Path $env:VCPKG_INSTALLATION_ROOT 'installed\x64-windows\bin'))) {
        Get-ChildItem (Join-Path $env:VCPKG_INSTALLATION_ROOT 'installed\x64-windows\bin') -Filter '*.dll' |
            Copy-Item -Destination $stage -Force
        Write-Host '已复制 vcpkg 运行时 DLL（libarchive 等）'
    }

    & "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe" "/DAppVersion=$appVersion" (Join-Path $PSScriptRoot 'installer.iss')
    if ($LASTEXITCODE -ne 0) { throw 'Inno Setup 编译失败' }
    $installer = Join-Path $PSScriptRoot "output\AETHERIS-Setup-$appVersion-x64.exe"
    if (-not (Test-Path $installer)) { throw "未找到预期安装包：$installer" }
    Write-Host "安装包已生成：$installer"
} finally { Pop-Location }
