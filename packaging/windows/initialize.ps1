param([Parameter(Mandatory = $true)][string]$InstallDir)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$existingConfig = Join-Path $env:ProgramData 'AETHERIS\server.env'
if (Test-Path $existingConfig) {
    [System.Windows.Forms.MessageBox]::Show('检测到已有 AETHERIS 数据和配置，本次安装已保留原数据库与账号。', 'AETHERIS 安装完成', 'OK', 'Information') | Out-Null
    exit 0
}

function Fail([string]$Message) {
    [System.Windows.Forms.MessageBox]::Show($Message, 'AETHERIS 初始化失败', 'OK', 'Error') | Out-Null
    exit 1
}

$form = New-Object Windows.Forms.Form
$form.Text = 'AETHERIS 初始账号设置'
$form.Size = New-Object Drawing.Size(470, 300)
$form.StartPosition = 'CenterScreen'
$form.FormBorderStyle = 'FixedDialog'
$form.MaximizeBox = $false
$form.MinimizeBox = $false

$title = New-Object Windows.Forms.Label
$title.Text = '设置系统管理员账号'
$title.Font = New-Object Drawing.Font('Microsoft YaHei UI', 14, [Drawing.FontStyle]::Bold)
$title.Location = New-Object Drawing.Point(28, 22)
$title.AutoSize = $true
$form.Controls.Add($title)

$hint = New-Object Windows.Forms.Label
$hint.Text = '用户名至少 3 位；密码为 12–128 个字符。安装程序会同时初始化数据库和 TLS 证书。'
$hint.Location = New-Object Drawing.Point(30, 60)
$hint.Size = New-Object Drawing.Size(400, 42)
$form.Controls.Add($hint)

$userLabel = New-Object Windows.Forms.Label
$userLabel.Text = '管理员用户名'
$userLabel.Location = New-Object Drawing.Point(30, 112)
$userLabel.AutoSize = $true
$form.Controls.Add($userLabel)
$userBox = New-Object Windows.Forms.TextBox
$userBox.Text = 'admin'
$userBox.Location = New-Object Drawing.Point(160, 108)
$userBox.Size = New-Object Drawing.Size(260, 25)
$form.Controls.Add($userBox)

$passwordLabel = New-Object Windows.Forms.Label
$passwordLabel.Text = '管理员密码'
$passwordLabel.Location = New-Object Drawing.Point(30, 151)
$passwordLabel.AutoSize = $true
$form.Controls.Add($passwordLabel)
$passwordBox = New-Object Windows.Forms.TextBox
$passwordBox.UseSystemPasswordChar = $true
$passwordBox.Location = New-Object Drawing.Point(160, 147)
$passwordBox.Size = New-Object Drawing.Size(260, 25)
$form.Controls.Add($passwordBox)

$ok = New-Object Windows.Forms.Button
$ok.Text = '初始化并完成安装'
$ok.Location = New-Object Drawing.Point(260, 205)
$ok.Size = New-Object Drawing.Size(160, 34)
$ok.DialogResult = [Windows.Forms.DialogResult]::OK
$form.AcceptButton = $ok
$form.Controls.Add($ok)

while ($true) {
    if ($form.ShowDialog() -ne [Windows.Forms.DialogResult]::OK) { exit 2 }
    $username = $userBox.Text.Trim()
    $password = $passwordBox.Text
    if ($username -notmatch '^[a-z0-9._-]{3,64}$') {
        [Windows.Forms.MessageBox]::Show('用户名只能包含小写字母、数字、点、下划线和连字符，长度至少 3 位。', '输入有误', 'OK', 'Warning') | Out-Null
        continue
    }
    if ($password.Length -lt 12 -or $password.Length -gt 128) {
        [Windows.Forms.MessageBox]::Show('密码长度必须为 12–128 个字符。', '输入有误', 'OK', 'Warning') | Out-Null
        continue
    }
    break
}

