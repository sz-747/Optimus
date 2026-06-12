using System;
using System.Collections.Generic;
using System.Collections.Immutable;
using System.Text.Json;
using Optimus.Core;
using Xunit;

namespace Optimus.Core.Tests;

public sealed class CommandRouterTests
{
    [Fact] // Covers R2.
    public void Dispatch_v2_system_ping_returns_ok()
    {
        string? response = CommandRouter.Dispatch(
            @"{""id"":""1"",""method"":""system.ping""}",
            new FakeSocketEffects(),
            AuthState.Unprotected);

        Assert.NotNull(response);
        using JsonDocument doc = JsonDocument.Parse(response);
        Assert.True(doc.RootElement.GetProperty("ok").GetBoolean());
        Assert.True(doc.RootElement.GetProperty("result").GetProperty("pong").GetBoolean());
    }

    [Fact] // Covers R2.
    public void Dispatch_unknown_v2_method_returns_method_not_found()
    {
        string? response = CommandRouter.Dispatch(
            @"{""id"":""2"",""method"":""unknown.method""}",
            new FakeSocketEffects(),
            AuthState.Unprotected);

        Assert.NotNull(response);
        using JsonDocument doc = JsonDocument.Parse(response);
        Assert.False(doc.RootElement.GetProperty("ok").GetBoolean());
        Assert.Equal("method_not_found", doc.RootElement.GetProperty("error").GetProperty("code").GetString());
    }

    [Fact] // Covers U4 mapping and R5.
    public void Notify_target_dispatches_notification_create_handler()
    {
        FakeSocketEffects effects = new();

        string? response = CommandRouter.Dispatch(
            "notify_target_async ws S2 task|done|ready",
            effects,
            AuthState.Unprotected);

        Assert.Equal(SocketWireProtocol.SerializeV1Response("OK"), response);
        Assert.Equal(new SurfaceId(2), effects.CreatedForSurface);
        Assert.Equal("ws", effects.CreatedForWorkspace);
        Assert.Equal("task", effects.CreatedTitle);
        Assert.Equal("done", effects.CreatedSubtitle);
        Assert.Equal("ready", effects.CreatedBody);
    }

    [Fact] // Covers U6 create route.
    public void Create_notification_dispatches_notification_target_handler()
    {
        FakeSocketEffects effects = new();

        string? response = CommandRouter.Dispatch(
            "notification.create ws S1 title|subtitle|body",
            effects,
            AuthState.Unprotected);

        Assert.Equal(SocketWireProtocol.SerializeV1Response("OK"), response);
        Assert.Equal(new SurfaceId(1), effects.CreatedForSurface);
        Assert.Equal("ws", effects.CreatedForWorkspace);
        Assert.Equal("title", effects.CreatedTitle);
        Assert.Equal("subtitle", effects.CreatedSubtitle);
        Assert.Equal("body", effects.CreatedBody);
    }

    [Fact] // Covers U6 create_for_caller route.
    public void Create_for_caller_notification_dispatches_caller_handler()
    {
        FakeSocketEffects effects = new();

        string? response = CommandRouter.Dispatch(
            "notification.create_for_caller title|subtitle|body",
            effects,
            AuthState.Unprotected);

        Assert.Equal(SocketWireProtocol.SerializeV1Response("OK"), response);
        Assert.Equal("title", effects.CallerTitle);
        Assert.Equal("subtitle", effects.CallerSubtitle);
        Assert.Equal("body", effects.CallerBody);
        Assert.Null(effects.CallerPreferredSurface);
    }

    [Fact] // Covers U6 mark_read surface.
    public void Mark_read_dispatches_surface_handler()
    {
        FakeSocketEffects effects = new();

        string? response = CommandRouter.Dispatch(
            "notification.mark_read S2",
            effects,
            AuthState.Unprotected);

        Assert.Equal(SocketWireProtocol.SerializeV1Response("OK"), response);
        Assert.Equal(new SurfaceId(2), effects.MarkReadSurface);
    }

    [Fact] // Covers U6 mark_read all scope.
    public void Mark_read_dispatches_all_handler()
    {
        FakeSocketEffects effects = new();

        string? response = CommandRouter.Dispatch(
            "notification.mark_read all",
            effects,
            AuthState.Unprotected);

        Assert.Equal(SocketWireProtocol.SerializeV1Response("OK"), response);
        Assert.True(effects.MarkAllRead);
    }

    [Fact] // Covers U6 dismiss all_read scope.
    public void Dismiss_dispatches_all_read_handler()
    {
        FakeSocketEffects effects = new();

        string? response = CommandRouter.Dispatch(
            "notification.dismiss all_read",
            effects,
            AuthState.Unprotected);

        Assert.Equal(SocketWireProtocol.SerializeV1Response("OK"), response);
        Assert.True(effects.DismissAllReadCalled);
    }

