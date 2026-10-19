#ifndef AppVersion
  #error AppVersion must be passed by scripts/package.ps1
#endif
[Setup]
AppId={{C5C3A601-CFC1-4597-A7CA-D77735208B6D}
AppName=DoNothing&Win Launcher
AppVersion={#AppVersion}
VersionInfoVersion={#AppVersion}
AppPublisher=Revolutionart
AppPublisherURL=https://github.com/revolutionart/DoNothingAndWin
AppSupportURL=https://github.com/revolutionart/DoNothingAndWin/issues
AppUpdatesURL=https://github.com/revolutionart/DoNothingAndWin/releases
DefaultDirName={localappdata}\Programs\DoNothingAndWin
DefaultGroupName=DoNothing&Win Launcher
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=..\dist
OutputBaseFilename=DoNothingAndWin-Setup-{#AppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
DisableProgramGroupPage=yes
LicenseFile=..\LICENSE
InfoBeforeFile=welcome.txt
UninstallDisplayIcon={app}\DoNothingAndWinLauncher.exe
CloseApplications=no
RestartApplications=no
SetupLogging=yes

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked
Name: "startup"; Description: "Start with Windows"; GroupDescription: "Startup:"; Flags: unchecked
Name: "startup\minimized"; Description: "Start in the system tray (no taskbar button)"

[Files]
Source: "..\dist\app\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\DoNothing&Win Launcher"; Filename: "{app}\DoNothingAndWinLauncher.exe"; Check: not IsLauncherUpdate
Name: "{autodesktop}\DoNothing&Win Launcher"; Filename: "{app}\DoNothingAndWinLauncher.exe"; Tasks: desktopicon; Check: not IsLauncherUpdate

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "EngineWorkspace"; ValueData: "{code:StartupCommand}"; Tasks: startup; Flags: uninsdeletevalue; Check: not IsLauncherUpdate
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "EngineWorkspace"; Tasks: not startup; Flags: deletevalue; Check: not IsLauncherUpdate

[Run]
; Hand off to the desktop before starting the app so installer process restrictions
; cannot propagate to Unreal Editor and block users' redirected cache folders.
Filename: "{app}\DoNothingAndWinLauncher.exe"; Parameters: "--launch-from-desktop"; Description: "Launch DoNothing&&Win Launcher"; Flags: nowait postinstall skipifsilent; Check: not IsLauncherUpdate
Filename: "{app}\DoNothingAndWinLauncher.exe"; Parameters: "--launch-from-desktop"; Flags: nowait; Check: ShouldRestartUpdate

[Code]
function IsLauncherUpdate(): Boolean;
begin
  Result := ExpandConstant('{param:DNWUPDATE|0}') = '1';
end;

function ShouldRestartUpdate(): Boolean;
begin
  Result := IsLauncherUpdate and (ExpandConstant('{param:DNWSUPERVISED|0}') <> '1');
end;

function StartupCommand(Param: String): String;
begin
  Result := '"' + ExpandConstant('{app}\DoNothingAndWinLauncher.exe') + '" --no-scan';
  if WizardIsTaskSelected('startup\minimized') then Result := Result + ' --minimized';
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var ExitCode, Attempts: Integer;
begin
  ExtractTemporaryFile('DoNothingAndWinLauncher.exe');
  Attempts := 0;
  repeat
    if not Exec(ExpandConstant('{tmp}\DoNothingAndWinLauncher.exe'), '--check-running', '', SW_HIDE, ewWaitUntilTerminated, ExitCode) then
    begin
      Result := 'Could not check whether the launcher is running. Setup has stopped safely.';
      Exit;
    end;
    if ExitCode = 0 then Exit;
    if (ExitCode <> 2) or (not IsLauncherUpdate) or (Attempts >= 120) then
    begin
      Result := 'Finish any downloads or installations, then exit DoNothing&Win Launcher and retry. Setup will not interrupt active work.';
      Exit;
    end;
    { Let the updater's parent exit normally; never terminate a running launcher. }
    Sleep(250);
    Attempts := Attempts + 1;
  until False;
end;

function InitializeUninstall(): Boolean;
var ExitCode: Integer;
begin
  Result := Exec(ExpandConstant('{app}\DoNothingAndWinLauncher.exe'), '--check-running', '', SW_HIDE, ewWaitUntilTerminated, ExitCode);
  Result := Result and (ExitCode = 0);
  if not Result then MsgBox('Finish active downloads and exit the launcher before uninstalling. Your projects and engines will be kept.', mbInformation, MB_OK);
end;
