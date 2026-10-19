; Inno Setup 6 script for the Nomadeer Windows installer.
;
;   python tools\release.py inno-defines --output target\umbra-defines.iss
;   iscc /DAppVersion=0.1.0 ^
;        /DOptimizedExe=target\optimized\release\nomadeer.exe ^
;        /DCompatibleExe=target\compatible\release\nomadeer.exe ^
;        /DUmbraDefines=%CD%\target\umbra-defines.iss ^
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
;
; The "umbra" task downloads Umbra's core during a per-user install, checks
; each file's SHA-256 and puts them where Nomadeer looks, with the same
; "verified" marker Nomadeer writes after its own check, so she is online on
; first launch. UmbraDefines names the file `tools/release.py inno-defines`
; writes from src/local_intel.rs and src/paths.rs: her URLs, SHA-256s, sizes
; and folders. Only a test build (/DUmbraTestDownloads) may point them
; anywhere but https. If her download fails or is cancelled, Setup still
; finishes and says she can be installed later from her panel. Updates from
; inside Nomadeer (/RELAUNCH) and all-users installs never download her.

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
#ifndef UmbraDefines
  #error Pass Umbra's download defines with /DUmbraDefines=<file from tools/release.py inno-defines>
#endif
#include UmbraDefines
#ifndef UmbraTestDownloads
  #if Copy(UmbraModelUrl, 1, 8) != "https://" || Copy(UmbraTokenizerUrl, 1, 8) != "https://"
    #error Umbra's downloads must be https outside test builds
  #endif
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
english.UmbraTask=Install Umbra, Nomadeer's intel companion (about {#UmbraDownloadSize} download)
english.UmbraDownloadCaption=Bringing Umbra aboard
english.UmbraDownloadDescription=Setup is downloading Umbra's core. She runs entirely on this computer.
english.UmbraNotDownloaded=Umbra could not be downloaded, so Nomadeer will be installed without her. You can install her later from her panel in Nomadeer.%n%nWhat went wrong: %1
english.UmbraNotPlaced=Umbra could not be set up, so Nomadeer was installed without her. You can install her later from her panel in Nomadeer.%n%nWhat went wrong: %1
english.UmbraCancelled=the download was cancelled
english.UmbraInstalled=Umbra is aboard and will be online when Nomadeer starts.
english.UmbraLater=Umbra was not installed. You can install her later from her panel in Nomadeer.

[Tasks]
; Umbra lives in each player's own %LOCALAPPDATA%, so only per-user installs
; offer her, and only while she is not already there.
Name: "umbra"; Description: "{cm:UmbraTask}"; Check: UmbraTaskOffered
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

  // FILETIME of 1970-01-01, in 100 ns intervals since 1601.
  UNIX_EPOCH_FILETIME = 116444736000000000;

type
  // WIN32_FILE_ATTRIBUTE_DATA, each FILETIME as its low and high halves.
  TUmbraFileData = record
    Attributes: DWORD;
    CreationTimeLow, CreationTimeHigh: DWORD;
    LastAccessTimeLow, LastAccessTimeHigh: DWORD;
    LastWriteTimeLow, LastWriteTimeHigh: DWORD;
    SizeHigh, SizeLow: DWORD;
  end;

var
  OptimizedEdition: Boolean;
  UmbraDownloadPage: TDownloadWizardPage;
  // Both of Umbra's files are verified in {tmp}, waiting to be put in place.
  UmbraDownloaded: Boolean;
  // Set once Setup tried to install Umbra: whether she is now in place.
  UmbraTried, UmbraPlaced: Boolean;

function IsProcessorFeaturePresent(ProcessorFeature: Cardinal): BOOL;
  external 'IsProcessorFeaturePresent@kernel32.dll stdcall';

function GetFileAttributesEx(FileName: String; InfoLevelId: Integer; var Data: TUmbraFileData): BOOL;
  external 'GetFileAttributesExW@kernel32.dll stdcall';

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

// Where Nomadeer keeps Umbra's core: paths::data_dir / models / her folder.
function UmbraCoreDir(): String;
begin
  Result := ExpandConstant('{localappdata}\{#UmbraDataDir}\{#UmbraModelsDir}\{#UmbraCoreDir}');
end;

function Int64OfHalves(High, Low: DWORD): Int64;
begin
  Result := Int64(High) * 4294967296 + Int64(Low);
