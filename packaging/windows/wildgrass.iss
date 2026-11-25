#ifndef MyAppVersion
  #define MyAppVersion "1.0.0"
#endif

#ifndef MyBinaryPath
  #define MyBinaryPath "..\..\target\release\wildgrass.exe"
#endif

[Setup]
AppId={{9347DDE1-9564-47D8-B56E-1E3D2FAAD55A}
AppName=wildgrass
AppVersion={#MyAppVersion}
AppVerName=wildgrass {#MyAppVersion}
AppPublisher=wocstombs
AppPublisherURL=https://github.com/wocstombs/wildgrass
AppSupportURL=https://github.com/wocstombs/wildgrass/issues
AppUpdatesURL=https://github.com/wocstombs/wildgrass/releases
DefaultDirName={autopf}\wildgrass
DisableProgramGroupPage=yes
LicenseFile=..\..\LICENSE
PrivilegesRequired=lowest
MinVersion=10.0.17134
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\target\installer
OutputBaseFilename=wildgrass-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ChangesEnvironment=yes
UninstallDisplayName=wildgrass
UninstallDisplayIcon={app}\wildgrass.exe

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "deps"; Description: "Install missing dependencies (mpv, yt-dlp, ffmpeg) via winget"; GroupDescription: "Dependencies:"

[Files]
Source: "{#MyBinaryPath}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\fonts\UBUNTU-FONT-LICENCE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\windows-terminal\wildgrass.json"; DestDir: "{localappdata}\Microsoft\Windows Terminal\Fragments\wildgrass"; Flags: ignoreversion
Source: "..\fonts\UbuntuMono[wght].ttf"; DestDir: "{autofonts}"; FontInstall: "Ubuntu Mono"; Flags: onlyifdoesntexist uninsneveruninstall
Source: "..\fonts\UbuntuMono-Italic[wght].ttf"; DestDir: "{autofonts}"; FontInstall: "Ubuntu Mono Italic"; Flags: onlyifdoesntexist uninsneveruninstall
Source: "install_deps.ps1"; DestDir: "{tmp}"; Flags: deleteafterinstall; Tasks: deps; Check: not WizardSilent

[Run]
Filename: "powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{tmp}\install_deps.ps1"""; StatusMsg: "Checking dependencies (mpv, yt-dlp, ffmpeg)..."; Tasks: deps; Check: not WizardSilent; Flags: waituntilterminated

[Code]
const
  UserEnvironmentKey = 'Environment';

procedure AddUserPath(Dir: string);
var
  Paths: string;
  Normalized: string;
begin
  Normalized := RemoveBackslashUnlessRoot(Dir);
  if not RegQueryStringValue(HKEY_CURRENT_USER, UserEnvironmentKey, 'Path', Paths) then
    Paths := '';

  if (Pos(';' + Uppercase(Normalized) + ';', ';' + Uppercase(Paths) + ';') > 0) or
     (Pos(';' + Uppercase(Normalized) + '\;', ';' + Uppercase(Paths) + ';') > 0) then
    Exit;

  if (Paths <> '') and (Paths[Length(Paths)] <> ';') then
    Paths := Paths + ';';
  Paths := Paths + Normalized;

  if not RegWriteExpandStringValue(HKEY_CURRENT_USER, UserEnvironmentKey, 'Path', Paths) then
    Log('Failed to add ' + Normalized + ' to user Path.');
end;

function RemovePathEntry(var Paths: string; Entry: string): Boolean;
var
  P: Integer;
  Removed: Boolean;
begin
  Removed := False;
  P := Pos(';' + Uppercase(Entry) + ';', ';' + Uppercase(Paths) + ';');
  while P > 0 do
  begin
    if P > 1 then
      Delete(Paths, P - 1, Length(Entry) + 1)
    else
      Delete(Paths, 1, Length(Entry) + 1);
    Removed := True;
    P := Pos(';' + Uppercase(Entry) + ';', ';' + Uppercase(Paths) + ';');
  end;
  Result := Removed;
end;

procedure RemoveUserPath(Dir: string);
var
  Paths: string;
  Normalized: string;
  Changed: Boolean;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, UserEnvironmentKey, 'Path', Paths) then
    Exit;

  Normalized := RemoveBackslashUnlessRoot(Dir);
  Changed := RemovePathEntry(Paths, Normalized);
  if RemovePathEntry(Paths, Normalized + '\') then
    Changed := True;

  if Changed then
  begin
    if not RegWriteExpandStringValue(HKEY_CURRENT_USER, UserEnvironmentKey, 'Path', Paths) then
      Log('Failed to remove ' + Normalized + ' from user Path.');
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    AddUserPath(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    RemoveUserPath(ExpandConstant('{app}'));
end;