    [Fact] // Covers U4 and V2 parser.
    public void Surface_send_text_dispatches_send_handler()
    {
        FakeSocketEffects effects = new();

        string? response = CommandRouter.Dispatch(
            @"{""id"":""3"",""method"":""surface.send_text"",""params"":{""surface_id"":""S4"",""text"":""hello""}}",
            effects,
            AuthState.Unprotected);

        Assert.NotNull(response);
        using JsonDocument doc = JsonDocument.Parse(response);
        Assert.True(doc.RootElement.GetProperty("ok").GetBoolean());
        Assert.True(doc.RootElement.GetProperty("result").GetProperty("ok").GetBoolean());
        Assert.Equal(new SurfaceId(4), effects.SentSurface);
        Assert.Equal("hello", effects.SentText);
    }

    [Fact] // Covers auth gating.
    public void Auth_required_blocks_command_until_authenticated()
    {
        FakeSocketEffects effects = new();
        AuthState locked = new(RequiresAuthentication: true, IsAuthenticated: false);

        string? v1 = CommandRouter.Dispatch("send S1 hello", effects, locked);
        Assert.Equal(SocketWireProtocol.SerializeV1Response("ERROR: auth required"), v1);

        string? v2 = CommandRouter.Dispatch(
            @"{""id"":""4"",""method"":""system.ping""}",
            effects,
            locked);

        Assert.NotNull(v2);
        using JsonDocument doc = JsonDocument.Parse(v2);
        Assert.False(doc.RootElement.GetProperty("ok").GetBoolean());
        Assert.Equal("auth_required", doc.RootElement.GetProperty("error").GetProperty("code").GetString());
    }

    [Fact] // Covers U4 events.stream handoff signal.
    public void Events_stream_dispatch_returns_no_inline_response()
    {
        Assert.Null(CommandRouter.Dispatch("events.stream", new FakeSocketEffects(), AuthState.Unprotected));
        Assert.Null(CommandRouter.Dispatch(@"{""id"":""5"",""method"":""events.stream"",""params"":{}}", new FakeSocketEffects(), AuthState.Unprotected));
    }

    [Fact] // Covers V2 null-field hardening: JSON null text must not reach ISocketEffects.
    public void Surface_send_text_with_json_null_text_returns_invalid_params()
    {
        FakeSocketEffects effects = new();

        string? response = CommandRouter.Dispatch(
            @"{""id"":""10"",""method"":""surface.send_text"",""params"":{""surface_id"":""S4"",""text"":null}}",
            effects,
            AuthState.Unprotected);

        Assert.NotNull(response);
        using JsonDocument doc = JsonDocument.Parse(response);
        Assert.False(doc.RootElement.GetProperty("ok").GetBoolean());
        Assert.Equal("invalid_params", doc.RootElement.GetProperty("error").GetProperty("code").GetString());
        Assert.Equal(string.Empty, effects.SentText);
    }

    [Fact] // Covers V2 missing-field hardening.
    public void Surface_send_text_with_missing_text_returns_invalid_params()
    {
        string? response = CommandRouter.Dispatch(
            @"{""id"":""11"",""method"":""surface.send_text"",""params"":{""surface_id"":""S4""}}",
            new FakeSocketEffects(),
            AuthState.Unprotected);

        Assert.NotNull(response);
        using JsonDocument doc = JsonDocument.Parse(response);
        Assert.False(doc.RootElement.GetProperty("ok").GetBoolean());
        Assert.Equal("invalid_params", doc.RootElement.GetProperty("error").GetProperty("code").GetString());
    }

    [Fact] // Covers V2 null-field hardening on set-status.
    public void Set_status_with_json_null_status_returns_invalid_params()
    {
        string? response = CommandRouter.Dispatch(
            @"{""id"":""12"",""method"":""set-status"",""params"":{""status"":null}}",
            new FakeSocketEffects(),
            AuthState.Unprotected);

        Assert.NotNull(response);
        using JsonDocument doc = JsonDocument.Parse(response);
        Assert.False(doc.RootElement.GetProperty("ok").GetBoolean());
        Assert.Equal("invalid_params", doc.RootElement.GetProperty("error").GetProperty("code").GetString());
    }

    [Fact] // Covers V2 null-field hardening on report_pwd.
    public void Report_pwd_with_json_null_path_returns_invalid_params()
    {
        string? response = CommandRouter.Dispatch(
            @"{""id"":""13"",""method"":""report_pwd"",""params"":{""surface_id"":""S2"",""path"":null}}",
            new FakeSocketEffects(),
            AuthState.Unprotected);

        Assert.NotNull(response);
        using JsonDocument doc = JsonDocument.Parse(response);
        Assert.False(doc.RootElement.GetProperty("ok").GetBoolean());
        Assert.Equal("invalid_params", doc.RootElement.GetProperty("error").GetProperty("code").GetString());
    }

    [Fact] // Covers V2 optional-field fallback: null title/subtitle/body degrade to empty strings, never null.
    public void Notify_v2_with_json_null_optional_fields_falls_back_to_empty_strings()
    {
        FakeSocketEffects effects = new();

        string? response = CommandRouter.Dispatch(
            @"{""id"":""14"",""method"":""notify"",""params"":{""surface_id"":""S3"",""title"":null,""subtitle"":null,""body"":null}}",
            effects,
            AuthState.Unprotected);

        Assert.NotNull(response);
        using JsonDocument doc = JsonDocument.Parse(response);
        Assert.True(doc.RootElement.GetProperty("ok").GetBoolean());
        Assert.Equal(new SurfaceId(3), effects.CreatedForSurface);
        Assert.Equal(string.Empty, effects.CreatedTitle);
        Assert.Equal(string.Empty, effects.CreatedSubtitle);
        Assert.Equal(string.Empty, effects.CreatedBody);
    }

