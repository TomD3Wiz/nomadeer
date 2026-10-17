; Inno Setup 6 script for the Nomadeer Windows installer.
;
;   iscc /DAppVersion=0.1.0 ^
;        /DOptimizedExe=target\optimized\release\nomadeer.exe ^
;        /DCompatibleExe=target\compatible\release\nomadeer.exe ^
;        packaging\windows\nomadeer.iss
;
; OptimizedExe is the x86-64-v3 (AVX2/FMA) build and CompatibleExe the
; baseline x86-64 build. Both are bundled and Setup installs the one this
; processor can run as Nomadeer.exe, asking the compatible build to check the
; processor. Relative paths are taken from this script's folder, and the
; defaults below match `make release` on Windows.
;
; Setup also accepts /EDITION=optimized or /EDITION=compatible to override
; the processor check.
;
; Nomadeer's Update button runs Setup as
;   /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS /SP-
;   /CURRENTUSER or /ALLUSERS  /EDITION=<the running edition>
;   /DIR=<the running install> /LOG=<temp>\nomadeer-updates\setup.log /RELAUNCH
; after checking it against its .sha256 (see src/self_update.rs). /RELAUNCH
; starts Nomadeer again once a silent install is done. Passing the running
; edition keeps a compatible edition the player chose on a processor that
; could run the optimized one; otherwise it is what the check would pick.

#if Ver < EncodeVer(6, 3, 0)
  #error Inno Setup 6.3 or newer is required
#endif
#ifndef AppVersion
  #error Pass the version with /DAppVersion=x.y.z
#endif
#ifndef OptimizedExe
  #define OptimizedExe "..\..\target\optimized\release\nomadeer.exe"
#endif
#ifndef CompatibleExe
  #define CompatibleExe "..\..\target\compatible\release\nomadeer.exe"
#endif

