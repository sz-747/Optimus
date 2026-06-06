using System;
using System.Collections.Generic;

namespace Cmux.Core;

/// <summary>
/// Socket-command effect surface (Core/U4): command handlers call this interface instead of WinUI.
/// The app implements these operations so command-dispatch can stay fully unit-testable.
/// </summary>
public interface ISocketEffects
{
    TreeSnapshot Tree();

    string Ping();
    string Capabilities();

    bool Authenticate(string credential);

    void FocusSurface(SurfaceId surface);

    void SendText(SurfaceId surface, string text);
    void SendKey(SurfaceId surface, uint virtualKey, uint modifiers);

    void CreateNotificationForTarget(string workspaceId, SurfaceId surface, string title, string subtitle, string body);

    IReadOnlyList<TerminalNotification> NotificationList();
    void NotificationDismiss(Guid notificationId);
    void NotificationDismissForSurface(SurfaceId surface);
    void NotificationClear();
    void NotificationMarkRead(Guid notificationId);
    void NotificationOpen(Guid notificationId);
    bool JumpToUnread();

    void SetStatus(string status);
    void SetProgress(string progress);
    void LogLine(string line);
    void SidebarState(string payload);

    void ReportGitBranch(SurfaceId surface, string branch, bool isDirty);
    void ReportPr(SurfaceId surface, string number, string label, string status, string? branch, bool isStale);
    void ReportPwd(SurfaceId surface, string path);
}

