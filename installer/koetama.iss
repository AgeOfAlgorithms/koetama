; Koetama's Windows installer (Inno Setup 6). build.py runs it:
;   ISCC /DAppVersion=0.1.0 /DAppName=Koetama /DSourceDir=<dist\Koetama> /DOutDir=<dist> installer\koetama.iss
; Per user (no admin rights): installs to %LOCALAPPDATA%\Programs\Koetama, so the in-app updater can run it silently
; (/VERYSILENT /RELAUNCH=1): it closes a running Koetama, replaces the files and starts the new version. A silent install
; without /RELAUNCH=1 (a package manager, the CI) starts nothing.
; The speech models are not in it: Koetama downloads them per language into %LOCALAPPDATA%\Koetama\models.

#ifndef AppName
  #define AppName "Koetama"
#endif
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{DE2BF700-F731-4A9C-93BB-6ED60D3D04BB}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=AgeOfAlgorithms
AppPublisherURL=https://github.com/AgeOfAlgorithms/koetama
AppSupportURL=https://github.com/AgeOfAlgorithms/koetama/issues
AppUpdatesURL=https://github.com/AgeOfAlgorithms/koetama/releases
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
; (the app's Ember look: dark, the window's graphite, its own title bar; the pictures from app/assets/
;  make_installer_art.py, one per screen scale)
WizardStyle=modern dark includetitlebar hidebevels
WizardBackColor=#0b0909
WizardImageBackColor=#0b0909
#ifdef ArtDir
WizardImageFile={#ArtDir}\wizard-100.bmp,{#ArtDir}\wizard-125.bmp,{#ArtDir}\wizard-150.bmp,{#ArtDir}\wizard-200.bmp,{#ArtDir}\wizard-250.bmp
WizardSmallImageFile={#ArtDir}\small-100.bmp,{#ArtDir}\small-125.bmp,{#ArtDir}\small-150.bmp,{#ArtDir}\small-200.bmp,{#ArtDir}\small-250.bmp
#endif
CloseApplications=yes
RestartApplications=no
; (no license page: MIT needs no click-through - LICENSE and THIRD_PARTY_NOTICES.txt ship in the install folder)
DisableWelcomePage=no
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
; an install by hand: a "Launch Koetama" box at the end; an update (silent, /RELAUNCH=1): start the new version at once
Filename: "{app}\{#AppName}.exe"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
Filename: "{app}\{#AppName}.exe"; Flags: nowait; Check: Relaunch

[Code]
function Relaunch: Boolean;
begin
  Result := WizardSilent and (ExpandConstant('{param:RELAUNCH|0}') = '1');
end;

{ ---- Uninstalling: the downloaded speech models and the settings (%LOCALAPPDATA%\Koetama, not the install folder)
  go too unless the player says No - someone who uninstalls has no use for gigabytes of models (the user, 2026-10-08).
  The question defaults to Yes; a silent uninstall (/SUPPRESSMSGBOXES) takes Yes. Updates install over the old copy
  and never run this. }
var
  RemoveData: Boolean;

function DataDir: String;
begin
  Result := ExpandConstant('{localappdata}\{#AppName}');
end;

{ the bytes in a folder and everything under it }
function FolderBytes(const Dir: String): Int64;
var
  R: TFindRec;
begin
  Result := 0;
  if FindFirst(Dir + '\*', R) then
  begin
    try
      repeat
        if (R.Name <> '.') and (R.Name <> '..') then
        begin
          if (R.Attributes and FILE_ATTRIBUTE_DIRECTORY) <> 0 then
            Result := Result + FolderBytes(Dir + '\' + R.Name)
          else
            Result := Result + Int64(R.SizeHigh) * 65536 * 65536 + R.SizeLow;
        end;
      until not FindNext(R);
    finally
      FindClose(R);
    end;
  end;
end;

function SizeText(Bytes: Int64): String;
begin
  if Bytes >= 1073741824 then
    Result := Format('%.1f GB', [Bytes / 1073741824.0])
  else
    Result := Format('%d MB', [Bytes div 1048576]);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
  begin
    RemoveData := False;
    if DirExists(DataDir) then
      RemoveData := SuppressibleMsgBox('Also remove the downloaded speech models and your Koetama settings (' +
        SizeText(FolderBytes(DataDir)) + ')?' + #13#10#13#10 +
        'Choose No to keep them for a later reinstall.',
        mbConfirmation, MB_YESNO or MB_DEFBUTTON1, IDYES) = IDYES;
  end
  else if (CurUninstallStep = usPostUninstall) and RemoveData then
    DelTree(DataDir, True, True, True);
end;

[Messages]
WelcomeLabel1=Welcome to [name]
WelcomeLabel2=Koetama plays the other players' voices where they stand in the game, and writes what you say as you say it.%n%nIt installs for you only (no administrator rights). The speech model for your language downloads the first time you speak.
FinishedHeadingLabel=[name] is ready
FinishedLabel=Start a game with a mod that works with Koetama (for example Teardown with Proximity Babble Chat): Koetama connects by itself.
