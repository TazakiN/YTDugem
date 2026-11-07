; Inno Setup script for YTDugem's Windows installer. Built by scripts/build-installer.ps1 from the
; extracted portable ZIP, so the installer and the ZIP carry exactly the same files.
;
; Per-user install into %LOCALAPPDATA%\Programs\YTDugem: no admin prompt, and the folder stays
; writable, so the in-app updater (src/update.rs) can update an installed copy too.
; Never change AppId: Windows matches upgrades and the uninstaller by it, and src/update.rs
; (INSTALL_KEY) updates its DisplayVersion after an in-app update.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #error Pass /DSourceDir=<extracted YTDugem folder>
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

[Setup]
AppId={{C8AB3870-DAD2-4919-849F-1B729CD32A70}
AppName=YTDugem
AppVersion={#AppVersion}
AppPublisher=YTDugem contributors
AppPublisherURL=https://github.com/TazakiN/YTDugem
AppSupportURL=https://github.com/TazakiN/YTDugem/issues
AppUpdatesURL=https://github.com/TazakiN/YTDugem/releases
; Without the version: an in-app update changes the files, and only DisplayVersion is updated.
UninstallDisplayName=YTDugem
UninstallDisplayIcon={app}\YTDugem.exe
DefaultDirName={localappdata}\Programs\YTDugem
; A fixed folder: the uninstaller removes whole subfolders inside it (see [UninstallDelete]).
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile={#SourceDir}\LICENSE
SetupIconFile=..\..\assets\icon.ico
OutputDir={#OutputDir}
OutputBaseFilename=YTDugem-windows-x86_64-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\YTDugem"; Filename: "{app}\YTDugem.exe"
Name: "{autodesktop}\YTDugem"; Filename: "{app}\YTDugem.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\YTDugem.exe"; Description: "{cm:LaunchProgram,YTDugem}"; Flags: nowait postinstall skipifsilent

; An in-app update may add files the installer never recorded. The library and sign-in live in
; the user's application-data folder and are kept.
[UninstallDelete]
Type: filesandordirs; Name: "{app}\.update"
Type: filesandordirs; Name: "{app}\tools"
Type: filesandordirs; Name: "{app}\third-party"
Type: filesandordirs; Name: "{app}\source"
