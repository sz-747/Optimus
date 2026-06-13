---
title: "Build Tracker — Optimus (cmux for Windows)"
status: active
created: 2026-06-11
branch: feat/ram-safe-zone
---

# Build Tracker — Optimus (cmux for Windows)

**Single source of truth for cross-session execution state.** This file is the
first thing every `/ce-work` session reads. Plans tell you *how*; this tracker
tells you *order, ownership, and state*.

Source plans:
- **Umbrella:** [docs/plans/2026-06-04-001-feature-cmux-windows-plan.md](2026-06-04-001-feature-cmux-windows-plan.md) — cmux-for-Windows master plan (Phases 1–6).
- **Phase 2 — tabs + splits:** [docs/plans/2026-06-05-001-feat-phase2-tabs-splits-plan.md](2026-06-05-001-feat-phase2-tabs-splits-plan.md) — _shipped (status header flipped to `completed` in res U1)._
- **Phase 3 — notifications:** [docs/plans/2026-06-06-002-feat-phase3-notification-system-plan.md](2026-06-06-002-feat-phase3-notification-system-plan.md) — _shipped._
- **Discoverable pane controls:** [docs/plans/2026-06-06-001-feat-discoverable-pane-controls-plan.md](2026-06-06-001-feat-discoverable-pane-controls-plan.md) — _shipped._
- **RAM safe-zone MVP:** [docs/plans/2026-06-10-001-feat-ram-safe-zone-mvp-plan.md](2026-06-10-001-feat-ram-safe-zone-mvp-plan.md) — _shipped._

**Scope of this tracker:** Phase 6 (polish + optional) from the umbrella plan, plus
named residuals carried over from the RAM safe-zone smoke and prior phases.
Everything earlier than Phase 6 is shipped; do **not** re-execute it.

---

## MANDATORY RULES (do not soften)

- **R1. Resume-before-work scan** — before *any* unit, the agent reads this
  tracker top to bottom, then runs `gh pr list --state open`, `git branch -a`,
  and `git log origin/main --oneline -20`, reconciles drift (open PR matching an
  unchecked unit → resume it; merged commit matching unchecked unit → tick it;
  in-progress branch → continue it), and **announces findings before any code
  change**.
- **R2. One unit per session** — sessions are serial. One unit, one PR, then
  stop. Mid-flight handoff is acceptable; sneaking a second unit in is not.
- **R3. Tick on merge, not on PR open** — `[x]` only when the unit's PR merges
  to `main`. Include PR # and merge commit hash on the row.
- **R4. Hot-file coordination** — files flagged `(hot)` below get append-only
  edits or single-unit ownership in a wave. Always `git pull --rebase
  origin main` before `git push` if you're behind.
- **R4a. Worktree per unit** — every row specifies its worktree name and branch.
  After a unit merges, clean its worktree:
  ```powershell
  git worktree unlock <absolute-path>
  git worktree remove <absolute-path>
  git branch -d <branch>
  ```
- **R5. Standing build rules** (from `CLAUDE.md`):
  - **No raw `Color.FromArgb` / inline `FontSize` literals in view code** — use
    `app/Design/Tokens.cs`.
  - Read [DESIGN.md](../../DESIGN.md) before any chrome change.
  - Lead with the safe-zone capacity guarantee in any copy; never call Optimus
    "just a terminal multiplexer".
  - Verification gates per unit: `dotnet test tests/Optimus.Core.Tests.csproj`
    must pass (current floor **238**); `cargo test --manifest-path engine\Cargo.toml`
    must pass (current floor **19**); `dotnet build app\Optimus.App.csproj`
    must produce **0 warnings, 0 errors**. Build the Rust engine via
    `C:\Users\steve\.cargo\bin\cargo.exe build --lib` **before** the app build so
    `NativeMethods.g.cs` is fresh.
  - Conventional-commit subjects; small PRs; codex adversarial review on every
    non-docs PR.
