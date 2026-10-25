; Nexus Monitor — Guion de Instalador de Producción Inno Setup 6
#define MyAppName "Nexus Monitor"
#define MyAppVersion "0.4.0"
#define MyAppPublisher "Freddy"
#define MyAppExeName "Nexus.exe"

[Setup]
AppId={{5A8D39E1-72B9-4C45-A674-F8B2D5D9E022}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={localappdata}\Programs\Nexus
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=dist
OutputBaseFilename=NexusMonitor_Setup_v0.4.0
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\{#MyAppExeName}
CloseApplications=force

[Languages]
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl"

[Tasks]
Name: "desktopicon"; Description: "Crear un acceso directo en el escritorio"; GroupDescription: "Accesos directos adicionales:"; Flags: unchecked
Name: "autostart"; Description: "Iniciar Nexus con Windows automáticamente"; GroupDescription: "Opciones del sistema:"

[Files]
Source: "target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "nexus.toml"; DestDir: "{app}"; Flags: ignoreversion onlyifdoesntexist

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\Desinstalar {#MyAppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "NexusMonitor"; ValueData: """{app}\{#MyAppExeName}"""; Flags: uninsdeletevalue; Tasks: autostart

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
Type: files; Name: "{app}\nexus.toml"
Type: dirifempty; Name: "{app}"

[Code]
// Procedimiento para cerrar cualquier instancia de Nexus antes de desinstalar
function InitializeUninstall(): Boolean;
var
  ErrorCode: Integer;
begin
  ShellExec('open', 'powershell.exe', '-Command "Get-Process Nexus -ErrorAction SilentlyContinue | Stop-Process -Force"', '', SW_HIDE, ewWaitUntilTerminated, ErrorCode);
  Result := True;
end;
