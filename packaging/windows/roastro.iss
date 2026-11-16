; The Windows installer ("Installing and running Roastro on the roastery
; laptop", #53). Built by CI with Inno Setup from the desktop release build:
;   iscc /DAppVersion=0.1.0 /DSourceExe=target\release\roastro-desktop.exe packaging\windows\roastro.iss
; Per-user, no administrator rights; Start menu and desktop shortcuts; not
; started with Windows. The roasts live in %LOCALAPPDATA%\Roastro, which the
; installer and the uninstaller never touch.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\..\target\release\roastro-desktop.exe"
#endif

[Setup]
AppId={{6F1D0B2E-5C7A-4E0B-9C61-7E2A3B8D4F10}
AppName=Roastro
AppVersion={#AppVersion}
AppPublisher=Roastro
DefaultDirName={localappdata}\Programs\Roastro
DefaultGroupName=Roastro
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputBaseFilename=roastro-setup-{#AppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayName=Roastro
CloseApplications=yes

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "Roastro.exe"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Roastro"; Filename: "{app}\Roastro.exe"
Name: "{autodesktop}\Roastro"; Filename: "{app}\Roastro.exe"

[Run]
Filename: "{app}\Roastro.exe"; Description: "Open Roastro"; Flags: nowait postinstall skipifsilent
