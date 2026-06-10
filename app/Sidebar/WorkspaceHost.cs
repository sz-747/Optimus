using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using Cmux.Core;
using Cmux.Splits;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Cmux.Sidebar;

/// <summary>
/// The multi-workspace shell (plan Phase 5): a <see cref="SidebarView"/> beside a content area
/// holding one <see cref="WorkspaceView"/> per workspace, with only the selected one visible (the
/// others stay alive — engines keep running, Collapsed costs no composition). Owns the
/// <see cref="WorkspaceManager"/> (the model plane), the single process-wide
/// <see cref="ToastService"/>, and the routing of every Phase-4 socket effect to the right
/// workspace by surface id (unambiguous because all controllers share one id allocator).
///
/// <para>Sidebar rendering follows the snapshot-boundary rule (issue #2586): every change recomputes
/// an immutable <see cref="SidebarRowDto"/> array on the UI thread and hands it to the view — no row
/// ever binds to a live workspace object.</para>
/// </summary>
public sealed class WorkspaceHost : UserControl
{
    private readonly WorkspaceManager _manager = new();
    private readonly Dictionary<WorkspaceId, WorkspaceView> _views = new();
    private readonly SidebarView _sidebar = new();
    private readonly Grid _content = new();
    private readonly ToastService _toasts;

    // Starts false: a window launched without winning foreground (e.g. spawned from a background
    // shell) never fires Window.Activated, and a stale "focused" default would suppress every
    // notification (R4) until the user first clicks the window.
    private bool _appFocused;

    /// <summary>Raised when the selected workspace's focused surface title changes — drives window chrome.</summary>
    public event Action<string>? ActiveTitleChanged;