    [Fact] // Covers V2 create_for_caller: absent/null preferred_surface_id passes null, fields degrade to empty.
    public void Create_for_caller_v2_with_json_null_fields_passes_null_surface_and_empty_strings()
    {
        FakeSocketEffects effects = new();

        string? response = CommandRouter.Dispatch(
            @"{""id"":""15"",""method"":""notification.create_for_caller"",""params"":{""preferred_surface_id"":null,""title"":null}}",
            effects,
            AuthState.Unprotected);

        Assert.NotNull(response);
        using JsonDocument doc = JsonDocument.Parse(response);
        Assert.True(doc.RootElement.GetProperty("ok").GetBoolean());
        Assert.Null(effects.CallerPreferredSurface);
        Assert.Equal(string.Empty, effects.CallerTitle);
        Assert.Equal(string.Empty, effects.CallerSubtitle);
        Assert.Equal(string.Empty, effects.CallerBody);
    }

    [Fact] // Covers V2 auth.login: null credential degrades to empty string and fails auth instead of throwing.
    public void Auth_login_with_json_null_credential_fails_cleanly()
    {
        string? response = CommandRouter.Dispatch(
            @"{""id"":""16"",""method"":""auth.login"",""params"":{""credential"":null}}",
            new FakeSocketEffects(),
            AuthState.Unprotected);

        Assert.NotNull(response);
        using JsonDocument doc = JsonDocument.Parse(response);
        Assert.True(doc.RootElement.GetProperty("ok").GetBoolean());
        Assert.False(doc.RootElement.GetProperty("result").GetProperty("authorized").GetBoolean());
    }

    private sealed class FakeSocketEffects : ISocketEffects
    {
        public SurfaceId CreatedForSurface;
        public string CreatedForWorkspace = string.Empty;
        public string CreatedTitle = string.Empty;
        public string CreatedSubtitle = string.Empty;
        public string CreatedBody = string.Empty;
        public string? CallerPreferredSurface;
        public string CallerTitle = string.Empty;
        public string CallerSubtitle = string.Empty;
        public string CallerBody = string.Empty;
        public SurfaceId MarkReadSurface;
        public bool MarkAllRead;
        public bool DismissAllReadCalled;

        public SurfaceId SentSurface;
        public string SentText = string.Empty;

        public string Capabilities() => "v1,v2";
        public bool Authenticate(string credential) => credential == "ok";
        public TreeSnapshot Tree() => new(new PaneLeaf(new PaneId(1), ImmutableList.Create(new SurfaceId(1)), new SurfaceId(1)), new PaneId(1), null, 1);
        public string Ping() => "pong";
        public void FocusSurface(SurfaceId surface) {}
        public void SendText(SurfaceId surface, string text)
        {
            SentSurface = surface;
            SentText = text;
        }

        public void SendKey(SurfaceId surface, uint virtualKey, uint modifiers) {}
        public void CreateNotificationForTarget(string workspaceId, SurfaceId surface, string title, string subtitle, string body)
        {
            CreatedForWorkspace = workspaceId;
            CreatedForSurface = surface;
            CreatedTitle = title;
            CreatedSubtitle = subtitle;
            CreatedBody = body;
        }

        public void CreateNotification(string title, string subtitle, string body)
        {
            CallerTitle = title;
            CallerSubtitle = subtitle;
            CallerBody = body;
        }

        public void CreateNotificationForCaller(string? preferredSurfaceId, string title, string subtitle, string body)
        {
            CallerPreferredSurface = preferredSurfaceId;
            CallerTitle = title;
            CallerSubtitle = subtitle;
            CallerBody = body;
        }

        public IReadOnlyList<TerminalNotification> NotificationList() => [];
        public void NotificationDismiss(Guid notificationId) {}
        public void NotificationDismissAllRead() => DismissAllReadCalled = true;
        public void NotificationDismissForSurface(SurfaceId surface) {}
        public void NotificationClear() {}
        public void NotificationMarkRead(Guid notificationId) {}
        public void NotificationMarkRead(SurfaceId surface) => MarkReadSurface = surface;
        public void NotificationMarkAllRead() => MarkAllRead = true;
        public void NotificationOpen(Guid notificationId) {}
        public bool JumpToUnread() => true;
        public void SetStatus(string status) {}
        public void SetProgress(string progress) {}
        public void LogLine(string line) {}
        public void SidebarState(string payload) {}
        public void ReportGitBranch(SurfaceId surface, string branch, bool isDirty) {}
        public void ReportPr(SurfaceId surface, string number, string label, string status, string? branch, bool isStale) {}
        public void ReportPwd(SurfaceId surface, string path) {}
    }
}
