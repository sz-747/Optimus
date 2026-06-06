using System;
using System.Collections.Generic;
using System.Linq;
using Cmux.Core;
using Microsoft.UI.Xaml.Controls;

namespace Cmux.Splits;

/// <summary>
/// The single hosted workspace (plan Phase 2 U6): it owns the model plane (a
/// <see cref="SplitTreeController"/>), the surface plane (a <see cref="SurfaceManager"/> over real
/// <c>TerminalPane</c>s), the <see cref="SplitTreeView"/> that renders them, and the
/// <see cref="ShortcutRouter"/> that drives them by keyboard. It seeds one pane + one surface on
/// construction (preserving Phase-1 launch behavior) and is the sole place the two planes are wired
/// together.
///
/// <para><b>Event flow.</b> Controller operations raise <see cref="SplitTreeController.SurfaceCreated"/>/
/// <see cref="SplitTreeController.SurfaceClosed"/> (→ create/dispose the matching engine) and then a
/// <see cref="SplitTreeController.SnapshotChanged"/> (→ re-render the tree, reconcile each pane, and
/// move OS focus to the derived focused surface — R7). Closing the last surface raises
/// <see cref="SplitTreeController.Emptied"/>, which re-seeds so the window never goes contentless
/// (R6). On window close, <see cref="ShutdownAll"/> tears every engine down in order (R9).</para>
/// </summary>
public sealed class WorkspaceView : UserControl
{
    private readonly SplitTreeController _controller = new();
    private readonly SurfaceManager _surfaces = new(new TerminalPaneSurfaceFactory());
    private readonly SplitTreeView _tree;
    private readonly ShortcutRouter _shortcuts;

    private readonly Dictionary<PaneId, PaneView> _panes = new();
    private readonly Dictionary<SurfaceId, string> _titles = new();

    // Notification plane (plan Phase 3). The coordinator is the pure brain (store + queue + policy);
    // WorkspaceView is the thin driver that feeds it surface notifications, debounces the drain onto
    // the dispatcher, and renders the resulting cues. Per-surface notification handlers are kept so
    // they can be detached on close (leak safety — the macOS-style unbalanced lambda is deliberately
    // not copied here, since these capture the coordinator).
    private readonly NotificationCoordinator _coordinator = new();
    private readonly Dictionary<SurfaceId, Action<SurfaceNotification>> _notifyHandlers = new();
    private readonly ToastService _toasts;
    private bool _appFocused = true;
    private bool _drainScheduled;

    private SurfaceId? _lastFocusedSurface;

    /// <summary>Raised when the focused surface's title (or the focus itself) changes — drives the window chrome.</summary>
    public event Action<string>? ActiveTitleChanged;

    /// <summary>
    /// Whether the host window is currently the foreground window. A <see cref="UserControl"/> cannot
    /// read its window's activation directly (WinUI 3 exposes only <c>Window.Activated</c>), so
    /// <see cref="MainWindow"/> pushes it in here. Feeds the notification suppression rule (R4): a
    /// notification on the focused, visible surface is suppressed only while the app is foreground.
    /// </summary>
    public bool AppFocused
    {
        get => _appFocused;
        set => _appFocused = value;
    }

    public WorkspaceView()
    {
        _tree = new SplitTreeView(
            paneFactory: GetOrCreatePane,
            onDividerChanged: (branch, fraction) => _controller.SetDividerPosition(branch, fraction));

        _controller.SurfaceCreated += OnSurfaceCreated;
        _controller.SurfaceClosed += OnSurfaceClosed;
        _controller.Emptied += OnEmptied;
        _controller.SnapshotChanged += OnSnapshotChanged;

        _coordinator.Surfaced += OnSurfaced;
        _coordinator.FlashCleared += OnFlashCleared;

        // Desktop-toast surface (U8): isolated and self-degrading — if registration fails the rest of
        // the notification plane is unaffected. Clicking a toast focuses its surface (AE6).
        _toasts = new ToastService(DispatcherQueue);
        _toasts.Activated += FocusSurfaceById;
        _toasts.Register();

        _shortcuts = new ShortcutRouter(_controller, _surfaces);
        _shortcuts.Attach(this);

        Content = _tree;

        // The controller's constructor seeds the first pane + surface silently (no subscribers yet),
        // so create that engine here and drive the first render manually.
        foreach (SurfaceId id in _controller.AllSurfaces)
        {
            CreateSurfaceEngine(id);
        }
        OnSnapshotChanged(_controller.Snapshot());
    }

    /// <summary>Tear down every surface engine in order (R9) and unregister the toast activator.
    /// Called from the window's Closed handler.</summary>
    public void ShutdownAll()
    {
        _toasts.Unregister();
        _surfaces.DisposeAll();
    }

    // ---- Surface plane -----------------------------------------------------------------------

    private void OnSurfaceCreated(SurfaceId id) => CreateSurfaceEngine(id);

    private void CreateSurfaceEngine(SurfaceId id)
    {
        ISurface surface = _surfaces.CreateSurface(id); // default shell, inherited cwd (Phase-1 parity)
        surface.TitleChanged += title => OnSurfaceTitleChanged(id, title);

        // Notification handler is stored so it can be detached on close (leak safety — see field doc).
        void OnNotify(SurfaceNotification n) => OnSurfaceNotification(id, n);
        _notifyHandlers[id] = OnNotify;
        surface.NotificationRaised += OnNotify;
    }

