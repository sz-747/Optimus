# Tauri Migration — Technical Implementation Plan

> Step-by-step plan for the WinUI 3 (C#) → Tauri (Rust backend + web frontend) migration.
> **Hard requirements: (1) nothing crashes after migration; (2) the app launches as a
> desktop GUI app (icon, window, taskbar — no console).**
> Derived from a 5-agent codebase recon (2026-07-02): engine/, core/, IPC+CLI, app/ chrome,
> build+tests+installer. Supersedes parts of `tauri-switch-scope.md` — see Decision D1′ below.

---

## 0. Decisions locked by this plan

### D1′ — Web terminals (xterm.js), NOT the wgpu renderer. Supersedes scope-doc D1.

`tauri-switch-scope.md` D1 said "keep the GPU renderer" — that decision was **overturned**
by STRATEGY.md (2026-07-01): GPU renderer is in "Not working on"; terminals render in the
webview. This plan implements the STRATEGY decision. Consequences:

- **Deleted crash classes:** the D3D12 teardown AV (drain-GPU + leak-on-timeout fix in
  `render/terminal.rs:89-129`), swapchain reconfigure races, `apply_composition_transform`
  DPI math, the entire DComp airspace problem, and the `wgpu =29.0.1` exact pin. The single
  hardest, highest-risk item in the old scope (XL airspace work) is gone.
- **Deleted code:** `engine/src/render/` (mod.rs, terminal.rs, text.rs), `panel_ffi.rs`,
  wgpu + glyphon + pollster deps, wezterm-term/termwiz VT state (xterm.js parses VT
  frontend-side), `pollster` UI-thread `block_on`.
- **Kept in engine:** ConPTY (`conpty.rs`), the OSC-99 sniffer (`vt.rs:148-324` — it feeds
  on raw bytes, renderer-independent), PTY reader thread, child PID/handle publication.
- **Input simplifies:** xterm.js owns keyboard/mouse/selection/scrollback. The
  VK-code bridge (`TerminalPane.xaml.cs:287-386`, `ShortcutRouter`) is replaced by
  xterm.js `onData` → `send_text`. Chrome-level shortcuts re-key VK → `KeyboardEvent.code`.

### D2 — One process. Tauri main process hosts: engine (in-proc lib), all domain logic,
named-pipe server, job objects. WebView2 renders the frontend. CLI stays a separate
sidecar exe. This collapses today's topology (C# app + Rust cdylib over FFI) into one
Rust binary — the FFI boundary (16 `#[no_mangle]` fns, panic guard, thread-local
LAST_ERROR, ByteBuffer, csbindgen, `NativeMethods.g.cs`) is deleted, not re-bridged.

### D3 — Port order is dependency order, tests move first. The C# test corpus
(226 tests / 24 files) is the regression net; each subsystem's tests port to Rust
*before or with* the subsystem. The capacity tests (`CapacityModelTests.cs` 19 +
`SurfaceManagerCapacityTests.cs` 6) are the crown-jewel gate — the port is wrong until
they pass in Rust.

---

## 1. Phase map

| Phase | What | Exit gate |
|---|---|---|
| P0 | Scaffold + workspace restructure | `cargo tauri dev` opens an empty GUI window |
| P1 | Engine slim-down (delete FFI + renderer, keep ConPTY/OSC-99) | engine tests green; PTY bytes reach a stub frontend |
| P2 | Core domain port C#→Rust (capacity first) | all ported unit tests green, counts match C# |
| P3 | IPC + job objects + CLI | CLI round-trip against new pipe server; hook snippets byte-identical |
| P4 | Frontend build-out | full chrome functional; keyed pane reuse verified |
| P5 | Crash-safety hardening | crash-drill matrix passes; clean exit code 0 |
| P6 | Packaging + launch-as-GUI + CI | double-click install → Start-menu app, no console window |
| P7 | Cutover + deletion | .NET gone from repo; old app builds removed |

Phases P2 and P4 can run in parallel worktrees after P1 (backend port vs frontend build
touch disjoint files). P3 depends on P2 (router dispatches into domain). P5 gates release.

---

## 2. Phase 0 — Scaffold + workspace restructure

1. Create cargo workspace at repo root: members `engine/` (existing crate) + `shell/`
   (new Tauri app crate, `cargo tauri init`) + later `core-rs/` if kept separate
   (recommendation: fold domain into `shell/src/domain/` — one crate, fewer seams).
2. Tauri config, Windows-only:
   - `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` in `main.rs` —
     **this is the no-console-window requirement.** Debug builds keep the console.
   - `tauri-plugin-single-instance` — today's app has no single-instance check
     (`App.xaml.cs:69` recon); pipe name `optimus-stable` collides if two instances run.
     Second launch focuses the first window.
   - WebView2: fixed user-data-folder under `%LOCALAPPDATA%\optimus\webview2` (known
     lesson: unpackaged WebView2 UDF is non-writable by default; never memoize a faulted
     environment). Tauri exposes this via `tauri.conf.json` > `dataDirectory` /
     `WEBVIEW2_USER_DATA_FOLDER`.
   - App icon (`icons/` via `tauri icon`), product name Optimus, version from one source.
3. Frontend scaffold: plain HTML/CSS/JS (no framework — tokens.css + existing prototypes
   are vanilla; adding React is unforced weight). Vite for dev server/HMR.
   `tokens.css` (105 lines) copied in as canonical; add the stylelint raw-hex ban that
   replaces `TokensGuardTests.cs`.

Exit gate: `cargo tauri dev` shows an empty window with tokens.css background; release
build shows no console.

## 3. Phase 1 — Engine slim-down

Work entirely inside `engine/`:

1. **Delete FFI layer:** `lib.rs` 16 extern fns, `ffi/mod.rs` (guard, LAST_ERROR),
   `ffi/events.rs` (ByteBuffer), `render/panel_ffi.rs`, csbindgen build-dep + build.rs
   codegen. Public API becomes ordinary Rust: `Engine::new(opts) -> Result<Engine>`,
   methods return `Result<_, EngineError>` (replaces thread-local last-error).
2. **Delete render stack:** `render/` module, wgpu/glyphon/pollster deps. Crate type
   `["cdylib","rlib"]` → `["rlib"]`.
3. **Keep + rewire the command bus:** the mpsc thread (`engine.rs:119-131`) shrinks —
   no render arms, no swapchain attach. Decision: keep one worker thread per engine
   owning ConPTY (write side) OR go fully channel-less with a `Mutex<ConPty>`. Keep the
   thread — it preserves today's proven ordering (single owner of PTY lifetime) and the
   sync-reply plumbing shrinks to `spawn_shell`/`resize` only.
4. **PTY byte path:** reader thread (`engine.rs:670-694`) stays; instead of feeding
   wezterm-term, bytes go to (a) the OSC-99 sniffer (`vt.rs` unchanged — it has 8 tests)
   and (b) a `tauri::ipc::Channel<Vec<u8>>` per surface for the frontend (**Channel, not
   `AppHandle::emit`** — PTY output is high-volume; Channel has backpressure; emit
   broadcasts JSON to all listeners).
5. **Title/bell:** wezterm-term alert bridge (`vt.rs:60-81`) dies with wezterm-term;
   xterm.js reports OSC 0/2 title + BEL via its own parser hooks → frontend → backend
   `report_title` command. OSC-99 stays Rust-side (agents emit it from arbitrary child
   processes; sniffing raw bytes in Rust is authoritative regardless of what the frontend
   does).
6. **Preserve teardown order (crash-critical, from recon):**
   - `ConPty::shutdown()` (ClosePseudoConsole, idempotent flag `conpty.rs:249-253`)
     **before** reader-thread join (EOF unblocks the blocking ReadFile) — else deadlock.
   - Writer drop before pty drop; reader join before pty drop (handle close order,
     `engine.rs:890-902` ordering carries over verbatim).
7. **Panic policy:** per-surface worker + reader threads wrap their loops in
   `catch_unwind` (port of `handle_guarded`, `engine.rs:502`) → a panicking surface dies
   and reports an event; the process survives. No `unwrap`/`expect` outside startup
   (recon found only 2 non-init expects, both in deleted render code — keep it that way;
   add `#![warn(clippy::unwrap_used)]` on the crate).

Exit gate: `cargo test -p optimus-engine` green (ConPTY + OSC-99 + lifecycle tests;
spike5_ffi.rs deleted with FFI); a dev command spawns a shell and streams bytes to
devtools console.

## 4. Phase 2 — Core domain port (C# → Rust)

Port in dependency order (from recon dependency graph). Each unit = types + logic +
its C# tests translated to `#[cfg(test)]`.

| # | Unit | Source | Tests to port | Notes |
|---|------|--------|--------------|-------|
| 1 | Ids + IdAllocator | `core/Splits/Ids.cs` | (covered in controller tests) | 3 monotonic counters, never reused per session |
| 2 | LayoutGeometry | `core/Splits/LayoutGeometry.cs` | 9 | pure fns; overlap-penalty nav constant 1,000,000 |
| 3 | SplitTree + Controller | `SplitTree.cs`, `SplitTreeController.cs` | 19 | immutable tree (`Arc<SplitNode>` + rewrite), never-empty invariant, version counter |
| 4 | Workspace + Manager | `core/Sidebar/*` | 17 + 8 projection | never-empty, shared allocator ⇒ globally-unique SurfaceId |
| 5 | Notifications | `core/Notifications/*` | 30 | queue coalescing/generation, MaxPerDrain=16, policy context re-derived at drain (KTD6), store reindex |
| 6 | **CapacityModel** | `core/Capacity/*` | **19 + 6** | see below |
| 7 | SurfaceManager | `core/Splits/SurfaceManager.cs` | 7 + 6 capacity-gate | TryReserve→spawn→Commit two-phase; idempotent per SurfaceId |
| 8 | Projections + ShortcutMap | `SidebarProjection`, `TabHeaderProjection`, `ShortcutMap` | 8 + 12 | ShortcutMap re-keys VK → `KeyboardEvent.code` (test table re-derived, semantics identical) |

**CapacityModel port contract (crown jewel — port exactly, no "improvements"):**

- Constants verbatim: `OsReserveBytes = 2 GiB`, `SeedBudgetBytes = 200 MiB`,
  `CalibrationMinSamples = 3`, `RecoveryHeadroomFactor = 1.25`,
  `RecoveryTicksRequired = 2`, Warn ≥ 75%, Cap at 100%.
- Concurrency shape verbatim: one `Mutex` mirroring `_gate`; **events raised outside the
  lock** (C# does this deliberately at lines 105/123/139/162/192/246 — raising inside a
  Rust lock re-creates the deadlock the C# design avoids). LowMemorySignal from the OS
  thread only sets `low_signal_pending`; OnTick (1 Hz timer) reads+clears.
- Invariants the ported tests must pin: cap monotonically non-increasing under pressure;
  relaxation only after 2 consecutive recovered ticks at ≥1.25× headroom with low-signal
  clear; TOCTOU reserve gate (only first reserve wins at cap−1); live terminals never
  reaped on shrink; P75 nearest-rank (= max at 3 samples); calibration fingerprint-gated.
- Provider trait ports `ICapacityProvider`; Win32 impl (GlobalMemoryStatusEx,
  GetPerformanceInfo, QueryMemoryResourceNotification) moves from `app/Capacity/` into
  the shell crate via the `windows` crate. Calibration store: JSON file, same path,
  same schema (existing user calibrations must load).

Exit gate: ported test counts reconciled against C# (≈180 of 226; CLI's 43 move in P3;
TokensGuard's 1 became stylelint). Zero test-semantics changes without a written note.

## 5. Phase 3 — IPC, job objects, CLI

1. **Named-pipe server (Rust):** raw `CreateNamedPipeW` + `SECURITY_ATTRIBUTES` via
   `windows` crate — **not** tokio's named-pipe API (it doesn't expose the owner-SID
   ACL; today's server sets owner + protected DACL + FullControl-current-user,
   `PipeServer.cs:245-258`). Port: accept loop with 16-connection semaphore, per-client
   thread (blocking I/O is fine at 16 conns; no tokio needed), peer-SID resolution
   (`GetNamedPipeClientProcessId` → token → SID, `PeerIdentity.cs:19-27`), auth modes
   (OptimusOnly SID match / Password / AllowAll / Off), DPAPI password store
   (`CryptProtectData`, same `%LOCALAPPDATA%\optimus\optimus-socket-password.bin`,
   constant-time SHA-256 compare).
2. **V2 protocol + CommandRouter:** newline-delimited JSON, `{id, method, params}` /
   `{id, ok, result|error}` — serde structs. Port the d30009a parse-boundary hardening
   as type-level: params deserialize into typed structs, absent/mistyped → `invalid_params`,
   malformed JSON → `parse_error`. Keep V1 text fallback only if the 43 CLI tests still
   exercise it; otherwise delete (scope doc: nothing live speaks V1).
   Same pipe name scheme (`optimus-stable|nightly|staging|dev`, `OPTIMUS_SOCKET_PATH`
   override) — **external agents' pipe contract must not change.**
3. **Job objects fold into engine crate** (it owns the child handle — deletes today's
   C#-side duplicate-handle hop): after `spawn_shell`, engine creates anonymous job
   (`JOB_OBJECT_LIMIT_PROCESS_MEMORY` at 2× budget + `KILL_ON_JOB_CLOSE`) and assigns the
   child. Ownership now unambiguous: the single Tauri process owns every job handle;
   process death ⇒ OS closes handles ⇒ KILL_ON_JOB_CLOSE reaps all shells. Job creation
   failure stays best-effort (log, run unbackstopped — today's behavior).
4. **Teardown order** (mirrors `TerminalPane.xaml.cs:134-152`): engine surface shutdown
   (ClosePseudoConsole → orderly child exit) **then** job dispose (reaps stragglers).
   Never job-first (would hard-kill shells that were exiting cleanly).
5. **CLI rewrite (clap), Tauri sidecar:** argv → V2 frames, pipe discovery/probe
   (250 ms probe, 2 s connect — same timeouts), hooks command for claude/codex/gemini/
   cursor/copilot × session events. **Hook .ps1/.cmd snippets must regenerate
   byte-identical** (agents already have them installed) — port `HooksCommandTests`
   golden-file style. 43 CLI tests port here.

Exit gate: old CLI's test vectors pass against new server; `optimus hooks claude stop`
round-trips end-to-end from a shell spawned inside a migrated pane.

## 6. Phase 4 — Frontend build-out

All state authoritative in Rust; frontend renders snapshots + sends intents (same
philosophy as today's dumb views).

1. **Terminal panes: xterm.js** (+ `@xterm/addon-webgl` for perf, canvas fallback;
   `@xterm/addon-fit`). Wire: backend Channel → `term.write(bytes)`; `term.onData` →
   `surface_send_text` command. Scrollback 10k (today's engine default).
   - **Pane-visibility manager (renderer virtualization — required, not optional; soak verdict 2026-07-02).**
     Chromium/WebView2 hard-caps live WebGL contexts at ~16/page and silently evicts the oldest →
     panes blank once agent count exceeds the cap. Rule: **many live terminals, few live renderers.**
     A frontend visibility manager attaches `@xterm/addon-webgl` only to on-screen panes (≤ ~12);
     off-screen panes detach the WebGL addon and sit as paused headless buffers (PTY bytes keep
     accumulating in the backend Surface, replayed on reveal), so no terminal loses history. It
     reacts to the layout/viewport changes the split-tree + sidebar already emit (no new event bus);
     it is invisible plumbing, not a UI surface — distinct from the control plane (R6). WebGL renderer
     contexts are treated as a **governed capacity resource inside the safe-zone governor** (see TRD
     §C) — one more scarce thing the capacity model budgets, alongside RAM/slots. Cap scrollback and
     batch writes to bound per-pane memory (~34 MB/pane at 5k scrollback).
2. **Keyed pane reuse (crash/UX-critical, recon: `SplitTreeView.cs:25`, `PaneView.cs:26`):**
   pane DOM nodes keyed by SurfaceId, **moved** (`element.append` re-parent) on relayout,
   never destroyed/recreated — destroying an xterm.js instance on split-drag would drop
   scrollback + selection. Port of today's cached-pane re-parenting. The
   `SurfaceLifecycleGuard` (WinUI Loaded/Unloaded double-fire) has no DOM analogue —
   dies, but attach-once/shutdown-once idempotence moves into the Rust SurfaceManager
   (it already has it: reserve/commit/release idempotent per id).
3. **Split view:** CSS grid + pointer-event dividers; divider fraction round-trips to
   `SetDividerPosition` (clamped backend-side, [0,1]). TreeSnapshot arrives as a Tauri
   event; frontend reconciles (version counter prevents stale renders).
4. **Sidebar:** port `manager.html` (749-line prototype) to live data from
   SidebarProjection DTOs. **Capacity indicator:** shell.js `bd-cap` 8-pip meter +
   SAFE-ZONE chip, bound to CapacityState events (Calm/Warn/Cap → tokens.css levels).
   Always visible (CLAUDE.md thesis requirement).
5. **Command palette:** shell.js V7 Spotlight scaffold → real command registry.
   Chrome shortcuts: one keydown listener at document level using `KeyboardEvent.code`
   chords from ported ShortcutMap; terminal-focused keys pass through to xterm.js
   untouched except the reserved chord table (today's semantics).
6. **Toasts:** `tauri-plugin-notification`; NotificationCoordinator.Surfaced event →
   OS toast; click → focus surface (routes through existing click-action model);
   fail-open if OS denies notification permission (today's behavior).
7. Design rules: tokens only (stylelint gate), DESIGN.md read before any chrome work.

Exit gate: spawn/split/close/tab-cycle/zoom all functional; drag-split with a running
`ping -t` visibly preserves terminal content (keyed-reuse proof); capacity meter moves
when spawning to cap; **pane-visibility manager holds past the ~16 WebGL-context wall —
N×20-pane soak (`scratchpad/xterm-soak.html`, ramped in WebView2 not just Chrome) sustains
≥30 FPS with stable heap, off-screen panes detach renderers and replay intact on reveal.**

## 7. Phase 5 — Crash-safety hardening (the "nothing crashes" gate)

Invariant checklist (each gets a drill or test):

| # | Invariant | Mechanism | Verify |
|---|-----------|-----------|--------|
| C1 | Clean window close | On `WindowEvent::CloseRequested`: stop pipe server → SurfaceManager.DisposeAll (per-surface: ConPTY shutdown → reader join → job dispose) → capacity stop → exit. Mirrors `MainWindow.OnClosed` order | close with 10 live shells; exit code 0; no WER dump; no orphan conhost/shell in Task Manager |
| C2 | No PTY teardown deadlock | ClosePseudoConsole **before** reader join (EOF releases blocking read) | drill: close pane while `type` streaming a huge file |
| C3 | Child reaping | engine Dispose then job dispose; KILL_ON_JOB_CLOSE backstop | kill Optimus process (Task Manager) → all child shells die |
| C4 | Panic containment | catch_unwind per surface worker/reader; Tauri commands all return `Result`; panic hook logs to `%LOCALAPPDATA%\optimus\logs` | inject `panic!` behind a debug command → pane shows "surface died", app alive |
| C5 | Pipe robustness | malformed frame → `parse_error` reply, connection survives; client crash mid-frame → slot released (semaphore in drop guard) | fuzz newline garbage at the pipe; connect/kill 100 clients |
| C6 | Capacity under churn | TOCTOU reserve gate; spawn storm at cap; shrink with live terminals | ported tests + manual spawn-spam at cap |
| C7 | WebView2 env | explicit UDF dir; creation failure → error dialog + clean exit (never memoize faulted env, never blank-window zombie) | point UDF at read-only dir → graceful error |
| C8 | Renderer crash class | **deleted** — no wgpu/D3D12 in the process. WebView2 GPU crashes are Chromium's problem; xterm.js webgl addon falls back to canvas on context loss | verify fallback: force-lose WebGL context in devtools |
| C9 | DPI | no SetMatrixTransform math left; WebView2 handles per-monitor DPI | move window 100%↔150%↔200% monitors while shells run |
| C10 | Sleep/resume | ConPTY + pipe survive power transition | sleep 5 min with live shells, resume, type |

Also: `tauri-plugin-window-state` (restore size/pos), panic hook + single log file
(port of `install_render_panic_logger`), and a `--safe-mode` flag (skip session restore)
as the escape hatch.

Exit gate: full drill matrix passes on a clean Windows 11 VM **and** a 150%/200% DPI
multi-monitor box.

## 8. Phase 6 — Packaging, launch-as-GUI, CI

1. **GUI launch requirements** (the "shows as a desktop app" ask): release binary built
   with `windows_subsystem = "windows"` (no console flash); app icon embedded; Start-menu
   shortcut + optional desktop shortcut from installer; taskbar identity via stable
   AppUserModelID (also makes toasts attribute correctly); proper `productName`/version
   in tauri.conf so Task Manager/Add-Remove show "Optimus".
2. **Installer:** Tauri bundler NSIS, per-user (`perMachine: false` — matches today's
   lowest-privilege Inno posture, `{LOCALAPPDATA}\Programs\Optimus`), WebView2 Evergreen
   bootstrap via bundler's `webviewInstallMode: downloadBootstrapper` (replaces the
   hand-rolled EdgeUpdate registry detection in `optimus.iss`). CLI ships alongside as
   sidecar + opt-in PATH. Retire `installer/optimus.iss` after parity check (retain the
   UDF/unpackaged contract notes from `installer/README.md` into new docs).
3. **CI (none exists today — add it):** GitHub Actions windows-latest:
   `cargo fmt --check`, `clippy -D warnings`, `cargo test --workspace`, frontend
   stylelint + typecheck, `cargo tauri build`, upload installer artifact. Gate merges.
4. `build/build.ps1` → thin wrapper over `cargo tauri build` (or delete).

Exit gate: fresh Windows VM, double-click installer → Start menu → launches GUI window,
no console, toast works, uninstall clean.

## 9. Phase 7 — Cutover + deletion

1. Side-by-side period: keep `app/` (C#) building on main until P5 drill matrix passes.
2. Then delete in one PR: `app/`, `tests/` (C# — Rust ports are the survivors),
   `spikes/spike1_swapchainpanel/`, `installer/optimus.iss`, all csproj, csbindgen
   build step, `NativeMethods.g.cs`, `worktrees/wt-p6-u4-webview2-pane`.
3. Update: CLAUDE.md (stack description), DESIGN.md pointers, `tauri-switch-scope.md`
   (mark superseded by this plan, esp. D1), README build instructions.

---

## 10. Risk register

| Risk | Sev | Mitigation |
|------|-----|-----------|
| Capacity port drift (silent safe-zone break) | **High** | test-first port, constants table above, no refactors during port; calibration file format unchanged |
| xterm.js perf with many panes | Med | webgl addon **only on ≤~12 on-screen panes (pane-visibility manager, P4 item 1)** — the ~16 WebGL-context cap is the hard wall, not raw FPS; off-screen = paused headless buffers; virtual write batching (Channel chunks); N×20 pane soak in P4 exit gate run in WebView2; this replaced the *harder* airspace risk |
| Named-pipe ACL parity (agents can't connect) | Med | port SID logic exactly; integration test with real second-process client in CI |
| Hook snippet drift breaks installed agents | Med | golden-file tests, byte-identical requirement |
| Event ordering differences (C# events → Tauri events) | Med | keep "raise outside lock" rule; frontend reconciles by snapshot version, never by event order |
| WebView2 runtime absent on target | Low | bundler downloadBootstrapper + C7 graceful-fail |
| wezterm-term deletion loses an escape-sequence behavior users rely on | Low | xterm.js is the most battle-tested VT impl in existence; OSC-99 (our custom one) stays Rust-side |

## 11. What this plan deliberately does NOT do

- No cross-platform work (D2 stands: Windows-only).
- No new features during migration — control plane (R6), memory layer (R2) build *after*
  cutover, on the Tauri base. Migration PRs that add features get bounced.
- No renderer work — D1′ is final unless xterm.js measurably fails the many-terminals
  soak, in which case the recorded fallback is one wgpu surface in a child HWND
  (the old D1), resurrected from git history, not carried as live code.