    public WorkspaceHost()
    {
        // Desktop-toast surface (Phase 3 U8): the COM activator registers once per process, so the
        // host owns the single instance and shares it across workspace views. Clicking a toast
        // focuses its surface — selecting the owning workspace first (AE6).
        _toasts = new ToastService(DispatcherQueue);
        _toasts.Activated += FocusSurface;
        _toasts.Register();

        _manager.WorkspaceCreated += OnWorkspaceCreated;
        _manager.WorkspaceClosed += OnWorkspaceClosed;
        _manager.Changed += OnManagerChanged;

        _sidebar.WorkspaceInvoked += _manager.SelectWorkspace;
        _sidebar.WorkspaceCloseRequested += _manager.CloseWorkspace;
        _sidebar.NewWorkspaceRequested += () => _manager.NewWorkspace();

        var root = new Grid
        {
            ColumnDefinitions =
            {
                new ColumnDefinition { Width = GridLength.Auto },
                new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) },
            },
        };
        Grid.SetColumn(_sidebar, 0);
        root.Children.Add(_sidebar);
        Grid.SetColumn(_content, 1);
        root.Children.Add(_content);
        Content = root;

        // The manager seeded its first workspace before we could subscribe — build its view now.
        foreach (Workspace workspace in _manager.Workspaces)
        {
            OnWorkspaceCreated(workspace);
        }
        OnManagerChanged();
    }

    /// <summary>Window foreground state, pushed down to every workspace (notification rule R4).</summary>
    public bool AppFocused
    {
        get => _appFocused;
        set
        {
            _appFocused = value;
            foreach (WorkspaceView view in _views.Values)
            {
                view.AppFocused = value;
            }
        }
    }

    /// <summary>Tear down the toast activator and every workspace's engines (R9). Window-close path.</summary>
    public void ShutdownAll()
    {
        _toasts.Unregister();
        foreach (WorkspaceView view in _views.Values)
        {
            view.ShutdownAll();
        }
    }

    // ---- Workspace lifecycle -------------------------------------------------------------------

    private void OnWorkspaceCreated(Workspace workspace)
    {
        var view = new WorkspaceView(workspace, _toasts)
        {
            Visibility = Visibility.Collapsed,
            AppFocused = _appFocused,
        };
        view.ActiveTitleChanged += title => OnViewTitleChanged(workspace.Id, title);
        view.NotificationsChanged += RenderSidebar;
        _views[workspace.Id] = view;
        _content.Children.Add(view);
    }

    private void OnWorkspaceClosed(Workspace workspace)
    {
        if (_views.Remove(workspace.Id, out WorkspaceView? view))
        {
            _content.Children.Remove(view);
            view.ShutdownAll();
        }
    }

    private void OnManagerChanged()
    {
        foreach ((WorkspaceId id, WorkspaceView view) in _views)
        {
            view.Visibility = id == _manager.SelectedId ? Visibility.Visible : Visibility.Collapsed;
        }
        RenderSidebar();
        OnViewTitleChanged(_manager.SelectedId, _manager.Selected.Title);
    }

    private void OnViewTitleChanged(WorkspaceId id, string title)
    {
        if (id == _manager.SelectedId)
        {
            ActiveTitleChanged?.Invoke(string.IsNullOrEmpty(title) ? "cmux" : title);
        }
    }

    private void RenderSidebar() =>
        _sidebar.Render(SidebarProjection.Project(
            _manager,
            unreadOf: id => _views.TryGetValue(id, out WorkspaceView? v) ? v.UnreadCount : 0,
            latestOf: id => _views.TryGetValue(id, out WorkspaceView? v) ? LatestText(v) : null));

    private static string? LatestText(WorkspaceView view)
    {
        if (view.LatestNotification is not TerminalNotification latest)
        {
            return null;
        }
        return string.IsNullOrEmpty(latest.Body) ? latest.Title : latest.Body;
    }

    // ---- Socket-effect routing (the Phase-4 seam, called via PipeServerEffects on the UI thread) -

    /// <summary>The selected workspace's tree (V1 <c>tree</c> verb predates multi-workspace).</summary>
    public TreeSnapshot Tree() => _manager.Selected.Controller.Snapshot();

    /// <summary>Select the owning workspace and focus <paramref name="surface"/> within it.</summary>
    public void FocusSurface(SurfaceId surface)
    {
        if (_manager.FindBySurface(surface) is not Workspace workspace)
        {
            return;
        }
        _manager.SelectWorkspace(workspace.Id);
        _views[workspace.Id].FocusSurfaceById(surface);
    }

    public void SendText(SurfaceId surface, string text) => ViewOf(surface)?.SendText(surface, text);

    public void SendKey(SurfaceId surface, uint virtualKey, uint modifiers) =>
        ViewOf(surface)?.SendKey(surface, virtualKey, modifiers);

    public void CreateNotificationForTarget(string workspaceId, SurfaceId surface, string title, string subtitle, string body)
    {
        // Resolve by explicit workspace id when it parses; otherwise fall back to the surface owner
        // (macOS accepted both routes).
        Workspace? workspace = ParseWorkspaceId(workspaceId) is WorkspaceId id ? _manager.Find(id) : null;
        workspace ??= _manager.FindBySurface(surface);
        if (workspace is not null && _views.TryGetValue(workspace.Id, out WorkspaceView? view))
        {
            view.CreateNotificationForTarget(workspaceId, surface, title, subtitle, body);
        }
    }

    public void CreateNotification(string title, string subtitle, string body) =>
        SelectedView.CreateNotification(title, subtitle, body);

    public void CreateNotificationForCaller(string? preferredSurfaceId, string title, string subtitle, string body)
    {
        // A parseable CMUX_SURFACE_ID pins the caller's workspace; otherwise the selected one hosts.
        WorkspaceView view = SelectedView;
        if (TryParseSurfaceId(preferredSurfaceId, out SurfaceId surface) && ViewOf(surface) is WorkspaceView owner)
        {
            view = owner;
        }
        view.CreateNotificationForCaller(preferredSurfaceId, title, subtitle, body);
    }

    /// <summary>All workspaces' notifications, newest first (ids are globally unique guids).</summary>
    public IReadOnlyList<TerminalNotification> NotificationList() =>
        _views.Values
            .SelectMany(v => v.NotificationList())
            .OrderByDescending(n => n.CreatedAt)
            .ToList();

    public void NotificationDismiss(Guid notificationId) => ForEachView(v => v.NotificationDismiss(notificationId));

    public void NotificationDismissForSurface(SurfaceId surface) => ViewOf(surface)?.NotificationDismissForSurface(surface);

    public void NotificationDismissAllRead() => ForEachView(v => v.NotificationDismissAllRead());

    public void NotificationClear() => ForEachView(v => v.NotificationClear());

    public void NotificationMarkRead(Guid notificationId) => ForEachView(v => v.NotificationMarkRead(notificationId));

    public void NotificationMarkRead(SurfaceId surface) => ViewOf(surface)?.NotificationMarkRead(surface);

    public void NotificationMarkAllRead() => ForEachView(v => v.NotificationMarkAllRead());

    /// <summary>Open a notification by id: the owning workspace is selected and its surface focused.</summary>
    public void NotificationOpen(Guid notificationId)
    {
        foreach ((WorkspaceId id, WorkspaceView view) in _views)
        {
            if (view.NotificationOpen(notificationId))
            {
                _manager.SelectWorkspace(id);
                return;
            }
        }
    }

    /// <summary>Jump to the newest unread, preferring the selected workspace (macOS parity).</summary>
    public bool JumpToUnread()
    {
        if (SelectedView.JumpToUnread())
        {
            return true;
        }
        foreach ((WorkspaceId id, WorkspaceView view) in _views)
        {
            if (view.JumpToUnread())
            {
                _manager.SelectWorkspace(id);
                return true;
            }
        }
        return false;
    }

    /// <summary>Agent status (<c>set-status key:value</c>) lands on the selected workspace: the V2
    /// frame carries no surface, and hooks run in the workspace the user is working in.</summary>
    public void SetStatus(string status) => _manager.Selected.SetStatus(status);

    public void SetProgress(string progress) => _manager.Selected.SetProgress(progress);

    public void ReportGitBranch(SurfaceId surface, string branch, bool isDirty) =>
        _manager.ReportGitBranch(surface, branch, isDirty);

    public void ReportPr(SurfaceId surface, string number, string label, string status, string? branch, bool isStale) =>
        _manager.ReportPr(surface, number, label, status, branch, isStale);

    public void ReportPwd(SurfaceId surface, string path) => _manager.ReportPwd(surface, path);

    // ---- Helpers ---------------------------------------------------------------------------------

    private WorkspaceView SelectedView => _views[_manager.SelectedId];

    private WorkspaceView? ViewOf(SurfaceId surface) =>
        _manager.FindBySurface(surface) is Workspace w && _views.TryGetValue(w.Id, out WorkspaceView? view)
            ? view
            : null;

    private void ForEachView(Action<WorkspaceView> action)
    {
        foreach (WorkspaceView view in _views.Values)
        {
            action(view);
        }
    }

    private static WorkspaceId? ParseWorkspaceId(string? value)
    {
        if (string.IsNullOrWhiteSpace(value))
        {
            return null;
        }
        ReadOnlySpan<char> text = value.AsSpan().Trim();
        if (text.Length > 1 && char.ToUpperInvariant(text[0]) == 'W')
        {
            text = text[1..];
        }
        return int.TryParse(text, NumberStyles.Integer, CultureInfo.InvariantCulture, out int parsed)
            ? new WorkspaceId(parsed)
            : null;
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
            text = text[1..];
        }
        if (int.TryParse(text, NumberStyles.Integer, CultureInfo.InvariantCulture, out int parsed))
        {
            surface = new SurfaceId(parsed);
            return true;
        }
        return false;
    }
}
