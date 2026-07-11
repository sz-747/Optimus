# Tauri Switch — Scope Decision

> **Status: scope locked (2026-07-01); D1 revised 2026-07-02.** What's in / out of the pivot
> from WinUI 3 (C#) chrome to a Tauri (Rust backend + web frontend) desktop app, Windows-only.
> Derived from a 6-subsystem audit (`tauri-switch-scope-audit` workflow). Motive for the pivot:
> easier to position and sell — *not* a perf complaint. **D1 now renders terminals with
> xterm.js in the webview (D1′), superseding the earlier keep-the-GPU-renderer decision** — see
> `docs/design/tauri-migration-plan.md`. The RAM safe-zone differentiator is untouched; the
> many-terminals perf story now rides on xterm.js (soak-tested separately), with the wgpu stack
> parked in git history as the fallback.

## Locked decisions

| # | Decision | Consequence |
|---|----------|-------------|
| D1′ | **Web terminals (xterm.js).** Terminals render in the webview via xterm.js (WebGL addon, canvas fallback); the wgpu grid is dropped from the ship path and parked in git history. Uses Tauri's stock WRY window — no self-hosted WebView2, no native GPU child surface. | **Deletes the airspace/z-order/DPI class of work entirely** (was the pivot's highest risk). New, lower risk: many-panes xterm.js perf (soak-test gates it; fallback = resurrect wgpu-in-child-HWND from git). Supersedes the old D1 keep-GPU decision. |
| D2 | **Windows-only.** No cross-platform target. | Segoe/Cascadia system-font stacks stay valid (no webfonts); Win32 governor (memory + job objects) ports straight to the `windows` crate. No provider abstraction tax. |
| D3 | **Backend is Rust, in-process.** All C# domain logic ports to Rust; the C-ABI FFI layer is deleted, not re-bridged. | Engine, capacity, splits, notifications, IPC all become one Rust backend. No .NET runtime shipped. |
| D4 | **Design system carries over as-is.** `docs/design/web-overlay-palette/tokens.css` becomes canonical; `Tokens.cs` dropped. | Graphite needs no re-derivation. Biggest single head-start of the pivot. |

## KEEP → Rust backend (port, mostly verbatim)

| Area | Notes | Effort · Risk |
|------|-------|:---:|
| Engine: ConPTY, VT (wezterm-term), OSC-99 sniffer | Already Rust, zero WinUI coupling. Called in-process, not over FFI. | S · low |
| Engine orchestration (mpsc command bus, PTY reader, key/selection/scrollback) | Drop the swapchain arm + sync-reply plumbing; keep the shape. | M · med |
| ~~wgpu draw stack: grid.rs, text.rs, terminal.rs~~ | **Dropped from ship path under D1′ (xterm.js renders instead); kept in git history as the fallback if the xterm.js soak test fails.** | — |
| **Capacity governor** (CapacityModel, P75 calibration, monotonic-cap hysteresis, reserve/commit ledger) | **Crown jewel. Port exactly + bring the C# test corpus.** Naive rewrite silently breaks the safe-zone guarantee. | L · **high** |
| Split-tree + controller, LayoutGeometry, IdAllocator | Authoritative state → Rust single source of truth. Frontend renders TreeSnapshot. | L · med |
| WorkspaceManager, Workspace metadata | Never-empty invariant, surface→workspace routing. | M · med |
| Notification queue / store / policy | Largest algorithmic subsystem after capacity; couples to tree snapshot. | L · med |
| IPC: V2 wire protocol, named-pipe server, peer-SID auth, password store | V2 is already language-agnostic JSON. Named-pipe stays an external listener (agents connect from arbitrary shells). | L · med |
| Win32: job objects (KILL_ON_JOB_CLOSE), memory measurement | → `windows` crate. **Job logic folds into the engine crate** (already owns the ConPTY child handle) — deletes an FFI hop. | M · med |
| CLI (argv→V2, agent hooks) | Rebuild in Rust (clap), ship as **Tauri sidecar**. Hook .ps1/.cmd snippets regenerate byte-identical. | L · med |
| Design tokens, native fonts, no-blur/value-step, scrim/overlay-card | tokens.css already the mirror. *More* natural on web. | S · low |

## DROP (deleted — no Tauri analogue)

