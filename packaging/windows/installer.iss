#define AppName "AETHERIS"
#ifndef AppVersion
  #define AppVersion "0.3.0"
#endif
#define Publisher "AETHERIS Medical Imaging Cloud"

[Setup]
AppId={{0C410F2B-A20E-4EBD-A65B-101F110AC459}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#Publisher}
DefaultDirName={autopf}\AETHERIS
DefaultGroupName=AETHERIS
OutputDir=output
OutputBaseFilename=AETHERIS-Setup-{#AppVersion}-x64
SetupIconFile=..\..\apps\viewer\src-tauri\icons\icon.ico
Compression=lzma2/ultra64
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin
UninstallDisplayIcon={app}\AETHERIS.exe
WizardStyle=modern

[Files]
Source: "stage\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "initialize.ps1"; DestDir: "{tmp}"; Flags: deleteafterinstall

[Icons]
Name: "{autodesktop}\AETHERIS"; Filename: "{app}\aetheris-launcher.exe"; WorkingDir: "{app}"
Name: "{group}\AETHERIS"; Filename: "{app}\aetheris-launcher.exe"; WorkingDir: "{app}"
Name: "{group}\卸载 AETHERIS"; Filename: "{uninstallexe}"

[Run]
Filename: "{app}\aetheris-launcher.exe"; Description: "启动 AETHERIS"; Flags: nowait postinstall skipifsilent

[Code]
procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
  Parameters: String;
begin
  if CurStep = ssPostInstall then
  begin
    WizardForm.StatusLabel.Caption := '正在配置数据库、证书和管理员账号...';
    Parameters := '-NoLogo -NoProfile -ExecutionPolicy Bypass -File "' +
      ExpandConstant('{tmp}\initialize.ps1') + '" -InstallDir "' +
      ExpandConstant('{app}') + '"';
    if not Exec('powershell.exe', Parameters, '', SW_SHOW, ewWaitUntilTerminated, ResultCode) then
      RaiseException('无法启动 AETHERIS 初始化程序。');
    if ResultCode <> 0 then
      RaiseException(Format('AETHERIS 初始化未完成（退出码 %d）。请重新运行安装程序。', [ResultCode]));
  end;
end;
