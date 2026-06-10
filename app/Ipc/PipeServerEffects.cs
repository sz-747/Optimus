using System;
using System.Collections.Generic;
using System.Threading.Tasks;
using Optimus.Core;
using Optimus.Sidebar;
using Microsoft.UI.Dispatching;

namespace Optimus.Ipc;

internal sealed class PipeServerEffects : ISocketEffects
{
    private readonly WorkspaceHost _workspace;
    private readonly Func<string, bool> _authenticate;

    public PipeServerEffects(WorkspaceHost workspace, Func<string, bool> authenticate)
    {
        _workspace = workspace ?? throw new System.ArgumentNullException(nameof(workspace));
        _authenticate = authenticate ?? throw new System.ArgumentNullException(nameof(authenticate));
    }

    public TreeSnapshot Tree() => _workspace.Tree();

    public string Ping() => "pong";

    public string Capabilities() => "v1,v2";

    public bool Authenticate(string credential) => _authenticate(credential);

    public void FocusSurface(SurfaceId surface)
    {
        RunOnDispatcher(() => _workspace.FocusSurface(surface));
    }

    public void SendText(SurfaceId surface, string text)
    {
        RunOnDispatcher(() => _workspace.SendText(surface, text));
    }

    public void SendKey(SurfaceId surface, uint virtualKey, uint modifiers)
    {
        RunOnDispatcher(() => _workspace.SendKey(surface, virtualKey, modifiers));
    }

    public void CreateNotificationForTarget(string workspaceId, SurfaceId surface, string title, string subtitle, string body)
    {
        RunOnDispatcher(() => _workspace.CreateNotificationForTarget(workspaceId, surface, title, subtitle, body));
    }

    public void CreateNotification(string title, string subtitle, string body)
    {
        RunOnDispatcher(() => _workspace.CreateNotification(title, subtitle, body));
    }

    public void CreateNotificationForCaller(string? preferredSurfaceId, string title, string subtitle, string body)
    {
        RunOnDispatcher(() => _workspace.CreateNotificationForCaller(preferredSurfaceId, title, subtitle, body));
    }

    public IReadOnlyList<TerminalNotification> NotificationList() => InvokeOnDispatcher(_workspace.NotificationList);

    public void NotificationDismiss(Guid notificationId)
    {
        RunOnDispatcher(() => _workspace.NotificationDismiss(notificationId));
    }

    public void NotificationDismissForSurface(SurfaceId surface)
    {
        RunOnDispatcher(() => _workspace.NotificationDismissForSurface(surface));
    }

    public void NotificationDismissAllRead()
    {
        RunOnDispatcher(() => _workspace.NotificationDismissAllRead());
    }

    public void NotificationClear()
    {
        RunOnDispatcher(() => _workspace.NotificationClear());
    }

    public void NotificationMarkRead(Guid notificationId)
    {
        RunOnDispatcher(() => _workspace.NotificationMarkRead(notificationId));
    }

    public void NotificationMarkRead(SurfaceId surface)
    {
        RunOnDispatcher(() => _workspace.NotificationMarkRead(surface));
    }

    public void NotificationMarkAllRead()
    {
        RunOnDispatcher(() => _workspace.NotificationMarkAllRead());
    }

    public void NotificationOpen(Guid notificationId)
    {
        RunOnDispatcher(() => _workspace.NotificationOpen(notificationId));
    }

    public bool JumpToUnread() => InvokeOnDispatcher(_workspace.JumpToUnread);

    // Phase 5: status/progress/report verbs land on workspace metadata and feed the sidebar.
    public void SetStatus(string status) => RunOnDispatcher(() => _workspace.SetStatus(status));

    public void SetProgress(string progress) => RunOnDispatcher(() => _workspace.SetProgress(progress));

    public void LogLine(string line) { }

    public void SidebarState(string payload) { }

    public void ReportGitBranch(SurfaceId surface, string branch, bool isDirty) =>
        RunOnDispatcher(() => _workspace.ReportGitBranch(surface, branch, isDirty));

    public void ReportPr(SurfaceId surface, string number, string label, string status, string? branch, bool isStale) =>
        RunOnDispatcher(() => _workspace.ReportPr(surface, number, label, status, branch, isStale));

    public void ReportPwd(SurfaceId surface, string path) =>
        RunOnDispatcher(() => _workspace.ReportPwd(surface, path));

    private void RunOnDispatcher(Action action)
    {
        if (!_workspace.DispatcherQueue.TryEnqueue(() => action()))
        {
            action();
        }
    }

    private T InvokeOnDispatcher<T>(Func<T> action)
    {
        if (_workspace.DispatcherQueue.HasThreadAccess)
        {
            return action();
        }

        TaskCompletionSource<T> completion = new(TaskCreationOptions.RunContinuationsAsynchronously);
        RunOnDispatcher(() =>
        {
            try
            {
                completion.TrySetResult(action());
            }
            catch (Exception ex)
            {
                completion.TrySetException(ex);
            }
        });
        return completion.Task.GetAwaiter().GetResult();
    }
}
