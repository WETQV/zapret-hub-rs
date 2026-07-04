#define AppName "Zapret Hub"
#ifndef AppVersion
  #define CargoTomlPath AddBackslash(SourcePath) + "..\Cargo.toml"
  #define AppVersion ExecAndGetFirstLine( \
    "powershell", \
    "-NoProfile -ExecutionPolicy Bypass -Command ""(((Get-Content -Path '" + CargoTomlPath + "' | Where-Object { $_ -like 'version = *' } | Select-Object -First 1) -replace 'version = ', '').Trim().Trim('" + '""' + "'))""", \
    SourcePath \
  )
  #if AppVersion == ""
    #error AppVersion is not set and version was not found in Cargo.toml.
  #endif
#endif
#ifndef SourceDir
  #define SourceDir "..\dist\stage"
#endif

[Setup]
AppId={{D0C9E88A-3764-4C32-A2F7-0D4E3A0B9C22}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=WETQV
DefaultDirName={localappdata}\Zapret Hub
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
Compression=lzma
SolidCompression=yes
WizardStyle=modern dynamic windows11 hidebevels excludelightcontrols
WizardSizePercent=110
WizardBackColor=#111827
WizardBackColorDynamicDark=#0b1020
WizardBackImageFile=..\src\screen\menu_hub.png
WizardBackImageFileDynamicDark=..\src\screen\menu_hub.png
WizardBackImageOpacity=130
WizardImageFile=
WizardSmallImageFile=
OutputDir=..\dist\installer
OutputBaseFilename=zapret-hub-setup-{#AppVersion}
UninstallDisplayIcon={app}\Zapret Hub.exe
SetupIconFile=..\assets\icons\app.ico
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional shortcuts:"

[Files]
Source: "{#SourceDir}\Zapret Hub.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\builtin-whitelist.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\bundle\*"; DestDir: "{app}\bundle"; Flags: ignoreversion recursesubdirs createallsubdirs

[InstallDelete]
Type: filesandordirs; Name: "{app}\bundle"

[Icons]
Name: "{autodesktop}\Zapret Hub"; Filename: "{app}\Zapret Hub.exe"; Tasks: desktopicon
Name: "{group}\Zapret Hub"; Filename: "{app}\Zapret Hub.exe"
Name: "{group}\Uninstall Zapret Hub"; Filename: "{uninstallexe}"

[Run]
Filename: "{app}\Zapret Hub.exe"; Description: "Launch Zapret Hub"; Flags: nowait postinstall skipifsilent shellexec; Verb: "runas"

[UninstallDelete]
Type: filesandordirs; Name: "{app}\bundle"

[Code]
function ExecQuiet(const Params: string): Boolean;
var
  ResultCode: Integer;
begin
  Result := Exec(
    ExpandConstant('{cmd}'),
    '/C ' + Params,
    '',
    SW_HIDE,
    ewWaitUntilTerminated,
    ResultCode
  );
end;

procedure StopZapretRuntime();
begin
  ExecQuiet('taskkill /IM "Zapret Hub.exe" /T /F');
  ExecQuiet('taskkill /FI "WINDOWTITLE eq zapret:*" /T /F');
  ExecQuiet('taskkill /IM winws.exe /T /F');
  ExecQuiet('taskkill /IM TgWsProxy_windows.exe /T /F');
  ExecQuiet('net stop zapret');
  ExecQuiet('sc stop WinDivert');
  ExecQuiet('sc stop WinDivert14');
  ExecQuiet('sc delete zapret');
  ExecQuiet('sc delete WinDivert');
  ExecQuiet('sc delete WinDivert14');
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssInstall then
    StopZapretRuntime();
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    StopZapretRuntime();
end;
