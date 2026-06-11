# Runbook — RAM safe-zone smoke test

Verifies the three tiers of the capacity governor on a live build: the safe-zone
cap + indicator (U2/U6), the live tightening under memory pressure (U3), and the
per-terminal Job Object backstop (U4).

## 1. Build and launch

```powershell
# Engine DLL first, then the app (cargo is NOT on PATH on the dev machine):
& C:\Users\steve\.cargo\bin\cargo.exe build --lib   # from engine\
dotnet build app\Optimus.App.csproj
Start-Process app\bin\Debug\net9.0-windows10.0.19041.0\win-x64\Optimus.App.exe
```

## 2. Verify the indicator at startup

In the sidebar, directly above "+ New workspace", you should see
**"X / Y terminals"** plus a thin fill bar (muted grey while calm).

Sanity-check `Y` (MaxTerminals) against the machine, remembering it derives from
**live headroom at launch**, not total RAM:

```
safeZone = min(availablePhysicalRAM, commitHeadroom) − 2 GB (OS reserve)
Y        = floor(safeZone / perTerminalBudget)        # budget seeds at 200 MB
```

e.g. ~6 GB available → Y ≈ 20; a heavily loaded 16 GB box with ~3 GB free →
Y ≈ 5. `Y` is *not* fixed per machine — re-run the math with current free RAM
(`Get-CIMInstance Win32_OperatingSystem | % FreePhysicalMemory`, in KB). After
calibration persists a larger measured budget, Y shrinks accordingly.

Each workspace/terminal you create should increment `X`; the bar fills, turns
`git-dirty` amber at ≥ 75% of Y, and `pr-closed` red at the cap, where the
"+ New workspace" button disables with the hint
"Safe-zone full — close a workspace to spawn more". Closing a workspace frees
the slot and re-enables the button. If the governor failed to start, the
indicator shows "— / — terminals" and never blocks spawns (fail-open by design).

## 3. Force low memory and watch the cap tighten

Run a ballast allocator *outside* Optimus (so the Job Object backstop doesn't
interfere), e.g.:

```powershell
# Hold ~4 GB of committed, touched pages until you press Enter:
python -c "x = bytearray(4 * 1024**3); input()"
# or pure PowerShell (slower to allocate):
$ballast = foreach ($i in 1..40) { New-Object byte[] (100MB) }; pause
```

Within ~1–2 s (the 1 Hz ticker) the indicator's `Y` drops. Tightening is
immediate; recovery is deliberately sticky — after releasing the ballast,
available RAM must stay ≥ 1.25× the tightened safe zone for 2 consecutive ticks
(and the OS low-memory signal must be clear) before `Y` relaxes back up. A
shrink below the current terminal count never kills terminals; it only blocks
new spawns (`X / Y` can read e.g. `7 / 5`).

## 4. Verify the Job Object backstop

Each terminal's ConPTY child shell is enrolled in a Job Object limited to
2× the per-terminal budget with `KILL_ON_JOB_CLOSE`. Inside one terminal, run a
runaway allocation:

```powershell
python -c "x = bytearray(3 * 1024**3)"     # MemoryError / killed — terminal-local
# or:
$x = [byte[]]::new(3GB)
```

Expected: the allocation fails (or the shell dies) **inside that terminal
only** — the app, the indicator, and all sibling terminals stay alive. To
confirm enrollment without typing into a terminal, check the shell PID
(child of Optimus.App.exe) with `IsProcessInJob`:

```powershell
$src = 'using System;using System.Runtime.InteropServices;public static class J{[DllImport("kernel32")]public static extern bool IsProcessInJob(IntPtr p,IntPtr j,out bool r);}'
Add-Type $src
$shell = Get-CimInstance Win32_Process -Filter "Name='pwsh.exe' or Name='powershell.exe' or Name='cmd.exe'" |
  Where-Object { (Get-CimInstance Win32_Process -Filter "ProcessId=$($_.ParentProcessId)").Name -eq 'OpenConsole.exe' -or
                 (Get-Process -Id $_.ParentProcessId -ErrorAction SilentlyContinue).Name -like '*Optimus*' }
$h = (Get-Process -Id $shell.ProcessId).Handle; $in = $false
[J]::IsProcessInJob($h, [IntPtr]::Zero, [ref]$in); $in   # → True
```

Enrollment is best-effort: a failure logs `TerminalPane.EnrollChildInJobObject`
and leaves the terminal un-backstopped rather than blocking the spawn.

## 5. Verify calibration save-on-exit (p6 U2)

On graceful shutdown the window's `Closed` handler tears the governor down
(ticker first, then provider) and persists the learned budget via
`SaveCalibration()`. Confirm the file actually mutates:

```powershell
$cal = "$env:LOCALAPPDATA\optimus\capacity.json"
$before = (Get-Item $cal -ErrorAction SilentlyContinue).LastWriteTime
# Launch the app, spawn ≥ 3 surfaces, then close the window normally (titlebar X).
$after = (Get-Item $cal).LastWriteTime
"$before → $after"          # after must be newer (file created if it was absent)
Get-Content $cal            # budgetBytes reflects the session's learned budget
```

Expected: `LastWriteTime` advances past the app-exit moment and the JSON still
has the `{ "budgetBytes": …, "hardwareFingerprintGb": … }` shape. A save
failure is non-fatal — it logs through `App.LogError` with source
`App.StopCapacityGovernor` (or `JsonCalibrationStore.Save`) instead of blocking
shutdown.

## 6. Where state and logs live

- **Calibration:** `%LOCALAPPDATA%\optimus\capacity.json` —
  `{ "budgetBytes": …, "hardwareFingerprintGb": … }`. Delete it to reset to the
  200 MB seed budget. It is ignored when the fingerprint (total RAM, GB) doesn't
  match the machine.
- **Debug log lines** (debugger / DebugView): every capacity state change emits
  `[capacity] used=… reserved=… max=… level=Calm|Warn|Cap`.
- **Recovered errors** (governor startup failure, ticker faults, job-object
  enrollment failure, calibration save failure) go through `App.LogError` with
  sources `App.StartCapacityGovernor`, `CapacityTicker.Tick`,
  `CapacityTicker.OnLowMemorySignaled`, `TerminalPane.EnrollChildInJobObject`,
  `JsonCalibrationStore.Save`.

## 7. Screenshot capture note (dev machine)

Programmatic verification uses `SetProcessDPIAware` + `PrintWindow` /
`CopyFromScreen`. DPI-awareness is **required** — without it the right side of
the window is cropped at 125% scaling (see memory `gui-screenshot-dpi-capture`).
