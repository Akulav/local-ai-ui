; Lumen's Windows setup (Inno Setup 6): the usual wizard - Welcome, License, Folder, Shortcuts, Install,
; Finish (with "Launch Lumen") - in English or Romanian, per user, no administrator rights, with its
; uninstaller (Settings -> Apps and the Start menu). The release workflow builds it:
;
;   iscc /DAppVersion=0.9.0 /DExe=<path>\lumen.exe /O<out dir> src-tauri\installer\lumen.iss
;
; This setup is also the update: it is the only Windows file of a release. Lumen's Update button
; (src-tauri/src/updater.rs) downloads it and starts it on the install's folder:
;
;   Lumen-setup.exe /SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS /LAUNCH=1 /DIR=<folder> /LOG=<file>
;
; Before installing, an existing install is uninstalled silently (the program, shortcuts and Apps entry -
; LumenData with the models, chats and settings stays; only an uninstall where you answer "yes" deletes
; it), then the new version is installed and, with /LAUNCH=1, started. Run by hand over an install it
; does the same with its usual pages. Installs made by the old NSIS setup (Lumen 0.8 and earlier, same
; folder) are taken over the same way. The pictures come from scripts\installer-images.ps1.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef Exe
  #define Exe "..\target\release\lumen.exe"
#endif
; (tests compile a separate copy - /DName=LumenTest /DGuid=… /DNsisKeyName=… - so a test install never
; touches a real one: its shortcuts, Apps entry and the NSIS entry it takes over)
#ifndef Name
  #define Name "Lumen"
#endif
#ifndef Guid
  ; never change it: it is how an update or the uninstaller finds this install
  #define Guid "9627B1F1-110C-4E82-9331-519A0C00C734"
#endif
#ifndef NsisKeyName
  #define NsisKeyName "Lumen"
#endif
; (Windows file versions are numbers only: 1.0.0-beta.1 -> 1.0.0)
#define NumVersion Copy(AppVersion, 1, Pos("-", AppVersion + "-") - 1)

