; Inno Setup 6 script for Unlatch.
;
; Setup is a thin wrapper: it unpacks Unlatch.exe and unlatch_shell.dll to a
; temporary folder and runs `Unlatch.exe --install --quiet`. Unlatch's own
; installer then copies itself to %LOCALAPPDATA%\Programs\Unlatch, adds the
; right-click entries (including the Windows 11 top-level menu), and registers
; its own "Installed apps" entry, which is also how it is uninstalled. Doing
; it this way means Setup and `--install` can never disagree, and the shell
; DLL that Explorer keeps loaded is replaced safely on upgrades.
;
; Build (from the repository root, after `cargo build --release --workspace`):
;   ISCC.exe /DAppVersion=0.1.0 installer\Unlatch.iss

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef BinDir
  #define BinDir "..\target\release"
#endif

[Setup]
AppId={{A270C354-D4EC-451F-AD09-E9D0089B6F4E}
AppName=Unlatch
AppVersion={#AppVersion}
AppVerName=Unlatch {#AppVersion}
AppPublisher=Unlatch
AppPublisherURL=https://github.com/AdriaanBoshoff/Unlatch
AppSupportURL=https://github.com/AdriaanBoshoff/Unlatch/issues
VersionInfoVersion={#AppVersion}.0
VersionInfoDescription=Unlatch Setup
; Per-user install: no UAC prompt, nothing outside the user profile.
PrivilegesRequired=lowest
; Unlatch places and uninstalls itself, so Setup needs neither an app folder
; nor an uninstaller of its own.
CreateAppDir=no
Uninstallable=no
DisableProgramGroupPage=yes
LicenseFile=..\LICENSE
SetupIconFile=..\assets\unlatch.ico
WizardStyle=modern
WizardSizePercent=100
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
Compression=lzma2/max
SolidCompression=yes
OutputDir=..\target\installer
OutputBaseFilename=Unlatch-{#AppVersion}-Setup

[Messages]
WelcomeLabel2=This will install [name/ver] on your computer.%n%nUnlatch shows which programs are using a file or folder, and lets you close them so you can delete, move or rename it.%n%nIt is installed for your user account only and does not need administrator rights.
FinishedHeadingLabel=Unlatch is ready
FinishedLabelNoIcons=Right-click any file or folder and choose Unlock with Unlatch.%n%nTo remove Unlatch later, use Settings > Apps > Installed apps.

[Files]
Source: "{#BinDir}\unlatch.exe"; DestDir: "{tmp}"; DestName: "Unlatch.exe"; Flags: deleteafterinstall
Source: "{#BinDir}\unlatch_shell.dll"; DestDir: "{tmp}"; Flags: deleteafterinstall

[Code]
procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
begin
  if CurStep <> ssPostInstall then
    exit;
  WizardForm.StatusLabel.Caption := 'Adding Unlatch to the right-click menu...';
  if not Exec(ExpandConstant('{tmp}\Unlatch.exe'), '--install --quiet', '',
              SW_HIDE, ewWaitUntilTerminated, ResultCode) then
    ResultCode := 1;
  case ResultCode of
    0: ;
    2: MsgBox('Unlatch is installed, but its menu entry is under "Show more options".' + #13#10#13#10 +
              'The Windows 11 top-level menu needs Developer Mode (Settings > System > For developers). ' +
              'Turn it on and run Setup again to add it.', mbInformation, MB_OK);
  else
    MsgBox('Unlatch could not be installed.' + #13#10#13#10 +
           'If Unlatch is open, close it and run Setup again.', mbError, MB_OK);
  end;
end;