    private void OnSurfaceClosed(SurfaceId id)
    {
        if (_notifyHandlers.Remove(id, out Action<SurfaceNotification>? handler)
            && _surfaces.Get(id) is ISurface surface)
        {
            surface.NotificationRaised -= handler;
        }
        _surfaces.DisposeSurface(id);
        _titles.Remove(id);
    }

    private void OnSurfaceTitleChanged(SurfaceId id, string title)
    {
        _titles[id] = title;
        if (_controller.FocusedSurface == id)
        {
            RaiseActiveTitle();
        }
    }

    // ---- Model → view ------------------------------------------------------------------------

    private void OnEmptied() => _controller.SeedRoot(); // re-seeds and emits SurfaceCreated + a snapshot.

    private void OnSnapshotChanged(TreeSnapshot snapshot)
    {
        _tree.Render(snapshot);

        if (snapshot.Root is SplitNode root)
        {
            var present = new HashSet<PaneId>();
            foreach (PaneLeaf leaf in root.Leaves())
            {
                present.Add(leaf.Id);
                GetOrCreatePane(leaf.Id).Sync(snapshot);
            }
            foreach (PaneId id in _panes.Keys.Where(k => !present.Contains(k)).ToList())
            {
                _panes.Remove(id);
            }
        }

        // OS focus follows the derived focused surface, but only when it actually changes — so a
        // divider drag, equalize, or zoom never steals focus from the terminal (R7).
        if (_controller.FocusedSurface != _lastFocusedSurface)
        {
            _lastFocusedSurface = _controller.FocusedSurface;
            // Focusing a surface marks its notifications read and clears the pane flash (R6/AE6).
            _coordinator.OnFocusChanged(snapshot);
            if (_lastFocusedSurface is SurfaceId focused)
            {
                _surfaces.Get(focused)?.FocusSurface();
            }
        }

        RaiseActiveTitle();
    }

    private PaneView GetOrCreatePane(PaneId id)
    {
        if (!_panes.TryGetValue(id, out PaneView? pane))
        {
            pane = new PaneView(id, _controller, _surfaces, IsSurfaceUnread);
            _panes[id] = pane;
        }
        return pane;
    }

    // ---- Notification plane ------------------------------------------------------------------

    private bool IsSurfaceUnread(SurfaceId id) => _coordinator.IsSurfaceUnread(id);

    /// <summary>
    /// A surface raised an engine notification (OSC 9/99/777). Enqueue it and debounce a drain onto
    /// the dispatcher: many notifications arriving back-to-back collapse into a single drain pass
    /// (KTD4), and a capped batch reschedules its remainder.
    /// </summary>
    private void OnSurfaceNotification(SurfaceId id, SurfaceNotification n)
    {
        _coordinator.OnNotification(id, n);
        ScheduleDrain();
    }

    private void ScheduleDrain()
    {
        if (_drainScheduled)
        {
            return;
        }
        _drainScheduled = true;
        DispatcherQueue.TryEnqueue(DrainNotifications);
    }

    private void DrainNotifications()
    {
        _drainScheduled = false;
        // Re-derive owning pane + visibility from a fresh snapshot at delivery time (KTD6).
        bool more = _coordinator.Drain(_controller.Snapshot(), _appFocused);
        if (more)
        {
            ScheduleDrain();
        }
    }

    /// <summary>
    /// A notification cleared policy and was delivered: flash its pane (if requested) and re-render
    /// that pane's tab strip so the unread dot appears. The OS toast (U8) hangs off this too.
    /// </summary>
    private void OnSurfaced(SurfacedNotification surfaced)
    {
        if (_panes.TryGetValue(surfaced.Notification.PaneId, out PaneView? pane))
        {
            if (surfaced.Flash)
            {
                pane.Flash();
            }
            pane.RefreshHeaders();
        }
        if (surfaced.ShowToast)
        {
            _toasts.Show(surfaced.Notification);
        }
    }

    /// <summary>
    /// Focus the surface a clicked toast originated from (AE6): select its tab and focus its pane,
    /// which drives the snapshot focus-change path that marks the notification read and clears the
    /// pane flash. No-op if the surface has since closed.
    /// </summary>
    private void FocusSurfaceById(SurfaceId id)
    {
        TreeSnapshot snapshot = _controller.Snapshot();
        if (snapshot.Root.FindContaining(id) is PaneLeaf leaf)
        {
            _controller.FocusPane(leaf.Id);
            _controller.SelectTab(leaf.Id, id);
        }
    }

    /// <summary>Focus cleared a pane's unread state: stop its flash and drop the unread dot (AE6).</summary>
    private void OnFlashCleared(PaneId pane)
    {
        if (_panes.TryGetValue(pane, out PaneView? view))
        {
            view.ClearFlash();
            view.RefreshHeaders();
        }
    }

    private void RaiseActiveTitle()
    {
        string title = _controller.FocusedSurface is SurfaceId id
            && _titles.TryGetValue(id, out string? t)
            && !string.IsNullOrEmpty(t)
                ? t
                : "cmux";
        ActiveTitleChanged?.Invoke(title);
    }
}