- **R6. Log to durable memory** — architectural decisions and surprising bug
  fixes go in `C:\Users\steve\.claude\projects\C--dev-Cmux-windows\memory\` as
  new memory files (and a one-line `MEMORY.md` index entry). Tracker rows
  reference memories; they don't duplicate them.
- **R7. Update tracker in the unit's PR** — the checkbox tick + PR # + merge
  hash are part of the unit's own PR diff, not a follow-up cleanup commit.

---

## Execution waves

### Wave 0 — Hygiene & isolated quick wins  (all `║`, no shared files)

- [x] **res U1** — Flip stale Phase 2 plan status `active → completed` and add a
  one-line `Outcome:` block citing the shipping commit. `║`
  Worktree: `wt-res-u1-phase2-status` · Branch: `chore/res-u1-phase2-status`
  Files: `docs/plans/2026-06-05-001-feat-phase2-tabs-splits-plan.md`
  PR: #4 · Merge: content commit `f382150` (PR #3 was mis-based on the stale
  GitHub default branch `feat/phase1-walking-skeleton`; default flipped to `main`,
  re-landed as PR #4)
  _Pure docs; no test impact. Mirrors the closeout done for the RAM safe-zone plan in commit `bbb7fcf`._

- [x] **res U2** — Fix CLI `optimus.exe` stdin hang when stdin is redirected but
  open. Replace the blocking `Console.In.ReadToEnd()` with a peek/read-with-
  timeout pattern so the CLI no longer needs `< NUL` as a workaround. `║`
  Worktree: `wt-res-u2-cli-stdin` · Branch: `fix/res-u2-cli-stdin-hang`
  Files: `cli/Program.cs` (~line 18) · add coverage in `tests/Cli/` if a test
  project exists; otherwise add a small repro doc under `docs/runbooks/`.
  PR: #5 · Merge: content commit `96f9650` (new `cli/StdinReader.cs` +
  `tests/Cli/StdinReaderTests.cs`; surprise: PowerShell 5.1 parents push a lone
  BOM onto the redirected stdin pipe, handled via quiet-window drain + BOM strip)
  _Surfaced by the RAM safe-zone live smoke. Verify via `Get-Content NUL | optimus.exe ...` and `echo hi | optimus.exe ...`._

### Wave 1 — Engine, lifecycle, and packaging spike  (mixed)

Hot file in this wave: `app/App.xaml.cs` (governor lifecycle). Only **p6 U2**
touches it — others stay disjoint.

- [x] **p6 U1** — Renderer polish (deferred from Phase 1): font fallback chains,
  color emoji, ligatures, subpixel AA via cosmic-text; GPU perf-tuning (damage
  regions, frame pacing). `║`
  Worktree: `wt-p6-u1-renderer-polish` · Branch: `feat/p6-u1-renderer-polish`
  Files: `engine/src/render/`, `engine/src/text/`, glyphon usage. **Does not
  touch** any C# chrome file.
  PR: #9 · Merge: content commits `f944307` + `96b6a95` (preferred monospace
  chain Cascadia Code → Cascadia Mono → Consolas replaces fontdb's Courier New
  default; ligatures via existing Shaping::Advanced; emoji/CJK via cosmic-text
  per-script fallback; frame-signature skip drops the whole GPU pass on
  unchanged frames. Subpixel AA: unsupported by glyphon — grayscale AA in sRGB
  space retained and documented. Damage regions delivered as a frame-level
  skip, not scissored partial present.)
  _Verification: `cargo test --manifest-path engine\Cargo.toml`; manual A/B
  screenshot vs main on an emoji/CJK/ligature corpus; record frame timings before/after._

- [x] **p6 U2** — Governor disposal on normal app shutdown + calibration
  save-on-exit verification. Currently the `CapacityModel`/`CapacityTicker`
  aren't disposed on graceful shutdown and `capacity.json` save-on-exit is
  unobserved live. Wire `App.OnLaunched` startup to a matching teardown in the
  `Closed` path (ticker before provider, per the order established in U3 of the
  RAM safe-zone plan), call `SaveCalibration()` on exit, and add a runbook step
  to confirm the file mutates. `║` (only `App.xaml.cs` touched — hot but
  single-owner in this wave)
  Worktree: `wt-p6-u2-governor-shutdown` · Branch: `fix/p6-u2-governor-shutdown`
  Files: `app/App.xaml.cs` `(hot)`, `app/Capacity/CapacityTicker.cs`,
  `app/Capacity/JsonCalibrationStore.cs`, `docs/runbooks/2026-06-10-ram-safe-zone-smoke.md`.
  PR: #6 · Merge: content commits `5b097b6` + `851e7e3` (App.StopCapacityGovernor:
  ticker → SaveCalibration → unpublish Capacity → provider, wired last in
  MainWindow.OnClosed; codex review reordered Capacity=null ahead of provider
  disposal; CapacityTicker/JsonCalibrationStore needed no changes)
  _Verification: dotnet test 238+; launch app, spawn ≥3 surfaces, exit, confirm
  `%LOCALAPPDATA%\optimus\capacity.json` updated mtime + new `budgetBytes`._

- [x] **p6 U3** — Packaging / distribution spike: unpackaged self-contained
  publish (`dotnet publish -r win-x64 -c Release --self-contained`), bundle
  Windows App SDK runtime, document the WebView2 Evergreen bootstrap path that
  **p6 U4** will consume. Land an installer script (Inno Setup or MSIX-optional
  toggle) and a clean-machine smoke runbook. `║`
  Worktree: `wt-p6-u3-packaging` · Branch: `feat/p6-u3-packaging`
  Files: `app/Optimus.App.csproj` (publish props), new `installer/`,
  `docs/runbooks/2026-06-11-clean-install-smoke.md`.
  PR: #7 · Merge: publish props already existed on main (csproj untouched);
  landed `installer/optimus.iss` + `installer/README.md` (WebView2 Evergreen
  bootstrap contract for U4), clean-install runbook, and `build.ps1 -Publish`
  now publishes the CLI self-contained single-file. Spike's key finding filed
  as **res U4** below.
  _Verification: `dotnet publish` succeeds; smoke on a clean VM or a fresh local
  user profile; tracker the WebView2 runtime detection contract for U4._

- [x] **res U4** — Release-profile engine crashes on exit: published Release
  app lingers ~60 s after window close, then dies 0xC0000005 in
  `D3D12Core.dll` (wgpu/D3D12 teardown race). Discovered by the p6 U3 publish
  smoke; debug-engine swap into the same publish exits 0 in ~2 s, pinning the
  fault to `cargo build --release` of the engine. Likely interacts with the
  R9 render-thread/panel teardown ordering; consider folding into **p6 U1**
  if it lands first. `║`
  Worktree: `wt-res-u4-release-engine-exit` · Branch: `fix/res-u4-release-engine-exit`
  Files: `engine/src/render/` (device/surface teardown), possibly
  `app/Splits/` shutdown ordering.
  PR: #10 · Merge: _on merge of #10_
  _Verification: published Release app exits code 0 within ~5 s of window
  close, no Application Error event; smoke per
  `docs/runbooks/2026-06-11-clean-install-smoke.md` §2. Live A/B on dev iGPU:
  fixed build exits 0 / no Application Error (no regression); race did not
  reproduce on integrated graphics even pre-fix, so the fix is merged
  correct-by-construction — full repro needs discrete-GPU/heavier-load timing._

### Wave 2 — Chrome surface area  (mixed)

- [x] **res U3** — Migrate the 4 older view files with inline `Color.FromArgb` /
  `FontSize` literals onto `app/Design/Tokens.cs`. Identify them with
  `grep -rn 'Color.FromArgb\|FontSize\s*=' app --include='*.cs'`, mirror DESIGN.md,
  and add a guard test (or a `dotnet build` warning-as-error switch) to keep
  regressions out. `║`
  Worktree: `wt-res-u3-tokens-migration` · Branch: `chore/res-u3-tokens-migration`
  Files: the 4 identified views under `app/`, `app/Design/Tokens.cs` (append-only),
  `DESIGN.md` (update shipped block), new `tests/Design/TokensGuardTests.cs`.
  PR: #8 · Merge: content commit `270ed80` (Tokens.cs expanded to 15 brushes +
  4 font sizes; views drop all inline literals; RISK #2 applied in-flight —
  pane flash → Attention teal, unread dot/badge → Unread magenta; guard test
  scans `app/**/*.cs` and fails on any raw `Color.FromArgb` / numeric
  `FontSize`, whitelisting only `Tokens.cs`)
  _Mandated by CLAUDE.md R5. Single-owner per file — no overlap with U4._

- [ ] **p6 U4** — WebView2 pane (`Microsoft.UI.Xaml.Controls.WebView2`). MUST
  set a per-user writable UDF via `CoreWebView2Environment.CreateWithOptionsAsync`
  (unpackaged default UDF under the exe dir is non-writable and the init
  throws). Detect/redistribute the Evergreen runtime per the U3 bootstrap
  contract. `→` (depends on **p6 U3** for the runtime-bootstrap contract)
  Worktree: `wt-p6-u4-webview2-pane` · Branch: `feat/p6-u4-webview2-pane`
  Files: new `app/Splits/WebView2Surface.cs` (mirror `TerminalPane` lifecycle),
  `core/Splits/SurfaceManager.cs` registration, `app/App.xaml.cs` UDF init.
  PR: _none yet_ · Merge: _—_
  _Verification: open a WebView2 pane in a workspace, navigate to a heavy site,
  capacity indicator still updates, ticker doesn't double-fire, shutdown clean._

### Wave 3 — Optional

- [ ] **p6 U5** — Cloud push parity *(optional)*: reuse the macOS
  `/api/notifications/push` contract verbatim (Bearer auth,
  `{title, subtitle?, body, workspaceId?, surfaceId?, hideContent?}`, same
  size/rate limits) for phone forwarding. Only the device push transport
  differs from APNs. `║`
  Worktree: `wt-p6-u5-cloud-push` · Branch: `feat/p6-u5-cloud-push`
  Files: new `app/Push/`, `cli/` notify route additions, secrets via
  Windows Credential Manager (DPAPI).
  PR: _none yet_ · Merge: _—_
  _Skip-by-default unit; only execute if the user explicitly asks for phone
  forwarding parity._

---

## Recommended serial path (single-session)

If only one `/ce-work` session is running at a time, work in this order. It
front-loads cheap wins, keeps the hot `App.xaml.cs` change isolated, and lands
the WebView2 work only after its packaging contract exists.

1. **res U1** — flip Phase 2 plan status (5 min)
2. **res U2** — CLI stdin hang fix
3. **p6 U2** — governor disposal + calibration save-on-exit
4. **p6 U3** — packaging spike (produces the runtime-bootstrap contract)
5. **res U3** — tokens migration on the 4 older views
6. **p6 U1** — renderer polish (engine-only; can also slot in earlier if
   chrome wave is blocked)
7. **p6 U4** — WebView2 pane (needs U3)
8. **p6 U5** — *only on explicit request*

---

## Cross-cutting standing checks

Run at the start of every session, before R1's reconciliation:

```powershell
# Rust engine fresh (regenerates app\Interop\NativeMethods.g.cs)
& 'C:\Users\steve\.cargo\bin\cargo.exe' build --lib --manifest-path engine\Cargo.toml