end;

// A file's size and last write time as Nomadeer reads them on Windows
// (Rust's fs::metadata): nanoseconds since 1970, from 100 ns intervals.
function UmbraFileStamp(Path: String; var Size, ModifiedNanos: Int64): Boolean;
var
  Data: TUmbraFileData;
begin
  Result := False;
  if not GetFileAttributesEx(Path, 0, Data) then
    Exit;
  if (Data.Attributes and FILE_ATTRIBUTE_DIRECTORY) <> 0 then
    Exit;
  Result := True;
  Size := Int64OfHalves(Data.SizeHigh, Data.SizeLow);
  ModifiedNanos := (Int64OfHalves(Data.LastWriteTimeHigh, Data.LastWriteTimeLow) - UNIX_EPOCH_FILETIME) * 100;
end;

function UmbraFileInPlace(Name: String; ExpectedSize: Int64): Boolean;
var
  Size, ModifiedNanos: Int64;
begin
  Result := UmbraFileStamp(UmbraCoreDir() + '\' + Name, Size, ModifiedNanos) and (Size = ExpectedSize);
end;

// Nomadeer checks a core already in place itself, so it is never downloaded
// again here.
function UmbraCoreInPlace(): Boolean;
begin
  Result := UmbraFileInPlace('{#UmbraModelFile}', {#UmbraModelSize})
    and UmbraFileInPlace('{#UmbraTokenizerFile}', {#UmbraTokenizerSize});
end;

function UmbraTaskOffered(): Boolean;
begin
  Result := not IsAdminInstallMode() and not RelaunchRequested() and not UmbraCoreInPlace();
end;

procedure DiscardUmbraDownloads();
begin
  DeleteFile(ExpandConstant('{tmp}\{#UmbraModelFile}'));
  DeleteFile(ExpandConstant('{tmp}\{#UmbraTokenizerFile}'));
end;

procedure InitializeWizard();
begin
  UmbraDownloadPage := CreateDownloadPage(CustomMessage('UmbraDownloadCaption'),
    CustomMessage('UmbraDownloadDescription'), nil);
  UmbraDownloadPage.ShowBaseNameInsteadOfUrl := True;
end;

// Downloads both files to {tmp}, where Inno checks each against its SHA-256.
// A failure only costs Umbra: Setup carries on without her.
procedure DownloadUmbra();
var
  Reason: String;
begin
  UmbraTried := True;
  UmbraDownloadPage.Clear;
  UmbraDownloadPage.Add('{#UmbraModelUrl}', '{#UmbraModelFile}', '{#UmbraModelSha256}');
  UmbraDownloadPage.Add('{#UmbraTokenizerUrl}', '{#UmbraTokenizerFile}', '{#UmbraTokenizerSha256}');
  UmbraDownloadPage.Show;
  try
    try
      UmbraDownloadPage.Download;
      UmbraDownloaded := True;
      Log('Umbra downloaded and verified.');
    except
      if UmbraDownloadPage.AbortedByUser then
        Reason := CustomMessage('UmbraCancelled')
      else
        Reason := GetExceptionMessage;
      Log('Umbra was not downloaded: ' + Reason);
      DiscardUmbraDownloads();
      SuppressibleMsgBox(FmtMessage(CustomMessage('UmbraNotDownloaded'), [AddPeriod(Reason)]),
        mbInformation, MB_OK, IDOK);
    end;
  finally
    UmbraDownloadPage.Hide;
  end;
end;

function NextButtonClick(CurPageID: Integer): Boolean;
begin
  if (CurPageID = wpReady) and WizardIsTaskSelected('umbra') then
    DownloadUmbra();
  Result := True;
end;

// Moves a verified download into Umbra's folder as Nomadeer itself would:
// through <name>.part, so an interrupted move never leaves a file under her
// real name.
function PlaceUmbraFile(Name: String; ExpectedSize: Int64; var Error: String): Boolean;
var
  Source, Destination, Part: String;
  Size, ModifiedNanos: Int64;
begin
  Result := False;
  Source := ExpandConstant('{tmp}\') + Name;
  Destination := UmbraCoreDir() + '\' + Name;
  Part := ChangeFileExt(Destination, '.part');
  DeleteFile(Part);
  if not RenameFile(Source, Part) then
#if Ver >= EncodeVer(6, 4, 0)
    if not CopyFile(Source, Part, False) then
#else
    if not FileCopy(Source, Part, False) then
#endif
    begin
      Error := 'could not write ' + Part;
      Exit;
    end;
  if not UmbraFileStamp(Part, Size, ModifiedNanos) or (Size <> ExpectedSize) then
  begin
    DeleteFile(Part);
    Error := Name + ' did not arrive whole';
    Exit;
  end;
  if FileExists(Destination) and not DeleteFile(Destination) then
  begin
    DeleteFile(Part);
    Error := 'could not replace ' + Destination;
    Exit;
  end;
  if not RenameFile(Part, Destination) then
  begin
    DeleteFile(Part);
    Error := 'could not write ' + Destination;
    Exit;
  end;
  Result := True;
end;

function UmbraMarkerLine(Name, Sha256: String; var Error: String): String;
var
  Size, ModifiedNanos: Int64;
begin
  Result := '';
  if not UmbraFileStamp(UmbraCoreDir() + '\' + Name, Size, ModifiedNanos) then
  begin
    Error := Name + ' disappeared';
    Exit;
  end;
  // local_intel::marker_contents: "<name> <sha256> <size> <modified nanos>\n".
  Result := Name + ' ' + Sha256 + ' ' + IntToStr(Size) + ' ' + IntToStr(ModifiedNanos) + #10;
end;

// Writes the marker Nomadeer writes once it has checksummed her files, so it
// trusts these without hashing them again. Inno checked the same SHA-256s.
function WriteUmbraMarker(var Error: String): Boolean;
var
  Marker, Part, Contents, Line: String;
begin
  Result := False;
  Contents := '';
  Line := UmbraMarkerLine('{#UmbraModelFile}', '{#UmbraModelSha256}', Error);
  if Line = '' then
    Exit;
  Contents := Contents + Line;
  Line := UmbraMarkerLine('{#UmbraTokenizerFile}', '{#UmbraTokenizerSha256}', Error);
  if Line = '' then
    Exit;
  Contents := Contents + Line;
  Marker := UmbraCoreDir() + '\{#UmbraMarkerFile}';
  Part := ChangeFileExt(Marker, '.part');
  if not SaveStringToFile(Part, Contents, False) then
  begin
    Error := 'could not write ' + Part;
    Exit;
  end;
  if (FileExists(Marker) and not DeleteFile(Marker)) or not RenameFile(Part, Marker) then
  begin
    DeleteFile(Part);
    Error := 'could not write ' + Marker;
    Exit;
  end;
  Result := True;
end;

procedure PlaceUmbra();
var
  Error: String;
begin
  Error := '';
  // Without its marker, whatever is in her folder is checked by Nomadeer before
  // she uses it, so a failure below never leaves anything she would trust.
  if not ForceDirectories(UmbraCoreDir()) then
    Error := 'could not create ' + UmbraCoreDir()
  else if FileExists(UmbraCoreDir() + '\{#UmbraMarkerFile}')
    and not DeleteFile(UmbraCoreDir() + '\{#UmbraMarkerFile}') then
    Error := 'could not replace her old marker'
  else if PlaceUmbraFile('{#UmbraModelFile}', {#UmbraModelSize}, Error)
    and PlaceUmbraFile('{#UmbraTokenizerFile}', {#UmbraTokenizerSize}, Error)
    and WriteUmbraMarker(Error) then
  begin
    UmbraPlaced := True;
    Log('Umbra installed in ' + UmbraCoreDir());
    Exit;
  end;
  Log('Umbra could not be set up: ' + Error);
  SuppressibleMsgBox(FmtMessage(CustomMessage('UmbraNotPlaced'), [AddPeriod(Error)]),
    mbInformation, MB_OK, IDOK);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and UmbraDownloaded then
    PlaceUmbra();
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
  // Say which edition went in, and whether Umbra did, just above "Click
  // Finish to exit Setup."
  Caption := WizardForm.FinishedLabel.Caption;
  Edition := FmtMessage(CustomMessage('EditionInstalled'), [EditionDescription()]);
  if UmbraPlaced then
    Edition := Edition + #13#10#13#10 + CustomMessage('UmbraInstalled')
  else if UmbraTried then
    Edition := Edition + #13#10#13#10 + CustomMessage('UmbraLater');
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