- **All C-ABI FFI glue:** 15 `#[no_mangle] extern "C"` fns, csbindgen, `NativeMethods.g.cs`, `ByteBuffer` alloc/free, panic guard, thread-local last-error. Replaced by direct Rust calls + `Result` + Tauri events.
- **All WinUI view code + XAML** (SidebarView, PaneTabStrip, PaneView, SplitTreeView, WorkspaceView, MainWindow.xaml, App.xaml).
- **SwapChainPanel host:** `ISwapChainPanelNative.cs`, `TerminalPane.xaml`, `TerminalPaneSurfaceFactory.cs`.
- **`apply_composition_transform`** (IDXGISwapChain2 DPI cancel — SwapChainPanel-only; HWND swapchain presents 1:1). Also frees the `wgpu =29.0.1` exact pin.
- **`panel_ffi.rs`** Spike-1 scaffolding.
- **`SurfaceLifecycleGuard`** (tames a XAML Loaded/Unloaded double-fire that doesn't exist in the Tauri model), **GridSplitter star-ratio math** (web uses CSS fr).
- **V1 legacy text protocol** (nothing live speaks it; CLI emits only V2).
- **(D1′) The wgpu ship path + all airspace work:** the GPU terminal surface, self-hosted WebView2-in-DirectComposition, DComp visual tree / Z-order / clip, per-pane child HWNDs. xterm.js in the stock WRY window replaces it. The wgpu draw stack stays in git history only, as the soak-test fallback.
- **`Tokens.cs` + `TokensGuardTests.cs`** → replace guard with stylelint banning raw hex outside tokens.css.
- **The entire .NET runtime.**

## REBUILD as web frontend (HTML/CSS/JS)

| Surface | Head-start | Effort · Risk |
|---------|-----------|:---:|
| Command palette / switcher / settings / capacity dashboard | 7-variant tournament winner (V7 Spotlight) + shell.js already built | M · low |
| Sidebar (workspace rows) | `docs/design/workspace-manager/manager.html` (~749 lines) | M · low |
| Capacity indicator (always-visible) | shell.js `bd-cap` + 8-pip meter + SAFE-ZONE chip + slot-cost tags | S · low |
| Tab strip, pane chrome (focus border, teal flash) | Trivial CSS; identity hue = 2px left border | S–M · med |
| **Terminal panes (xterm.js)** | **New under D1′.** One xterm.js instance per pane, fed by the engine's PTY byte stream over `tauri::ipc::Channel`; WebGL addon with canvas fallback. Many-panes perf + WebGL-context limit is the open risk (soak-tested). | L · **med-high** |
| Split tree view (recursive + draggable dividers + zoom) | allotment / react-resizable-panels; **must preserve keyed pane reuse** (never tear down a live terminal surface on relayout) | M · med |
| Shortcut chord table | ShortcutMap logic ports; re-key VK_* → web `KeyboardEvent.code` | S–M · med |
| OS toasts | → tauri-plugin-notification; preserve click-routes-to-surface + fail-open | M · med |

Projection glue (TabHeaderDto, SidebarRowDto) can live frontend-side directly.

## The immediate next decision (before implementation): xterm.js many-panes soak

D1′ (web terminals) **dissolves the old airspace problem** — with no native GPU surface, web
chrome and terminals live in one webview DOM, so overlays/scrims/borders are ordinary CSS and
Tauri's stock WRY window suffices. The risk moves to one place: **does xterm.js stay smooth and
memory-sane with many panes open?** — the app's whole point is a lot of parallel agents.

The crux is the renderer. Each xterm.js WebGL instance owns its own **WebGL context**, and
Chromium — hence WebView2 — **hard-caps live contexts at ~16 per page**, silently evicting the
oldest (the exact failure Hyper hit at 16 panes). xterm.js is also main-thread-bound, so N
saturating panes divide one frame budget. **Verdict (researched 2026-07-02, sources below):
conditional yes — viable only with renderer virtualization.**

The rule: **many live terminals, few live renderers.** Never one WebGL context per pane.

- Attach the WebGL addon only to **on-screen panes (≤ ~12)**; keep offscreen panes as headless
  buffers (`term.write` into a paused/detached terminal, or buffer the PTY stream and replay on
  reveal). This is the VS Code playbook — it survives xterm.js only because it shows a handful of
  terminals at once; Optimus's many-panes thesis *forces* this virtualization from day one.
- Cap scrollback (buffers dominate memory — ~34 MB/pane at 5k scrollback → ~1 GB at 32 panes);
  batch PTY writes.
- Canvas addon is the fallback but is **deprecated (removed in xterm v6)** → the DOM renderer is
  the long-term floor.
- **Govern WebGL contexts as a capacity resource** — the safe-zone governor budgets renderer
  contexts alongside RAM/slots. This drops straight out of Optimus's existing differentiator: the
  capacity model already governs scarce resources, and a renderer context is just another one.

**Soak harness exists** — `scratchpad/xterm-soak.html` (driver: `scratchpad/run-soak.js`, Playwright +
system Chrome): ramps N panes streaming output, measures sustained FPS + heap, exposes the active
renderer + the context-eviction warning. Gate: ≥30 FPS + stable heap at target pane count.

**Measured run — 2026-07-02, Chrome on Intel Iris Xe (ANGLE/D3D11), ramp to N=64 @ 30 lines/s/pane:**
- **The ~16 WebGL-context cap is CONFIRMED and exact** — live WebGL renderers pinned at **16, hard**;
  first eviction at N=20 (pane #17 lost its context), every subsequent pane fell to canvas. At N=64:
  `webgl:16 canvas:48`. This is Chromium behavior → **reproduces in WebView2**; the cap is architectural.
- **FPS held far better than the earlier prediction** — sustained **50–60 FPS all the way to N=64**,
  never below 30, banner PASS throughout. The per-pane onContextLoss→canvas fallback absorbed the
  overflow with no visible break on this hardware. (The earlier "FPS<30 at 24–32, panes blank" guess
  is contradicted by measurement — it assumed *no* fallback wired.)
- **Heap** ~102 MB at N=64, ~1.0 MB/s trend (under the 1.5 MB/s leak gate) — but scrollback was 1000,
  not 5k, so this is optimistic vs the ~34 MB/pane-at-5k figure.

**Caveats:** Chrome not WebView2 (same engine, cap identical; FPS may differ) · one mid-tier iGPU
(weaker hardware falls harder) · moderate 30 lines/s (bursty 500+ lps untested) · the fallback that
saved the run is the **deprecated canvas addon (gone in xterm v6)** → long-term the fallback floor is
the slower DOM renderer, so renderer virtualization is not optional forever.

**Verdict:** the cap is real and must be designed around (→ pane-visibility manager, migration-plan
P4 item 1), but with a fallback wired it is not a day-one crisis — 64 panes ran smooth on a mid iGPU.
Still **run the WebView2 gate before P4 commit.** Fallback if virtualization can't hold the gate on
weak hardware / high throughput: resurrect wgpu-in-child-HWND from git (old D1).

Sources: [Chromium 16-context cap](https://issues.chromium.org/issues/40939743) · [Hyper 16-pane failure](https://github.com/vercel/hyper/issues/6584) · [xterm.js "dozens of terminals"](https://github.com/xtermjs/xterm.js/issues/4379) · [xterm.js main-thread fps math](https://github.com/xtermjs/xterm.js/issues/3368) · [xterm.js buffer memory](https://github.com/xtermjs/xterm.js/issues/791) · [canvas addon deprecation](https://www.npmjs.com/package/@xterm/addon-canvas) · [VS Code WebGL + DOM fallback](https://github.com/microsoft/vscode/pull/84440).

## Residual open questions (non-blocking)

- Input/clipboard: under D1′ xterm.js consumes `KeyboardEvent` directly and handles selection/clipboard in-DOM, so the old VK/physical-pixel bridge mostly dissolves — remaining work is routing app-level chords (not the DPI/CSS-pixel translation the GPU path needed).
- PTY output is high-volume → feed xterm.js over `tauri::ipc::Channel` (backpressure) not `AppHandle::emit`.
- Named-pipe owner-SID ACL: tokio's named-pipe API doesn't expose it → raw `CreateNamedPipeW` + `SECURITY_ATTRIBUTES` via `windows` crate.
- Job-object ownership under the new process topology — which process owns the job so KILL_ON_JOB_CLOSE still holds.
- Capacity math: straight Rust rewrite (recommended) vs. transitional C# sidecar hedge (adds .NET weight for ~300 lines — only a temporary hedge, not end state).
