# Runbook — clean-machine install smoke (p6 U3)

Verifies the self-contained publish + installer run on a machine (or profile)
with **no .NET runtime, no Windows App SDK, no dev tooling** — the guarantee
the packaging spike exists to prove. The headline check is the same as every
Optimus smoke: the app comes up with the **safe-zone capacity indicator** live
in the sidebar, because capacity-aware multiplexing is the product.

## 0. Build the artifacts (dev machine)

```powershell
.\build\build.ps1 -Configuration Release -Publish
iscc installer\optimus.iss        # optional — only if testing the installer itself
```

Outputs:

- `app\bin\Release\net9.0-windows10.0.19041.0\win-x64\publish\` — full app folder
- `cli\bin\Release\net9.0\win-x64\publish\optimus.exe` — single-file CLI
- `installer\out\OptimusSetup-<version>.exe` — installer (if compiled)

## 1. Pick a clean environment

**Best:** a clean Windows 10/11 x64 VM with no Visual Studio, no .NET SDK.

**Acceptable fallback (no VM):** a fresh local user profile on the dev machine.
Create and log into it once, then run the smoke from inside it:

```powershell
# From an admin shell on the dev machine:
net user optsmoke /add
runas /user:optsmoke cmd      # or sign in via the lock screen for a real session
```

A fresh profile does NOT remove machine-wide runtimes, so on the dev machine
this only proves per-user state isolation (no `%LOCALAPPDATA%\optimus`, no user
PATH entry, default UDFs) — note that limitation in any sign-off. The
no-runtime claim can only be proven on a machine without the .NET SDK
installed machine-wide.

## 2. Smoke the raw publish folder (xcopy deploy)

Copy the entire `publish\` folder to the clean environment (e.g.
`C:\smoke\Optimus\`) and run `Optimus.exe` from there. Expected:

1. Window opens; sidebar shows **"X / Y terminals"** capacity indicator with a
   real `Y` (the safe-zone cap computed from this machine's free RAM — see the
   [RAM safe-zone smoke](2026-06-10-ram-safe-zone-smoke.md) §2 for the math).
2. A terminal spawns and accepts input (proves `optimus_engine.dll` + ConPTY
   work without dev tooling).
3. Close the window. The process exits within ~5 s with **code 0** and writes
   **no** Event Log → Application → "Application Error" entry (res U4 release-exit
   teardown fix — see the note below §2). `%LOCALAPPDATA%\optimus\capacity.json`
   exists with `{ "budgetBytes": …, "hardwareFingerprintGb": … }` (proves the
   p6 U2 save-on-exit path on a cold profile).
4. `bin`-less CLI check: copy `optimus.exe` (single-file CLI) anywhere, run
   `optimus.exe list` *while the app is up* — it must answer over the pipe, and
   exit promptly (not hang) when stdin is redirected (res U2 regression check):
   `Get-Content NUL | optimus.exe list`.

Failure triage:

| Symptom | Likely cause |
|---|---|
| Instant exit, no window, `crash.log` mentions resource map / XAML | `Optimus.pri` missing from publish — the `IncludeProjectPriInPublish` target regressed |
| "optimus_engine.dll not found" in `crash.log` | engine DLL not staged into publish (csproj `None Include` glob) |
| Window opens but indicator reads "— / — terminals" | governor failed to start — check `crash.log` for `App.StartCapacityGovernor` (fail-open is by design; the *cause* still needs fixing) |
| App needs a ".NET runtime" download prompt | publish wasn't `--self-contained` — rebuild via `build.ps1 -Publish` |

> **Fixed by res U4 (2026-06-13) — this is now the regression check.** The spike
> found that with the *release-profile* engine the process lingered ~60 s after
> window close and then died with 0xC0000005 in `D3D12Core.dll` (a wgpu/D3D12
> teardown race: the DX12 device/swapchain were torn down while a frame was still
> in flight, so D3D12's deferred-destruction queue freed resources the GPU was
> still reading — release builds retired frames fast enough to lose the race,
> debug builds drained in time). res U4 fixes it in the engine: `TerminalRenderer`
> now blocks on `device.poll(PollType::Wait)` before teardown and drops its GPU
> resource layers (then `panel`: surface → queue → device → instance) in that
> order. **Verify the fix:** with a *Release* publish (release engine DLL), close
> the window after spawning ≥3 terminals — the process must exit within ~5 s with
> **code 0** and leave **no** Event Log → Application → "Application Error" entry.
> A lingering process or an Application Error is a res U4 regression.
>
> **iGPU caveat:** the crash is a timing race and did **not** reproduce on the dev
> laptop's integrated graphics even with the *pre-fix* engine — fast frame retirement
> never lost the race there. A clean run on an iGPU therefore proves "no regression,"
> not "race fixed." To actually exercise the failure window, run this check on a
> discrete-GPU machine (or under heavy render load) where teardown can outrun the GPU.

## 3. Smoke the installer

Run `OptimusSetup-<version>.exe` in the clean environment:

1. No UAC prompt (per-user install). Lands in `%LOCALAPPDATA%\Programs\Optimus`.
2. Start-menu shortcut launches the app; same checks as §2.
3. With the "Add the optimus CLI to your user PATH" task checked: open a NEW
   terminal, `optimus list` resolves from `…\Optimus\bin\optimus.exe`.
4. WebView2: if the Evergreen bootstrapper was bundled and the machine lacked
   the runtime, the `pv` registry value now exists (see
   [installer/README.md](../../installer/README.md) for the exact keys).
   Without the bundle, setup must complete fine anyway — WebView2 is only
   needed by the future U4 pane, never by the terminal itself.
5. Uninstall from Settings → Apps: app folder and PATH entry removed.
   `%LOCALAPPDATA%\optimus` (calibration) intentionally survives uninstall.

## 4. Sign-off line for the tracker

`clean-install smoke: <VM | fresh profile (machine-runtime caveat)> · publish-folder run OK · installer run OK · indicator live · capacity.json written · CLI on PATH OK`
