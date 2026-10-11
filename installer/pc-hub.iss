; Inno Setup script for PC Hub. The release workflow builds it with:
;   ISCC /DAppVersion=0.2.0 installer\pc-hub.iss
; after `cargo build --release`. The setup lands in dist\.
;
; It installs for the current user only (no admin prompt), into a folder the hub can write
; its settings, backups and log to. An existing hub.toml is kept on upgrade.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
; Never change AppId: Windows uses it to recognize upgrades of the same app.
AppId={{4BDBD1E3-2451-451C-8EBC-94C136C09ACB}
AppName=PC Hub
AppVersion={#AppVersion}
AppVerName=PC Hub {#AppVersion}
AppPublisher=Sudeep Mishra
AppPublisherURL=https://github.com/sudpmishra/PC-Hub
AppSupportURL=https://github.com/sudpmishra/PC-Hub/issues
AppUpdatesURL=https://github.com/sudpmishra/PC-Hub/releases
DefaultDirName={localappdata}\Programs\PC Hub
DefaultGroupName=PC Hub
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=pc-hub-setup-{#AppVersion}
SetupIconFile=..\ui\icon.ico
UninstallDisplayIcon={app}\pc-hub.exe
LicenseFile=..\LICENSE
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
; Close a running hub before replacing it, and start it again afterwards.
CloseApplications=yes
RestartApplications=yes

[Tasks]
Name: "startup"; Description: "Start PC Hub when I sign in"
Name: "desktopicon"; Description: "Add a desktop shortcut"; Flags: unchecked

[Files]
Source: "..\target\release\pc-hub.exe"; DestDir: "{app}"; Flags: ignoreversion
; The settings file is the user's to edit: only put the starter one in place once.
Source: "..\hub.toml"; DestDir: "{app}"; Flags: onlyifdoesntexist uninsneveruninstall
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{group}\PC Hub"; Filename: "{app}\pc-hub.exe"
Name: "{group}\Edit hub.toml"; Filename: "{sys}\notepad.exe"; Parameters: """{app}\hub.toml"""
Name: "{group}\Uninstall PC Hub"; Filename: "{uninstallexe}"
Name: "{autodesktop}\PC Hub"; Filename: "{app}\pc-hub.exe"; Tasks: desktopicon

[Registry]
; The same entry the hub's own Start with Windows switch writes, so the two agree.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "PC Hub"; ValueData: """{app}\pc-hub.exe"""; Tasks: startup
; Removed on uninstall however it was turned on.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "PC Hub"; Flags: uninsdeletevalue

[Run]
Filename: "{app}\pc-hub.exe"; Description: "Start PC Hub now"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; A running hub would keep its files locked.
Filename: "{sys}\taskkill.exe"; Parameters: "/IM pc-hub.exe /F"; Flags: runhidden; RunOnceId: "StopHub"
