; ScreenLink installer (Inno Setup 6).
; Build the release binary first:  cargo build --release -p screenlink-app
; Then compile this script:        iscc installer\screenlink.iss
;
; Adds inbound Windows Firewall rules for the control (TCP) and realtime (UDP)
; ports on Private/Domain profiles so peers can reach this device. Code-sign the
; produced installer with an OV/EV cert to avoid SmartScreen (see README §8).

#define AppName "ScreenLink"
; AppVersion can be overridden from the build: iscc /DAppVersion=1.2.3 ...
#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#define AppPublisher "ScreenLink contributors"
#define AppExe "screenlink.exe"
#define ControlPort "47820"
#define RealtimePort "47821"

[Setup]
AppId={{8B2F4E2A-7C3D-4E1A-9B6F-2C5A1D9E3F00}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
UninstallDisplayIcon={app}\{#AppExe}
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Firewall changes need admin.
PrivilegesRequired=admin
OutputDir=Output
OutputBaseFilename=ScreenLink-Setup-{#AppVersion}
WizardStyle=modern

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "autostart"; Description: "Start ScreenLink when I sign in"; GroupDescription: "Startup:"; Flags: unchecked

[Files]
; Expects the release binary at ..\target\release\screenlink.exe
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{group}\Uninstall {#AppName}"; Filename: "{uninstallexe}"
Name: "{userstartup}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: autostart

[Run]
; Inbound firewall rules (Private + Domain profiles; not Public, by design).
Filename: "{sys}\netsh.exe"; \
  Parameters: "advfirewall firewall add rule name=""ScreenLink Control (TCP)"" dir=in action=allow protocol=TCP localport={#ControlPort} profile=private,domain program=""{app}\{#AppExe}"""; \
  Flags: runhidden; StatusMsg: "Adding firewall rule (control)..."
Filename: "{sys}\netsh.exe"; \
  Parameters: "advfirewall firewall add rule name=""ScreenLink Realtime (UDP)"" dir=in action=allow protocol=UDP localport={#RealtimePort} profile=private,domain program=""{app}\{#AppExe}"""; \
  Flags: runhidden; StatusMsg: "Adding firewall rule (realtime)..."
; Offer to launch at the end.
Filename: "{app}\{#AppExe}"; Description: "Launch ScreenLink"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""ScreenLink Control (TCP)"""; Flags: runhidden
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""ScreenLink Realtime (UDP)"""; Flags: runhidden
