; Installeur Inno Setup pour Auris (Windows x64).
; Compilé en CI (job installer-win de release.yml) :
;   iscc /DAppDir=<dir contenant auris.exe> packaging\windows\auris.iss
; La version est réécrite par packaging/windows/update-iss.py à chaque release.
#define MyAppName "Auris"
#define MyAppVersion "0.8.0"
#define MyAppPublisher "mjcc30"
#define MyAppURL "https://auris.audio"
#define MyAppExeName "auris.exe"
#ifndef AppDir
  #define AppDir "."
#endif

[Setup]
AppId={{7F3E9A21-5C4B-4E8D-9A6F-2B1C0D8E5F47}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL=https://github.com/mjcc30/auris/issues
AppUpdatesURL=https://github.com/mjcc30/auris/releases
DefaultDirName={autopf}\Auris
DefaultGroupName=Auris
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64Compatible
ArchitecturesInstallIn64BitMode=x64Compatible
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
PrivilegesRequired=admin
OutputBaseFilename=auris-setup-x64
UninstallDisplayIcon={app}\{#MyAppExeName}
UninstallDisplayName={#MyAppName}

[Languages]
Name: "french"; MessagesFile: "compiler:Languages\French.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked
Name: "autostart"; Description: "Démarrer Auris au login (interface web http://localhost:3000)"; Flags: checkedonce

[Files]
Source: "{#AppDir}\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Auris"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\Désinstaller Auris"; Filename: "{uninstallexe}"
Name: "{commondesktop}\Auris"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; \
  ValueName: "Auris"; ValueData: """{app}\{#MyAppExeName}"" serve --port 3000"; \
  Flags: uninsdeletevalue; Tasks: autostart

[Run]
Filename: "{app}\{#MyAppExeName}"; Parameters: "serve --port 3000"; \
  Description: "Lancer Auris maintenant"; Flags: nowait postinstall skipifsilent unchecked