[Setup]
; Never change AppId: Windows uses it to recognise upgrades and uninstalls.
AppId={{AD0924E7-1EB6-4693-84CD-96670577BFB9}
AppName=Nomadeer
AppVersion={#AppVersion}
AppVerName=Nomadeer {#AppVersion}
AppPublisher=TomD3Wiz
AppPublisherURL=https://github.com/TomD3Wiz/nomadeer
AppSupportURL=https://github.com/TomD3Wiz/nomadeer/issues
AppUpdatesURL=https://github.com/TomD3Wiz/nomadeer/releases
AppCopyright=Copyright (c) 2026 TomD3Wiz
VersionInfoProductName=Nomadeer
VersionInfoDescription=Nomadeer Setup
DefaultDirName={autopf}\Nomadeer
DefaultGroupName=Nomadeer
DisableProgramGroupPage=yes
; Installs for the current user without admin rights unless the player picks
; "all users" in the dialog.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
WizardStyle=modern
SetupIconFile=..\..\assets\icons\nomadeer.ico
#if Ver >= EncodeVer(6, 5, 2)
WizardSmallImageFile=..\..\assets\icons\nomadeer-64.png,..\..\assets\icons\nomadeer-128.png
#endif
UninstallDisplayIcon={app}\nomadeer.ico
UninstallDisplayName=Nomadeer
LicenseFile=..\..\LICENSE
CloseApplications=yes
RestartApplications=no
OutputDir=..\..\dist\windows
OutputBaseFilename=Nomadeer-{#AppVersion}-Setup
Compression=lzma2/max
SolidCompression=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
english.EditionOptimized=Optimized edition, for processors with AVX2
english.EditionCompatible=Compatible edition, for processors without AVX2
english.EditionInstalled=Installed the %1.
english.RemoveSavedData=Also remove your saved Nomadeer data?%n%nThis permanently deletes your saved characters, chains and Umbra (about 1.1 GB, if you installed it) from:%n%n%1%n%nChoose No to keep them for the next time you install Nomadeer.
english.SavedDataKeptAllUsers=Saved Nomadeer data was kept. Each Windows user's characters, chains and Umbra (about 1.1 GB, if installed) stay in their own folder:%n%n%1%n%nDelete that folder while signed in as that user to remove them.
english.RemoveSavedDataFailed=Some saved data could not be removed. You can delete this folder yourself:%n%n%1

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
; Both editions are bundled; only the one this processor can run is installed.
Source: "{#OptimizedExe}"; DestDir: "{app}"; DestName: "Nomadeer.exe"; Check: UseOptimizedEdition; Flags: ignoreversion
Source: "{#CompatibleExe}"; DestDir: "{app}"; DestName: "Nomadeer.exe"; Check: not UseOptimizedEdition; Flags: ignoreversion
; Run from {tmp} by InitializeSetup to ask the processor which edition fits.
Source: "{#CompatibleExe}"; DestName: "nomadeer-cpu-check.exe"; Flags: dontcopy
Source: "..\..\assets\icons\nomadeer.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Nomadeer"; Filename: "{app}\Nomadeer.exe"; WorkingDir: "{app}"; IconFilename: "{app}\nomadeer.ico"; Comment: "Wormhole companion for EVE Online"
Name: "{autodesktop}\Nomadeer"; Filename: "{app}\Nomadeer.exe"; WorkingDir: "{app}"; IconFilename: "{app}\nomadeer.ico"; Comment: "Wormhole companion for EVE Online"; Tasks: desktopicon

[Run]
Filename: "{app}\Nomadeer.exe"; Description: "{cm:LaunchProgram,Nomadeer}"; WorkingDir: "{app}"; Flags: nowait postinstall skipifsilent
; Updates from inside Nomadeer start it again once the new version is in.
Filename: "{app}\Nomadeer.exe"; WorkingDir: "{app}"; Flags: nowait runasoriginaluser; Check: RelaunchAsPlayer
; Nomadeer starts an all-users update through Windows' permission prompt, so
; Setup itself is elevated and has no unelevated user to fall back on.
; Explorer hands the start to the player's desktop, so Nomadeer does not run
; with administrator rights.
Filename: "{win}\explorer.exe"; Parameters: """{app}\Nomadeer.exe"""; Flags: nowait; Check: RelaunchThroughDesktop

[Code]
const
  PF_AVX2_INSTRUCTIONS_AVAILABLE = 40;

var
  OptimizedEdition: Boolean;

function IsProcessorFeaturePresent(ProcessorFeature: Cardinal): BOOL;
  external 'IsProcessorFeaturePresent@kernel32.dll stdcall';

// The optimized edition needs every x86-64-v3 feature (AVX2, FMA, BMI1/2,
// LZCNT, MOVBE, F16C), which Windows cannot report on its own. The compatible
// build checks them with CPUID and answers through its exit code: 0 when the
// optimized edition can run, 1 when it cannot. Windows' AVX2 flag is only the
// fallback for when that check cannot run.
function ProcessorSupportsOptimizedEdition(): Boolean;
var
  CheckExe: String;
  ResultCode: Integer;
begin
  try
    ExtractTemporaryFile('nomadeer-cpu-check.exe');
    CheckExe := ExpandConstant('{tmp}\nomadeer-cpu-check.exe');
    if Exec(CheckExe, '--supports-optimized-edition', '', SW_HIDE, ewWaitUntilTerminated, ResultCode)
      and ((ResultCode = 0) or (ResultCode = 1)) then
    begin
      Result := ResultCode = 0;
      if Result then
        Log('Processor supports x86-64-v3.')
      else
        Log('Processor does not support x86-64-v3.');
      Exit;
    end;
    Log('Processor check did not run (exit code ' + IntToStr(ResultCode) + ').');
  except
    Log('Processor check failed: ' + GetExceptionMessage);
  end;
  Result := IsProcessorFeaturePresent(PF_AVX2_INSTRUCTIONS_AVAILABLE);
  if Result then
    Log('Windows reports AVX2.')
  else
    Log('Windows does not report AVX2.');
end;

function InitializeSetup(): Boolean;
var
  Edition: String;
begin
  Edition := Lowercase(ExpandConstant('{param:edition|auto}'));
  if Edition = 'optimized' then
    OptimizedEdition := True
  else if Edition = 'compatible' then
    OptimizedEdition := False
  else
    OptimizedEdition := ProcessorSupportsOptimizedEdition();

  if OptimizedEdition then
    Log('Installing the optimized edition (/EDITION=' + Edition + ').')
  else
    Log('Installing the compatible edition (/EDITION=' + Edition + ').');
  Result := True;
end;

function UseOptimizedEdition(): Boolean;
begin
  Result := OptimizedEdition;
end;

// /RELAUNCH comes from Nomadeer's Update button and only counts when silent;
// a normal install offers to launch Nomadeer on its last page instead.
function RelaunchRequested(): Boolean;
var
  I: Integer;
begin
  Result := False;
  if not WizardSilent() then
    Exit;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), '/RELAUNCH') = 0 then
    begin
      Result := True;
      Exit;
    end;
end;

function RelaunchAsPlayer(): Boolean;
begin
  Result := RelaunchRequested() and not IsAdminInstallMode();
end;

function RelaunchThroughDesktop(): Boolean;
begin
  Result := RelaunchRequested() and IsAdminInstallMode();
end;

function EditionDescription(): String;
begin
  if OptimizedEdition then
    Result := CustomMessage('EditionOptimized')
  else
    Result := CustomMessage('EditionCompatible');
end;

procedure CurPageChanged(CurPageID: Integer);
var
  Caption, Edition, ClickFinish: String;
  Grown: Integer;
begin
  if CurPageID <> wpFinished then
    Exit;
  // Say which edition went in, just above "Click Finish to exit Setup."
  Caption := WizardForm.FinishedLabel.Caption;
  Edition := FmtMessage(CustomMessage('EditionInstalled'), [EditionDescription()]);
  ClickFinish := SetupMessage(msgClickFinish);
  if Pos(ClickFinish, Caption) > 0 then
    StringChangeEx(Caption, ClickFinish, Edition + #13#10#13#10 + ClickFinish, True)
  else
    Caption := Caption + #13#10#13#10 + Edition;
  WizardForm.FinishedLabel.Caption := Caption;
  // Keep the launch checkbox below the taller label.
  Grown := WizardForm.AdjustLabelHeight(WizardForm.FinishedLabel) + ScaleY(8);
  WizardForm.RunList.Top := WizardForm.RunList.Top + Grown;
  WizardForm.RunList.Height := WizardForm.RunList.Height - Grown;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  DataDir: String;
begin
  if CurUninstallStep <> usPostUninstall then
    Exit;
  // Saved characters, chains and Umbra live outside {app} and are kept unless
  // the player says otherwise. Silent uninstalls always keep them.
  if IsAdminInstallMode() then
  begin
    // An all-users uninstall runs elevated, possibly as another account whose
    // {localappdata} is not the player's, so it never offers to delete data.
    Log('All-users uninstall: keeping every user''s saved data.');
    if not UninstallSilent() then
      MsgBox(FmtMessage(CustomMessage('SavedDataKeptAllUsers'), ['%LOCALAPPDATA%\Nomadeer']),
        mbInformation, MB_OK);
    Exit;
  end;
  DataDir := ExpandConstant('{localappdata}\Nomadeer');
  if not DirExists(DataDir) then
    Exit;
  if UninstallSilent() then
  begin
    Log('Keeping saved data in ' + DataDir);
    Exit;
  end;
  if MsgBox(FmtMessage(CustomMessage('RemoveSavedData'), [DataDir]),
    mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
  begin
    if DelTree(DataDir, True, True, True) then
      Log('Removed saved data in ' + DataDir)
    else
      MsgBox(FmtMessage(CustomMessage('RemoveSavedDataFailed'), [DataDir]), mbError, MB_OK);
  end
  else
    Log('Keeping saved data in ' + DataDir);
end;
