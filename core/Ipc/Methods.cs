namespace Cmux.Core;

/// <summary>
/// Shared IPC method/verb constants for both CLI and app-side sockets.
/// </summary>
public static class SocketMethods
{
    // V2 system methods.
    public const string SystemPing = "system.ping";
    public const string SystemCapabilities = "system.capabilities";

    // V1 verbs (space-separated, newline-framed).
    public const string Send = "send";
    public const string SendKey = "send-key";
    public const string Notify = "notify";
    public const string NotifyTarget = "notify_target_async";
    public const string CreateNotification = "notification.create";
    public const string CreateNotificationForCaller = "notification.create_for_caller";
    public const string ListNotifications = "notification.list";
    public const string DismissNotification = "notification.dismiss";
    public const string DismissNotificationForSurface = "notification.dismiss_surface";
    public const string DismissAllNotifications = "notification.clear";
    public const string ClearReadNotifications = "notification.mark_read";
    public const string OpenNotification = "notification.open";
    public const string JumpToUnread = "notification.jump_to_unread";
    public const string SetStatus = "set-status";
    public const string SetProgress = "set-progress";
    public const string LogLine = "log";
    public const string SidebarState = "sidebar-state";

    public const string ReportGitBranch = "report_git_branch";
    public const string ReportPr = "report_pr";
    public const string ReportPwd = "report_pwd";
    public const string ReportShellState = "report_shell_state";
    public const string ReportReview = "report_review";

    // Events stream.
    public const string EventsStream = "events.stream";

    // V2 JSON methods.
    public const string SurfaceSendText = "surface.send_text";
    public const string SurfaceSendKey = "surface.send_key";
    public const string SurfaceFocus = "surface.focus";
    public const string EventsStreamV2 = "events.stream";
    public const string AuthLogin = "auth.login";
    public const string V2Notify = "notify";

    // Authentication.
    public const string Auth = "auth";
}