[Setup]
AppId={{{#Guid}}
AppName={#Name}
AppVersion={#AppVersion}
AppVerName=Lumen {#AppVersion}
AppPublisher=Akulav
AppCopyright=Made by Akulav
AppPublisherURL=https://github.com/Akulav/local-ai-ui
AppSupportURL=https://github.com/Akulav/local-ai-ui/issues
AppUpdatesURL=https://github.com/Akulav/local-ai-ui/releases
VersionInfoCompany=Akulav
VersionInfoCopyright=Made by Akulav
VersionInfoProductName=Lumen
VersionInfoDescription=Lumen Setup
VersionInfoVersion={#NumVersion}
DefaultDirName={code:DefaultDir}
; (Inno Setup hides the Welcome and folder pages unless told: this setup shows both)
DisableWelcomePage=no
DisableDirPage=no
DisableProgramGroupPage=yes
UsePreviousAppDir=yes
DirExistsWarning=no
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile=..\..\LICENSE
OutputBaseFilename=Lumen-setup
SetupIconFile=..\icons\icon.ico
UninstallDisplayIcon={app}\lumen.exe
UninstallDisplayName={#Name}
WizardStyle=modern
WizardImageFile=wizard-large.bmp,wizard-large-200.bmp
WizardSmallImageFile=wizard-small.bmp,wizard-small-200.bmp
Compression=lzma2/max
SolidCompression=yes
ShowLanguageDialog=auto
; (a running Lumen is closed - it takes llama-server with it - before its program is replaced)
CloseApplications=force
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
; (Romanian.isl translates the visible texts; anything it leaves out comes from Default.isl)
Name: "romanian"; MessagesFile: "compiler:Default.isl,Romanian.isl"

[CustomMessages]
english.AppComment=Chat with local AI models on this computer - made by Akulav
romanian.AppComment=Conversații cu modele AI locale pe acest calculator - creat de Akulav
english.DeleteData=Lumen is uninstalled.%n%nAlso delete its data in %1 ?%nThat deletes the downloaded models, chats, projects and settings for good.%n%nNo keeps them: installing Lumen again brings everything back.
romanian.DeleteData=Lumen a fost dezinstalat.%n%nȘtergi și datele din %1 ?%nAstfel se șterg definitiv modelele descărcate, conversațiile, proiectele și setările.%n%nNu le păstrează: la o nouă instalare, Lumen le regăsește.

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#Exe}"; DestDir: "{app}"; DestName: "lumen.exe"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#Name}"; Filename: "{app}\lumen.exe"; WorkingDir: "{app}"; Comment: "{cm:AppComment}"
Name: "{autoprograms}\{cm:UninstallProgram,{#Name}}"; Filename: "{uninstallexe}"; WorkingDir: "{app}"
Name: "{autodesktop}\{#Name}"; Filename: "{app}\lumen.exe"; WorkingDir: "{app}"; Comment: "{cm:AppComment}"; Tasks: desktopicon

[Run]
Filename: "{app}\lumen.exe"; Description: "{cm:LaunchProgram,Lumen}"; Flags: nowait postinstall skipifsilent
; (an update from Lumen's Update button: start the new version when the silent install is done)
Filename: "{app}\lumen.exe"; Parameters: "--after-update"; WorkingDir: "{app}"; Flags: nowait; Check: LaunchAfterUpdate

[UninstallDelete]
; (what an update leaves beside the program until Lumen's next start removes it)
Type: files; Name: "{app}\lumen.exe.old"
Type: filesandordirs; Name: "{app}\LumenData\updates"

[Code]
const
  NsisKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{#NsisKeyName}';

// Where Lumen goes: the folder of an install made by the old NSIS setup (so its LumenData is found),
// else %LOCALAPPDATA%\Lumen. (An install made by this setup is found through UsePreviousAppDir.)
function DefaultDir(Param: String): String;
var
  Dir: String;
begin
  if RegQueryStringValue(HKCU, NsisKey, 'InstallLocation', Dir) and (RemoveQuotes(Dir) <> '') then
    Result := RemoveQuotes(Dir)
  else
    Result := ExpandConstant('{localappdata}\Lumen');
end;

// A running Lumen from this folder is ended first (its job object takes llama-server with it).
procedure StopRunning(Dir: String);
var
  Code: Integer;
begin
  Exec('powershell.exe',
       '-NoProfile -NonInteractive -Command "Get-Process lumen -ErrorAction SilentlyContinue | ' +
       'Where-Object { $_.Path -eq ''' + Dir + '\lumen.exe'' } | Stop-Process -Force"',
       '', SW_HIDE, ewWaitUntilTerminated, Code);
  Sleep(1500);
end;

// /LAUNCH=1 (an update started by Lumen): start Lumen when the silent install is done.
function LaunchAfterUpdate(): Boolean;
begin
  Result := WizardSilent and (ExpandConstant('{param:LAUNCH|0}') = '1');
end;

// An install that is already there is uninstalled first, silently: the old program, its shortcuts and
// its Apps entry go; LumenData stays (the uninstallers delete it only when asked, never when silent).
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Uninst, Dir: String;
  Code: Integer;
begin
  Result := '';
  StopRunning(ExpandConstant('{app}'));
  // this setup's own previous version
  if RegQueryStringValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{' + '{#Guid}' + '}_is1',
                         'UninstallString', Uninst) then
  begin
    Uninst := RemoveQuotes(Uninst);
    if FileExists(Uninst) then
    begin
      Log('Uninstalling the installed version first (LumenData stays): ' + Uninst);
      if not Exec(Uninst, '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART', '', SW_HIDE, ewWaitUntilTerminated, Code) then
        Log('The old uninstaller did not start; installing over it');
    end;
  end;
  // Lumen 0.8 and earlier (NSIS). "_?=<folder>" makes the NSIS uninstaller run in place and wait -
  // without it, it copies itself away, returns at once and could delete the new lumen.exe later.
  if RegQueryStringValue(HKCU, NsisKey, 'UninstallString', Uninst) then
  begin
    Uninst := RemoveQuotes(Uninst);
    Dir := ExtractFileDir(Uninst);
    if FileExists(Uninst) then
    begin
      Log('Removing the NSIS install of Lumen first (LumenData stays): ' + Uninst);
      if Exec(Uninst, '/S _?=' + Dir, '', SW_HIDE, ewWaitUntilTerminated, Code) then
        DeleteFile(Uninst)
      else
        Log('The NSIS uninstaller did not start; installing over it');
    end;
    RegDeleteKeyIncludingSubkeys(HKCU, NsisKey);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Data: String;
begin
  if CurUninstallStep = usUninstall then
    StopRunning(ExpandConstant('{app}'));
  if (CurUninstallStep = usPostUninstall) and not UninstallSilent then
  begin
    Data := ExpandConstant('{app}\LumenData');
    if DirExists(Data) and (MsgBox(FmtMessage(CustomMessage('DeleteData'), [Data]), mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES) then
    begin
      DelTree(Data, True, True, True);
      RemoveDir(ExpandConstant('{app}'));
    end;
  end;
end;
