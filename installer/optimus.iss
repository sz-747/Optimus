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
; Per-user install: no UAC prompt. The user Programs folder is spelled out rather
; than left to {autopf}+lowest resolution so the README's path claim is literal.
DefaultDirName={localappdata}\Programs\{#AppName}
DefaultGroupName={#AppName}
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

[Code]
{ User-PATH handling lives here rather than in [Registry] with {olddata}: the
  declarative append corrupts the value when Path is absent/empty (leading ';')
  and can't be undone on uninstall. Add/remove are segment-wise, normalize
  trailing backslashes, and only ever touch HKCU (per-user install). The
  ChangesEnvironment=yes broadcast tells running shells to re-read it. }

function SameSeg(const A, B: string): Boolean;
var
  NA, NB: string;
begin
  NA := Trim(A);
  NB := Trim(B);
  while (Length(NA) > 0) and (NA[Length(NA)] = '\') do SetLength(NA, Length(NA) - 1);
  while (Length(NB) > 0) and (NB[Length(NB)] = '\') do SetLength(NB, Length(NB) - 1);
  Result := CompareText(NA, NB) = 0;
end;

{ Rebuild Path without Dir; report whether it was present. Empty segments are
  preserved as-is unless they match (they can't), so unrelated formatting survives. }
function RemoveSegFromPath(const PathValue, Dir: string; var NewValue: string): Boolean;
var
  Seg: string;
  I: Integer;
begin
  Result := False;
  NewValue := '';
  Seg := '';
  for I := 1 to Length(PathValue) + 1 do
  begin
    if (I = Length(PathValue) + 1) or (PathValue[I] = ';') then
    begin
      if (Trim(Seg) <> '') and SameSeg(Seg, Dir) then
        Result := True
      else if Seg <> '' then
      begin
        if NewValue <> '' then NewValue := NewValue + ';';
        NewValue := NewValue + Seg;
      end;
      Seg := '';
    end
    else
      Seg := Seg + PathValue[I];
  end;
end;

procedure AddToUserPath(const Dir: string);
var
  V, Ignored: string;
begin
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', V) then
    V := '';
  if RemoveSegFromPath(V, Dir, Ignored) then
    exit; { already present (any trailing-backslash spelling) }
  if (V <> '') and (V[Length(V)] <> ';') then
    V := V + ';';
  V := V + Dir;
  RegWriteExpandStringValue(HKCU, 'Environment', 'Path', V);
end;

procedure RemoveFromUserPath(const Dir: string);
var
  V, NewV: string;
begin
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', V) then
    exit;
  if RemoveSegFromPath(V, Dir, NewV) then
    RegWriteExpandStringValue(HKCU, 'Environment', 'Path', NewV);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then
    AddToUserPath(ExpandConstant('{app}\bin'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromUserPath(ExpandConstant('{app}\bin'));
end;

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
