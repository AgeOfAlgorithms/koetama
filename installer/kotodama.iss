; Kotodama's Windows installer (Inno Setup 6). build.py runs it:
;   ISCC /DAppVersion=0.1.0 /DAppName=Kotodama /DSourceDir=<dist\Kotodama> /DOutDir=<dist> installer\kotodama.iss
; Per user (no admin rights): installs to %LOCALAPPDATA%\Programs\Kotodama, so the in-app updater can run it silently
; (/VERYSILENT /RELAUNCH=1): it closes a running Kotodama, replaces the files and starts the new version. A silent install
; without /RELAUNCH=1 (a package manager, the CI) starts nothing.
; The speech models are not in it: Kotodama downloads them per language into %LOCALAPPDATA%\Kotodama\models.

#ifndef AppName
  #define AppName "Kotodama"
#endif
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{6F4C2D7E-3B1A-4E8F-9C2D-5A7B8E1F0D3C}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=AgeOfAlgorithms
AppPublisherURL=https://github.com/AgeOfAlgorithms/proximity-voice-chat-STT-engine
AppSupportURL=https://github.com/AgeOfAlgorithms/proximity-voice-chat-STT-engine/issues
AppUpdatesURL=https://github.com/AgeOfAlgorithms/proximity-voice-chat-STT-engine/releases
DefaultDirName={localappdata}\Programs\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutDir}
OutputBaseFilename={#AppName}-Setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
LicenseFile={#SourceDir}\LICENSE
#ifdef IconFile
SetupIconFile={#IconFile}
#endif
UninstallDisplayIcon={app}\{#AppName}.exe

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppName}.exe"
Name: "{group}\Uninstall {#AppName}"; Filename: "{uninstallexe}"
Name: "{userdesktop}\{#AppName}"; Filename: "{app}\{#AppName}.exe"; Tasks: desktopicon

[Run]
; an install by hand: a "Launch Kotodama" box at the end; an update (silent, /RELAUNCH=1): start the new version at once
Filename: "{app}\{#AppName}.exe"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
Filename: "{app}\{#AppName}.exe"; Flags: nowait; Check: Relaunch

[Code]
function Relaunch: Boolean;
begin
  Result := WizardSilent and (ExpandConstant('{param:RELAUNCH|0}') = '1');
end;
