; ITSaNAS-Setup.exe: the clickable Windows installer (HANDOVER §8 0w (6b), 1).
;
; Built by .github/workflows/release.yml from the release's own
; itsanas-x86_64-pc-windows-msvc.exe, byte for byte, so the installed program
; is the binary the signed manifest lists and updates itself like one
; installed by install/get.ps1. Inno Setup: free, no advertising, no service
; behind it. Not code-signed (no spending, Nicolas 2026-10-07), so Windows
; SmartScreen says "unknown publisher" the first time.
;
; It installs for this account only, where get.ps1 does, puts it on the
; PATH, and starts `itsanas setup`, which opens the setup page.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef Binary
  #define Binary "itsanas.exe"
#endif

[Setup]
AppId={{7B0E8B54-6C1E-4C47-9C7A-2F3E1D0A4B11}
AppName=ITSaNAS
AppVersion={#AppVersion}
AppPublisher=ITSaNAS
AppPublisherURL=https://github.com/SigSegGit/itsanas
DefaultDirName={localappdata}\Programs\itsanas\bin
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputBaseFilename=ITSaNAS-Setup-{#AppVersion}
SetupIconFile=..\..\docs\assets\itsanas.ico
UninstallDisplayIcon={app}\itsanas.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ChangesEnvironment=yes
CloseApplications=no

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "fr"; MessagesFile: "compiler:Languages\French.isl"

[Files]
Source: "{#Binary}"; DestDir: "{app}"; DestName: "itsanas.exe"; Flags: ignoreversion
Source: "..\..\docs\assets\itsanas.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\ITSaNAS settings"; Filename: "{app}\itsanas.exe"; Parameters: "settings"; IconFilename: "{app}\itsanas.ico"

[Registry]
; On the PATH, as get.ps1 does, so `itsanas` works in a new terminal.
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Check: NeedsAddPath(ExpandConstant('{app}'))

[Run]
Filename: "{app}\itsanas.exe"; Parameters: "setup"; Description: "Set up ITSaNAS now"; Flags: postinstall nowait skipifsilent

[UninstallRun]
; Stops the daemon and forgets the passphrase; the account, the files and
; the data hosted for others stay (clean removal is 0w (7)).
Filename: "{app}\itsanas.exe"; Parameters: "signout"; Flags: runhidden; RunOnceId: "signout"

[Code]
function NeedsAddPath(Dir: string): Boolean;
var
  Paths: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Paths) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Paths) + ';') = 0;
end;