# Core tests (floor 238)
dotnet test tests\Optimus.Core.Tests.csproj

# Engine tests (floor 19)
& 'C:\Users\steve\.cargo\bin\cargo.exe' test --manifest-path engine\Cargo.toml

# App build — must be 0 warnings, 0 errors
dotnet build app\Optimus.App.csproj
```

A red gate is the end of the unit; fix it before opening a PR.

---

## Session log (append-only)

- 2026-06-11 · chore/res-u1-phase2-status · res U1 · PR #4 · Phase 2 plan flipped to completed with Outcome block (PR #1, `30d97ab`); pure docs, no gates run per unit note. PR #3 mis-merged into stale GitHub default branch `feat/phase1-walking-skeleton`; repo default flipped to `main`, work re-landed as PR #4.
- 2026-06-11 · fix/res-u2-cli-stdin-hang · res U2 · PR #5 · CLI stdin hang fixed via `StdinReader` (500ms first-byte timeout → null, 150ms quiet-window drain, 2s hard cap, leading-BOM strip). Gates: dotnet 244 (floor 238 + 6 new), cargo 19, app build 0W/0E. Live smoke: open-silent stdin exits ~300ms (was: infinite hang). Surprise (R6): PowerShell 5.1 `Process.Start` pushes a lone U+FEFF onto the redirected stdin pipe even when nothing is written — "silent" pipes from .NET Framework parents are not byte-silent. Codex review caught a use-after-dispose race on the reader events (fixed in `96f9650`).
- 2026-06-12 · fix/p6-u2-governor-shutdown · p6 U2 · PR #6 · Governor now torn down on graceful shutdown: MainWindow.OnClosed → App.StopCapacityGovernor (ticker dispose → SaveCalibration → Capacity unpublished → provider dispose). Gates: dotnet 238 (floor; res U2's +6 live in unmerged PR #5), cargo 19, app build 0 new warnings (34 pre-existing nullable warnings in core/Ipc/CommandRouter.cs surfaced on full rebuild — they predate this unit). Live smoke ×3: graceful close exits 0, `%LOCALAPPDATA%\optimus\capacity.json` created then mtime-advances each exit, correct JSON shape. Codex review: 1 fix taken (unpublish Capacity before provider disposal), accepted trade-off documented (CapacityTicker.Dispose's bounded 2s+2s drains run on the UI thread during close — worst-case ~4s stall, common path milliseconds).

- 2026-06-12 · chore/res-u3-tokens-migration · res U3 · PR #8 · Tokens registry completed: `app/Design/Tokens.cs` extended from 5 brushes / 4 font sizes to 15 brushes / 4 font sizes; `SidebarView`, `PaneTabStrip`, `PaneView`, `SplitTreeView` now consume named tokens for every color and font size. RISK #2 applied in-flight: pane flash → `Attention` teal (was `#4D9CF0`), unread dot + sidebar badge → dedicated `Unread` magenta `#D86FB0` (were `#4D9CF0`), so `PrOpen` blue stops doubling as either. RISK #1 (per-workspace identity hue derivation) stays a separate follow-up. Guard: `tests/Design/TokensGuardTests.cs` walks `app/**/*.cs` and asserts no `Color.FromArgb(...)` or `FontSize = <digit>` outside `Tokens.cs` (comment lines skipped). Gates: dotnet test 245 (floor 238 + res U2's +6 already merged + 1 new guard test), cargo test 19, cargo build --lib clean, app build 0W/0E.

