# Optimus installer (p6 U3)

Optimus is the **capacity-aware terminal multiplexer** — it measures available
RAM at startup and locks a hard safe-zone cap on terminal count, so spawning
parallel agents can never crash the machine. This directory packages that
guarantee for machines with **nothing pre-installed**: no .NET runtime, no
Windows App SDK runtime, no MSIX.

## Pipeline

```powershell
# 1. From the repo root — self-contained Release publish of engine + app + CLI:
.\build\build.ps1 -Configuration Release -Publish

# 2. Compile the installer (requires Inno Setup 6, `iscc` on PATH):
iscc installer\optimus.iss
# → installer\out\OptimusSetup-<version>.exe
```

What the publish step guarantees (all enforced by `app/Optimus.App.csproj`):

| Property | Why |
|---|---|
| `WindowsPackageType=None` | unpackaged — runs from any folder, no MSIX identity |
| `SelfContained=true` | .NET 9 runtime carried in the publish folder |
| `WindowsAppSDKSelfContained=true` | Windows App SDK runtime DLLs carried too |
| `IncludeProjectPriInPublish` target | the app's own `Optimus.pri` lands in publish (publish drops it by default and the app then can't resolve compiled XAML) |
| CLI: `--self-contained -p:PublishSingleFile=true` | one-file `optimus.exe`, no runtime needed (`build.ps1 -Publish`) |

The installer is **per-user** (`PrivilegesRequired=lowest`): no UAC, installs to
`%LOCALAPPDATA%\Programs\Optimus`, CLI at `…\Optimus\bin\optimus.exe` with an
opt-in user-PATH task. MSIX remains deliberately out of scope for this spike —
`WindowsPackageType=None` is load-bearing for the unpackaged run path, and
nothing in Phase 6 needs package identity.

## WebView2 Evergreen bootstrap contract (consumed by p6 U4)

The terminal itself **does not need WebView2**; only the future WebView2 pane
(p6 U4) does. The contract between installer and app:

1. **Detection** — the WebView2 Evergreen runtime is present iff a non-empty,
   non-`0.0.0.0` `pv` (version) string exists at either:
   - per-machine x64: `HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}\pv`
   - per-user: `HKCU\Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}\pv`

   In-app (U4), prefer the SDK call that wraps exactly this check:
   `CoreWebView2Environment.GetAvailableBrowserVersionString()` — non-null ⇒
   runtime usable. Windows 11 ships the runtime inbox; Windows 10 may not.

2. **Install-time bootstrap (optional)** — if `MicrosoftEdgeWebview2Setup.exe`
   (the Evergreen bootstrapper, ~2 MB, downloads the runtime; fetch from
   <https://developer.microsoft.com/microsoft-edge/webview2/>) is placed next to
   `optimus.iss` before compiling, setup runs it silently **only when the
   detection above fails**. The bootstrapper is not committed to the repo.

3. **Runtime fallback (U4's job)** — if the runtime is still absent when a user
   opens a WebView2 pane, U4 must degrade gracefully: show an inline "WebView2
   runtime required" pane with a launch link to the bootstrapper, never crash,
   and never block terminal spawning (mirrors the governor's fail-open rule).

4. **UDF requirement (U4's job, restated from the tracker)** — unpackaged apps
   get a non-writable default user-data-folder next to the exe; U4 must create
   the environment with an explicit per-user UDF (e.g.
   `%LOCALAPPDATA%\optimus\webview2`) via
   `CoreWebView2Environment.CreateWithOptionsAsync`, or init throws.

## Verifying a build

See [docs/runbooks/2026-06-11-clean-install-smoke.md](../docs/runbooks/2026-06-11-clean-install-smoke.md)
for the clean-machine smoke procedure (fresh local user profile variant included).
