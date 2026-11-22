; Compiled by scripts/package-windows.ps1 with the same payload as the ZIP.
[Setup]
AppId=dev.velocity.Velocity
AppName=Velocity
AppVersion={#Version}
AppPublisher=Velocity Contributors
AppPublisherURL=https://github.com/mini-zoro/velocity
VersionInfoVersion={#NumericVersion}
VersionInfoProductVersion={#NumericVersion}
VersionInfoProductTextVersion={#Version}
DefaultDirName={localappdata}\Programs\Velocity
DefaultGroupName=Velocity
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\velocity.exe
SetupIconFile=velocity.ico
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
Compression=lzma2
SolidCompression=yes
ChangesAssociations=yes
ChangesEnvironment=yes
WizardStyle=modern

[Tasks]
Name: "addpath"; Description: "Add Velocity's console command (vel.exe) to my PATH"; Flags: unchecked

[Files]
Source: "{#Payload}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Velocity"; Filename: "{app}\velocity.exe"; WorkingDir: "{app}"

[Registry]
; An alternate Open with handler, never the default or UserChoice.
Root: HKCU; Subkey: "Software\Classes\Velocity.TextFile"; ValueType: string; ValueData: "Velocity"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Velocity.TextFile\DefaultIcon"; ValueType: string; ValueData: "{app}\velocity.exe,0"
Root: HKCU; Subkey: "Software\Classes\Velocity.TextFile\shell\open\command"; ValueType: string; ValueData: """{app}\velocity.exe"" ""%1"""
Root: HKCU; Subkey: "Software\Classes\Applications\velocity.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "Velocity"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Applications\velocity.exe\shell\open\command"; ValueType: string; ValueData: """{app}\velocity.exe"" ""%1"""
Root: HKCU; Subkey: "Software\Classes\.txt\OpenWithProgids"; ValueType: string; ValueName: "Velocity.TextFile"; Flags: uninsdeletevalue uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Classes\.text\OpenWithProgids"; ValueType: string; ValueName: "Velocity.TextFile"; Flags: uninsdeletevalue uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Classes\.log\OpenWithProgids"; ValueType: string; ValueName: "Velocity.TextFile"; Flags: uninsdeletevalue uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Classes\.md\OpenWithProgids"; ValueType: string; ValueName: "Velocity.TextFile"; Flags: uninsdeletevalue uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Classes\.json\OpenWithProgids"; ValueType: string; ValueName: "Velocity.TextFile"; Flags: uninsdeletevalue uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Classes\.toml\OpenWithProgids"; ValueType: string; ValueName: "Velocity.TextFile"; Flags: uninsdeletevalue uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Classes\.yaml\OpenWithProgids"; ValueType: string; ValueName: "Velocity.TextFile"; Flags: uninsdeletevalue uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Classes\.yml\OpenWithProgids"; ValueType: string; ValueName: "Velocity.TextFile"; Flags: uninsdeletevalue uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Classes\.ini\OpenWithProgids"; ValueType: string; ValueName: "Velocity.TextFile"; Flags: uninsdeletevalue uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Classes\.csv\OpenWithProgids"; ValueType: string; ValueName: "Velocity.TextFile"; Flags: uninsdeletevalue uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Classes\.xml\OpenWithProgids"; ValueType: string; ValueName: "Velocity.TextFile"; Flags: uninsdeletevalue uninsdeletekeyifempty

[Code]
const
  PathOwnerKey = 'Software\Velocity\Installer';

function IsPathEntry(const Item, Dir: string): Boolean;
begin
  Result := CompareText(RemoveBackslashUnlessRoot(Trim(Item)), Dir) = 0;
end;

function PathContains(const Path, Dir: string): Boolean;
var
  I, Start: Integer;
begin
  Result := True;
  Start := 1;
  for I := 1 to Length(Path) do
    if Path[I] = ';' then begin
      if IsPathEntry(Copy(Path, Start, I - Start), Dir) then exit;
      Start := I + 1;
    end;
  Result := IsPathEntry(Copy(Path, Start, Length(Path)), Dir);
end;

procedure RemoveManagedPath();
var
  Path, Dir: string;
  I, Start, Finish: Integer;
  Delimiter: Boolean;
begin
  if not RegQueryStringValue(HKCU, PathOwnerKey, 'AddedPath', Dir) then exit;
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', Path) then Path := '';
  // Remove just one exact entry we added, in reverse order. Preserve other entries.
  Finish := Length(Path) + 1;
  for I := Length(Path) downto 0 do begin
    if I = 0 then Delimiter := True else Delimiter := Path[I] = ';';
    if Delimiter then begin
      Start := I + 1;
      if Copy(Path, Start, Finish - Start) = Dir then begin
        if I > 0 then
          Delete(Path, I, Finish - I)
        else
          Delete(Path, 1, Finish);
        if not RegWriteExpandStringValue(HKCU, 'Environment', 'Path', Path) then exit;
        break;
      end;
      Finish := I;
    end;
  end;
  RegDeleteValue(HKCU, PathOwnerKey, 'AddedPath');
  RegDeleteKeyIfEmpty(HKCU, PathOwnerKey);
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Dir, Previous, Path: string;
begin
  if CurStep <> ssPostInstall then exit;
  Dir := ExpandConstant('{app}');
  if not WizardIsTaskSelected('addpath') then begin
    RemoveManagedPath();
    exit;
  end;
  if RegQueryStringValue(HKCU, PathOwnerKey, 'AddedPath', Previous) and (Previous <> Dir) then
    RemoveManagedPath();
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', Path) then Path := '';
  if PathContains(Path, Dir) then exit;
  if Path <> '' then Path := Path + ';';
  if not RegWriteStringValue(HKCU, PathOwnerKey, 'AddedPath', Dir) then
    Log('Could not record PATH ownership.')
  else if not RegWriteExpandStringValue(HKCU, 'Environment', 'Path', Path + Dir) then begin
    RegDeleteValue(HKCU, PathOwnerKey, 'AddedPath');
    Log('Could not append Velocity to the user PATH.');
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then RemoveManagedPath();
end;