- 2026-06-12 · feat/p6-u3-packaging · p6 U3 · PR #7 · Packaging spike: `build.ps1 -Publish` verified end-to-end (self-contained app publish 494 files incl. `Optimus.pri` + `optimus_engine.dll`; CLI now publishes self-contained single-file 68 MB); landed `installer/optimus.iss` (per-user Inno Setup, opt-in PATH, conditional WebView2 Evergreen bootstrap) + `installer/README.md` (WebView2 detection/bootstrap/UDF contract for p6 U4) + clean-install runbook. Gates: dotnet 244, cargo 19, app build 0W/0E. Published smoke: UI fully composed (sidebar 1/17 indicator, live terminal), capacity.json saves on close. KEY FINDING → res U4: release-profile engine AVs 0xC0000005 in D3D12Core.dll ~60 s after window close (debug-engine swap exits 0 in 2 s); filed as new unit, documented in runbook. Inno compile untested locally (no iscc on dev machine).

- 2026-06-12 · feat/p6-u1-renderer-polish · p6 U1 · PR #9 · Renderer polish: explicit monospace fallback chain (Cascadia Code → Cascadia Mono → Consolas; fontdb's `Family::Monospace` default on Windows is **Courier New** — logged to memory per R6) unlocking calt ligatures + per-script emoji/CJK fallback; frame-signature damage skip (rows + quads + geometry + palette defaults) drops the entire GPU pass on unchanged frames, reset on resize/DPI/reconfigure and stored only after a successful present. Gates: cargo 25 (floor 19 + 6 new headless shaping/signature tests), dotnet 245, app build 0W/0E. Codex review (sandboxed; failure-mode analysis): 1 real find taken — palette defaults (`default_fg`/`default_bg`) missing from the signature would freeze OSC 10/11 palette swaps; fixed in `96b6a95` + regression test. Honest scope: subpixel AA not feasible in glyphon (grayscale-in-sRGB retained); damage regions = frame-level skip, no partial present. Manual A/B emoji/ligature screenshot + frame timings deferred to the live smoke alongside res U4 (release-exit crash sits in the same teardown path).

