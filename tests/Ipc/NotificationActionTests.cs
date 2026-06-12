using System;
using System.Collections.Generic;
using System.Collections.Immutable;
using System.Text.Json;
using Optimus.Core;
using Xunit;

namespace Optimus.Core.Tests;

public sealed class NotificationActionTests
{
    [Fact] // Covers U6 v2 create + create_for_caller.
    public void V2_create_routes_to_create_target_with_fields()
    {
        FakeSocketEffects effects = new();

        string? response = CommandRouter.Dispatch(
            @"{""id"":""101"",""method"":""notify"",""params"":{""surface_id"":""S3"",""workspace_id"":""ws"",""title"":""done"",""subtitle"":""done-sub"",""body"":""done-body""}}",
            effects,
            AuthState.Unprotected);

        Assert.NotNull(response);
        using JsonDocument doc = JsonDocument.Parse(response);
        Assert.True(doc.RootElement.GetProperty("ok").GetBoolean());
        Assert.Equal(new SurfaceId(3), effects.CreatedForSurface);
        Assert.Equal("ws", effects.CreatedForWorkspace);
        Assert.Equal("done", effects.CreatedTitle);
        Assert.Equal("done-sub", effects.CreatedSubtitle);
        Assert.Equal("done-body", effects.CreatedBody);
    }

    [Fact] // Covers U6 v2 mark_read scope all.
    public void V2_mark_read_dispatches_all_read_marker()
    {
        FakeSocketEffects effects = new();

        string? response = CommandRouter.Dispatch(
            @"{""id"":""102"",""method"":""notification.mark_read"",""params"":{""scope"":""all""}}",
            effects,
            AuthState.Unprotected);

        Assert.NotNull(response);
        using JsonDocument doc = JsonDocument.Parse(response);
        Assert.True(doc.RootElement.GetProperty("ok").GetBoolean());
        Assert.True(effects.MarkAllRead);
    }

    [Fact] // Covers U6 v2 dismiss scope all_read.
    public void V2_dismiss_dispatches_all_read_marker()
    {
        FakeSocketEffects effects = new();

        string? response = CommandRouter.Dispatch(
            @"{""id"":""103"",""method"":""notification.dismiss"",""params"":{""scope"":""all_read""}}",
            effects,
            AuthState.Unprotected);

        Assert.NotNull(response);
        using JsonDocument doc = JsonDocument.Parse(response);
        Assert.True(doc.RootElement.GetProperty("ok").GetBoolean());
        Assert.True(effects.DismissAllReadCalled);
    }

    [Fact] // Covers U6 v2 create_for_caller preferred surface.
    public void V2_create_for_caller_uses_preferred_surface()
    {
        FakeSocketEffects effects = new();

        string? response = CommandRouter.Dispatch(
            @"{""id"":""104"",""method"":""notification.create_for_caller"",""params"":{""preferred_surface_id"":""S4"",""title"":""title"",""subtitle"":"""",""body"":""body""}}",
            effects,
            AuthState.Unprotected);

        Assert.NotNull(response);
        using JsonDocument doc = JsonDocument.Parse(response);
        Assert.True(doc.RootElement.GetProperty("ok").GetBoolean());
        Assert.Equal("S4", effects.CallerPreferredSurface);
        Assert.Equal("title", effects.CallerTitle);
        Assert.Equal(string.Empty, effects.CallerSubtitle);
        Assert.Equal("body", effects.CallerBody);
    }

    private sealed class FakeSocketEffects : ISocketEffects
    {
        public string CreatedForWorkspace = string.Empty;
        public SurfaceId CreatedForSurface;
        public string CreatedTitle = string.Empty;
        public string CreatedSubtitle = string.Empty;
        public string CreatedBody = string.Empty;

        public string? CallerPreferredSurface;
        public string CallerTitle = string.Empty;
        public string CallerSubtitle = string.Empty;
        public string CallerBody = string.Empty;

        public bool MarkAllRead;
        public bool DismissAllReadCalled;

        public string Capabilities() => "v1,v2";
        public bool Authenticate(string credential) => credential == "ok";
        public TreeSnapshot Tree() => new(new PaneLeaf(new PaneId(1), ImmutableList.Create(new SurfaceId(1)), new SurfaceId(1)), new PaneId(1), null, 1);
        public string Ping() => "pong";
        public void FocusSurface(SurfaceId surface) { }
        public void SendText(SurfaceId surface, string text) { }
        public void SendKey(SurfaceId surface, uint virtualKey, uint modifiers) { }
        public void CreateNotificationForTarget(string workspaceId, SurfaceId surface, string title, string subtitle, string body)
        {
            CreatedForWorkspace = workspaceId;
            CreatedForSurface = surface;
            CreatedTitle = title;
            CreatedSubtitle = subtitle;
            CreatedBody = body;
        }
        public void CreateNotification(string title, string subtitle, string body) => throw new NotImplementedException();
        public void CreateNotificationForCaller(string? preferredSurfaceId, string title, string subtitle, string body)
        {
            CallerPreferredSurface = preferredSurfaceId;
            CallerTitle = title;
            CallerSubtitle = subtitle;
            CallerBody = body;
        }
        public IReadOnlyList<TerminalNotification> NotificationList() => [];
        public void NotificationDismiss(Guid notificationId) { }
        public void NotificationDismissForSurface(SurfaceId surface) { }
        public void NotificationDismissAllRead() => DismissAllReadCalled = true;
        public void NotificationClear() { }
        public void NotificationMarkRead(Guid notificationId) { }
        public void NotificationMarkRead(SurfaceId surface) { }
        public void NotificationMarkAllRead() => MarkAllRead = true;
        public void NotificationOpen(Guid notificationId) { }
        public bool JumpToUnread() => true;
        public void SetStatus(string status) { }
        public void SetProgress(string progress) { }
        public void LogLine(string line) { }
        public void SidebarState(string payload) { }
        public void ReportGitBranch(SurfaceId surface, string branch, bool isDirty) { }
        public void ReportPr(SurfaceId surface, string number, string label, string status, string? branch, bool isStale) { }
        public void ReportPwd(SurfaceId surface, string path) { }
    }
}
