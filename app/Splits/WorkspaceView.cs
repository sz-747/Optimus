using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using Optimus.Controls;
using Optimus.Core;
using Microsoft.UI.Xaml.Controls;

namespace Optimus.Splits;

/// <summary>
/// One hosted workspace (plan Phase 2 U6, multi-workspace since Phase 5): it renders the model
/// plane of a <see cref="Optimus.Core.Workspace"/> (whose <see cref="SplitTreeController"/> it drives),
/// owns the surface plane (a <see cref="SurfaceManager"/> over real <c>TerminalPane</c>s), the
/// <see cref="SplitTreeView"/> that renders them, and the <see cref="ShortcutRouter"/> that drives
/// them by keyboard. The controller arrives pre-seeded with one pane + one surface (preserving
/// Phase-1 launch behavior); this view is the sole place the two planes are wired together.
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
    private readonly SplitTreeController _controller;
    private readonly SurfaceManager _surfaces =
        new(new TerminalPaneSurfaceFactory(), App.Capacity); // null-tolerant: ungoverned if the governor failed to start
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
    private readonly ToastService? _toasts;

    // False until the host pushes a real activation state — see WorkspaceHost._appFocused.
    private bool _appFocused;
    private bool _drainScheduled;

    private SurfaceId? _lastFocusedSurface;

    /// <summary>Raised when the focused surface's title (or the focus itself) changes — drives the window chrome.</summary>
    public event Action<string>? ActiveTitleChanged;

    /// <summary>
    /// Raised after any notification-plane mutation (deliver, mark, dismiss, clear) so the sidebar
    /// can recompute this workspace's unread badge and latest-text row (Phase 5).
    /// </summary>
    public event Action? NotificationsChanged;

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

    /// <summary>The workspace model this view renders (Phase 5): metadata + the split controller.</summary>
    public Workspace Model { get; }

    /// <summary>Total unread notifications in this workspace (the sidebar badge source).</summary>
    public int UnreadCount => _coordinator.Store.UnreadCount;

    /// <summary>Newest recorded notification in this workspace, or null (the sidebar latest-text source).</summary>
    public TerminalNotification? LatestNotification =>
        _coordinator.Store.Items.Count > 0 ? _coordinator.Store.Items[0] : null;

    /// <summary>
    /// Build the view for <paramref name="model"/>. <paramref name="toasts"/> is the app's single
    /// desktop-toast surface (the COM activator registers once per process, so the host owns it and
    /// shares it across workspaces); null leaves only the in-app flash + badge (KTD8).
    /// </summary>
    public WorkspaceView(Workspace model, ToastService? toasts = null)
    {
        Model = model ?? throw new ArgumentNullException(nameof(model));
        _controller = model.Controller;
        _toasts = toasts;

        _tree = new SplitTreeView(
            paneFactory: GetOrCreatePane,
            onDividerChanged: (branch, fraction) => _controller.SetDividerPosition(branch, fraction));

        _controller.SurfaceCreated += OnSurfaceCreated;
        _controller.SurfaceClosed += OnSurfaceClosed;
        _controller.Emptied += OnEmptied;
        _controller.SnapshotChanged += OnSnapshotChanged;

        _coordinator.Surfaced += OnSurfaced;
        _coordinator.FlashCleared += OnFlashCleared;

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

    public TreeSnapshot Tree() => _controller.Snapshot();

    public void FocusSurface(SurfaceId surface) => FocusSurfaceById(surface);

    public void SendText(SurfaceId surface, string text)
    {
        if (_surfaces.Get(surface) is TerminalPane pane)
        {
            pane.SendText(text);
        }
    }

    public void SendKey(SurfaceId surface, uint virtualKey, uint modifiers)
    {
        if (_surfaces.Get(surface) is TerminalPane pane)
        {
            pane.SendKey(virtualKey, modifiers);
        }
    }

    public void CreateNotificationForTarget(string workspaceId, SurfaceId surface, string title, string subtitle, string body)
    {
        _ = workspaceId;
        _coordinator.OnNotification(surface, new SurfaceNotification(title, subtitle, body), coalesce: false);
        ScheduleDrain();
    }

    public void CreateNotification(string title, string subtitle, string body)
    {
        if (_controller.FocusedSurface is not SurfaceId focused)
        {
            return;
        }

        _coordinator.OnNotification(focused, new SurfaceNotification(title, subtitle, body), coalesce: false);
        ScheduleDrain();
    }

    public void CreateNotificationForCaller(string? preferredSurfaceId, string title, string subtitle, string body)
    {
        if (!TryResolveNotificationSurface(preferredSurfaceId, out SurfaceId surface))
        {
            return;
        }

        _coordinator.OnNotification(surface, new SurfaceNotification(title, subtitle, body), coalesce: false);
        ScheduleDrain();
    }

    public IReadOnlyList<TerminalNotification> NotificationList() => _coordinator.ListNotifications();

    public void NotificationDismiss(Guid notificationId) =>
        NotifyMutation(() => _coordinator.DismissNotification(notificationId));

    public void NotificationDismissForSurface(SurfaceId surface) =>
        NotifyMutation(() => _coordinator.DismissNotificationForSurface(surface));

    public void NotificationDismissAllRead() => NotifyMutation(_coordinator.DismissAllRead);

    public void NotificationClear() => NotifyMutation(_coordinator.ClearNotifications);

    public void NotificationMarkRead(Guid notificationId) =>
        NotifyMutation(() => _coordinator.MarkRead(notificationId));

    public void NotificationMarkRead(SurfaceId surface) =>
        NotifyMutation(() => _coordinator.MarkRead(surface));

    public void NotificationMarkAllRead() => NotifyMutation(_coordinator.MarkAllRead);

    private void NotifyMutation(Action mutate)
    {
        mutate();
        NotificationsChanged?.Invoke();
    }

    public bool NotificationOpen(Guid notificationId)
    {
        foreach (TerminalNotification n in _coordinator.ListNotifications())
        {
            if (n.Id == notificationId)
            {
                FocusSurfaceById(n.SurfaceId);
                return true;
            }
        }
        return false;
    }

    public bool JumpToUnread()
    {
        (PaneId Pane, SurfaceId Surface)? target = _coordinator.JumpToUnreadTarget();
        if (!target.HasValue)
        {
            return false;
        }

        _controller.FocusPane(target.Value.Pane);
        _controller.SelectTab(target.Value.Pane, target.Value.Surface);
        return true;
    }

    /// <summary>Tear down every surface engine in order (R9). Called by the host on workspace close
    /// and window close (the shared toast activator is the host's to unregister).</summary>
    public void ShutdownAll()
    {
        _surfaces.DisposeAll();
    }

    // ---- Surface plane -----------------------------------------------------------------------

    private void OnSurfaceCreated(SurfaceId id) => CreateSurfaceEngine(id);

    private void CreateSurfaceEngine(SurfaceId id)
    {
        // Capacity-gated (RAM safe-zone plan U5): at the cap the create is refused gracefully —
        // the model-plane pane stays engineless rather than crashing the machine by over-spawning.
        // U6 disables the spawn affordances before users normally hit this path.
        ISurface? surface = _surfaces.TryCreateSurface(id); // default shell, inherited cwd (Phase-1 parity)
        if (surface is null)
        {
            System.Diagnostics.Debug.WriteLine($"[capacity] surface {id} refused: safe-zone cap reached");
            return;
        }
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
            // Focusing a surface marks its notifications read and clears the pane flash (R6/AE6) —
            // an unread mutation the sidebar badge must observe too (Phase 5).
            _coordinator.OnFocusChanged(snapshot);
            NotificationsChanged?.Invoke();
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
        NotificationsChanged?.Invoke();
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
            _toasts?.Show(surfaced.Notification);
        }
    }

    /// <summary>
    /// Focus the surface a clicked toast originated from (AE6): select its tab and focus its pane,
    /// which drives the snapshot focus-change path that marks the notification read and clears the
    /// pane flash. No-op if the surface has since closed.
    /// </summary>
    public void FocusSurfaceById(SurfaceId id)
    {
        TreeSnapshot snapshot = _controller.Snapshot();
        if (snapshot.Root.FindContaining(id) is PaneLeaf leaf)
        {
            _controller.FocusPane(leaf.Id);
            _controller.SelectTab(leaf.Id, id);
        }
    }

    private bool TryResolveNotificationSurface(string? preferredSurfaceId, out SurfaceId surface)
    {
        if (TryParseSurfaceId(preferredSurfaceId, out surface))
        {
            return true;
        }

        if (_controller.FocusedSurface is SurfaceId focused)
        {
            surface = focused;
            return true;
        }

        surface = default;
        return false;
    }

    private static bool TryParseSurfaceId(string? value, out SurfaceId surface)
    {
        surface = default;
        if (string.IsNullOrWhiteSpace(value))
        {
            return false;
        }

        ReadOnlySpan<char> text = value.AsSpan().Trim();
        if (text.Length > 1 && char.ToUpperInvariant(text[0]) == 'S')
        {
            if (int.TryParse(text[1..], NumberStyles.Integer, CultureInfo.InvariantCulture, out int parsed))
            {
                surface = new SurfaceId(parsed);
                return true;
            }
        }

        if (int.TryParse(text, NumberStyles.Integer, CultureInfo.InvariantCulture, out int raw))
        {
            surface = new SurfaceId(raw);
            return true;
        }

        return false;
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
                : "optimus";
        // The focused surface's title doubles as the workspace's derived title (the sidebar row
        // label, unless a custom title overrides it — Phase 5).
        Model.SetTitle(title);
        ActiveTitleChanged?.Invoke(title);
    }
}