try {
    $dataDir = Join-Path $env:ProgramData 'AETHERIS'
    $pgData = Join-Path $dataDir 'postgres'
    $storage = Join-Path $dataDir 'storage'
    $logs = Join-Path $dataDir 'logs'
    New-Item -ItemType Directory -Force $dataDir, $storage, $logs | Out-Null
    $envFile = Join-Path $dataDir 'server.env'

    $dbPassword = ([Guid]::NewGuid().ToString('N') + [Guid]::NewGuid().ToString('N'))
    $jwtSecret = ([Guid]::NewGuid().ToString('N') + [Guid]::NewGuid().ToString('N'))
    $pwFile = Join-Path $env:TEMP ('aetheris-pg-' + [Guid]::NewGuid().ToString('N') + '.txt')
    [IO.File]::WriteAllText($pwFile, $dbPassword, [Text.UTF8Encoding]::new($false))

    $pgBin = Join-Path $InstallDir 'postgres\bin'
    if (-not (Test-Path (Join-Path $pgBin 'initdb.exe'))) { throw '安装包缺少 PostgreSQL 运行时' }
    if (-not (Test-Path (Join-Path $pgData 'PG_VERSION'))) {
        & (Join-Path $pgBin 'initdb.exe') -D $pgData -U pacs --pwfile=$pwFile --auth=scram-sha-256 --encoding=UTF8 --locale=C
        if ($LASTEXITCODE -ne 0) { throw "数据库初始化失败：$LASTEXITCODE" }
        Add-Content (Join-Path $pgData 'postgresql.conf') "`nport = 55432`nlisten_addresses = '127.0.0.1'`n"
    }
    Remove-Item $pwFile -Force -ErrorAction SilentlyContinue

    & (Join-Path $pgBin 'pg_ctl.exe') start -w -D $pgData -l (Join-Path $logs 'postgres.log')
    if ($LASTEXITCODE -ne 0) { throw "数据库启动失败：$LASTEXITCODE" }
    $env:PGPASSWORD = $dbPassword
    & (Join-Path $pgBin 'createdb.exe') -h 127.0.0.1 -p 55432 -U pacs -O pacs pacs
    if ($LASTEXITCODE -ne 0) { throw "创建 PACS 数据库失败：$LASTEXITCODE" }

    $envLines = @(
        "DATABASE_URL=postgres://pacs:$dbPassword@127.0.0.1:55432/pacs"
        "PACS_STORAGE_ROOT=$storage"
        'PACS_DIMSE_BIND=127.0.0.1:11112'
        'PACS_AE_TITLE=REMOTE_PACS'
        'PACS_HTTP_BIND=127.0.0.1:8443'
        "PACS_JWT_SECRET=$jwtSecret"
        'RUST_LOG=info,pacsd=info'
    )
    [IO.File]::WriteAllLines($envFile, $envLines, [Text.UTF8Encoding]::new($false))

    Get-Content $envFile | ForEach-Object {
        if ($_ -match '^([^#=]+)=(.*)$') { [Environment]::SetEnvironmentVariable($matches[1], $matches[2], 'Process') }
    }
    & (Join-Path $InstallDir 'pacsd.exe') admin --username $username --password $password
    if ($LASTEXITCODE -ne 0) { throw "管理员账号初始化失败：$LASTEXITCODE" }
    [System.Windows.Forms.MessageBox]::Show('环境配置、数据库和管理员账号均已初始化完成。', 'AETHERIS 安装完成', 'OK', 'Information') | Out-Null
} catch {
    Remove-Item $pwFile -Force -ErrorAction SilentlyContinue
    if ($pgBin -and $pgData -and (Test-Path $pgData)) {
        & (Join-Path $pgBin 'pg_ctl.exe') stop -D $pgData -m fast 2>$null | Out-Null
        Remove-Item $pgData -Recurse -Force -ErrorAction SilentlyContinue
    }
    if ($envFile) { Remove-Item $envFile -Force -ErrorAction SilentlyContinue }
    Fail $_.Exception.Message
}
