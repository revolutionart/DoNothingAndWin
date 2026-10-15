#ifndef AppVersion
  #error AppVersion must be passed by scripts/package.ps1
#endif
[Setup]
AppId={{C5C3A601-CFC1-4597-A7CA-D77735208B6D}
AppName=DoNothing&Win Launcher
AppVersion={#AppVersion}
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
Name: "startup\minimized"; Description: "Start minimized"

[Files]
Source: "..\dist\app\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\DoNothing&Win Launcher"; Filename: "{app}\DoNothingAndWinLauncher.exe"
Name: "{autodesktop}\DoNothing&Win Launcher"; Filename: "{app}\DoNothingAndWinLauncher.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "EngineWorkspace"; ValueData: "{code:StartupCommand}"; Tasks: startup; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "EngineWorkspace"; Tasks: not startup; Flags: deletevalue

[Run]
Filename: "{app}\DoNothingAndWinLauncher.exe"; Description: "Launch DoNothing&&Win Launcher"; Flags: nowait postinstall skipifsilent

[Code]
function StartupCommand(Param: String): String;
begin
  Result := '"' + ExpandConstant('{app}\DoNothingAndWinLauncher.exe') + '" --no-scan';
  if WizardIsTaskSelected('startup\minimized') then Result := Result + ' --minimized';
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var ExitCode: Integer;
begin
  ExtractTemporaryFile('DoNothingAndWinLauncher.exe');
  if not Exec(ExpandConstant('{tmp}\DoNothingAndWinLauncher.exe'), '--check-running', '', SW_HIDE, ewWaitUntilTerminated, ExitCode) then
    Result := 'Could not check whether the launcher is running. Setup has stopped safely.'
  else if ExitCode <> 0 then
    Result := 'Finish any downloads or installations, then exit DoNothing&Win Launcher and retry. Setup will not interrupt active work.';
end;

function InitializeUninstall(): Boolean;
var ExitCode: Integer;
begin
  Result := Exec(ExpandConstant('{app}\DoNothingAndWinLauncher.exe'), '--check-running', '', SW_HIDE, ewWaitUntilTerminated, ExitCode);
  Result := Result and (ExitCode = 0);
  if not Result then MsgBox('Finish active downloads and exit the launcher before uninstalling. Your projects and engines will be kept.', mbInformation, MB_OK);
end;