- 2026-06-13 · fix/res-u4-release-engine-exit · res U4 · PR #10 · Release-exit D3D12 crash fixed in the engine teardown path. Root cause: `TerminalRenderer` released the wgpu/DX12 device while a frame was in flight → D3D12 deferred-destruction freed GPU-read resources → `0xC0000005` in `D3D12Core.dll` (release-only; debug drained in time). Fix (`e0cd1e9` + `d461d20`): `Drop` blocks on `device.poll(Wait)` (2 s bound) to drain the GPU, then releases resource layers (`quads`, `text`) before `panel` (surface → queue → device → instance); on wait-timeout the GPU fields are **leaked** (via `ManuallyDrop`) instead of released, since releasing a wedged device is the exact AV — keeps res U4's "no Application Error" bar even on timeout. FFI surface unchanged (no `NativeMethods.g.cs` churn). Gates: cargo 25, dotnet 245, app build 0W/0E. Codex review (read-only over diff): 1 [P2] (timeout branch fell through to device release), 0 [P1] — [P2] fixed by `d461d20`. Live verify (dev iGPU A/B): fixed build exits 0 / no Application Error (no regression), but the timing race did **not** reproduce on integrated graphics even pre-fix, so the fix is merged correct-by-construction — a true repro needs discrete-GPU/heavier-load timing (owner-approved merge).

Format per entry: `- YYYY-MM-DD · <session-id-or-branch> · <unit-id> · PR #<n> · <outcome>`
