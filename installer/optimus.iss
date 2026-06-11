; Optimus installer (p6 U3) — Inno Setup 6 script.
;
; Optimus is the capacity-aware terminal multiplexer: it measures available RAM at
; startup and locks a hard safe-zone cap on terminal count so parallel agents can
; never crash the machine by over-allocating. This installer ships the unpackaged
; self-contained publish output — no .NET runtime, no Windows App SDK runtime, and
; no MSIX requirement on the target machine.
;
; Build inputs (produce them first, from the repo root):
;   .\build\build.ps1 -Configuration Release -Publish
; which yields:
;   app\bin\Release\net9.0-windows10.0.19041.0\win-x64\publish\   (app, self-contained)
;   cli\bin\Release\net9.0\win-x64\publish\optimus.exe            (CLI, single-file)
;
; Compile (Inno Setup 6.x):
;   iscc installer\optimus.iss
; Output: installer\out\OptimusSetup-<version>.exe
;
; WebView2 note (contract consumed by p6 U4): this installer does NOT hard-require
; the WebView2 Evergreen runtime — the terminal works without it; only WebView2
; panes need it. If MicrosoftEdgeWebview2Setup.exe (the Evergreen bootstrapper) is
; placed next to this script before compiling, setup runs it silently when no
; runtime is detected. See installer\README.md for the full detection contract.

#define AppName "Optimus"
#define AppVersion "0.6.0"
#define AppPublisher "Optimus"
#define AppExeName "Optimus.exe"
#define RepoRoot ".."
#define AppPublishDir RepoRoot + "\app\bin\Release\net9.0-windows10.0.19041.0\win-x64\publish"
#define CliPublishDir RepoRoot + "\cli\bin\Release\net9.0\win-x64\publish"

[Setup]
AppId={{9C2B5A14-7E0D-4B7A-9C66-0F3D2C1A8E51}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
; Per-user install: no UAC prompt, lands under %LOCALAPPDATA%\Programs\Optimus.
; (With PrivilegesRequired=lowest, {autopf} resolves to the user Programs folder.)
PrivilegesRequired=lowest
DisableProgramGroupPage=yes
OutputDir=out
OutputBaseFilename=OptimusSetup-{#AppVersion}
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
ChangesEnvironment=yes
UninstallDisplayIcon={app}\{#AppExeName}

[Tasks]
Name: "addtopath"; Description: "Add the optimus CLI to your user PATH"; GroupDescription: "Shell integration:"

[Files]
; Self-contained app publish output, verbatim (includes optimus_engine.dll,
; Windows App SDK runtime DLLs, and Optimus.pri — see csproj IncludeProjectPriInPublish).
Source: "{#AppPublishDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
; Single-file CLI under a bin\ subdir so PATH exposes only the CLI verb, not the app's DLL forest.
Source: "{#CliPublishDir}\optimus.exe"; DestDir: "{app}\bin"; Flags: ignoreversion
; Optional WebView2 Evergreen bootstrapper — only packaged if present at compile time.
#if FileExists("MicrosoftEdgeWebview2Setup.exe")
Source: "MicrosoftEdgeWebview2Setup.exe"; DestDir: "{tmp}"; Flags: deleteafterinstall
#endif

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExeName}"
Name: "{group}\Uninstall {#AppName}"; Filename: "{uninstallexe}"

[Run]
#if FileExists("MicrosoftEdgeWebview2Setup.exe")
; Evergreen bootstrap, silent, only when no runtime is present (see [Code] check).
Filename: "{tmp}\MicrosoftEdgeWebview2Setup.exe"; Parameters: "/silent /install"; \
  StatusMsg: "Installing Microsoft Edge WebView2 runtime..."; \
  Check: not IsWebView2RuntimePresent; Flags: waituntilterminated
#endif
Filename: "{app}\{#AppExeName}"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent

[Registry]
; Append {app}\bin to the *user* PATH (per-user install ⇒ never touch the machine PATH).
; Inno has no native "append to PATH", so guard against duplicates in [Code].
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; \
  ValueData: "{olddata};{app}\bin"; Tasks: addtopath; Check: NeedsAddPath(ExpandConstant('{app}\bin'))

[Code]
{ WebView2 Evergreen runtime detection — THE CONTRACT p6 U4's in-app check mirrors.
  The runtime is present iff one of these registry values exists with a non-empty,
  non-"0.0.0.0" pv (version) string:
    per-machine x64:  HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}\pv
    per-user:         HKCU\Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}\pv }
function IsWebView2RuntimePresent: Boolean;
var
  Version: string;
begin
  Result :=
    (RegQueryStringValue(HKLM, 'SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', Version)
      and (Version <> '') and (Version <> '0.0.0.0')) or
    (RegQueryStringValue(HKCU, 'Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', Version)
      and (Version <> '') and (Version <> '0.0.0.0'));
end;

function NeedsAddPath(Param: string): Boolean;
var
  OrigPath: string;
begin
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', OrigPath) then
  begin
    Result := True;
    exit;
  end;
  { Already present (delimited match) ⇒ skip. }
  Result := Pos(';' + Lowercase(Param) + ';', ';' + Lowercase(OrigPath) + ';') = 0;
end;
