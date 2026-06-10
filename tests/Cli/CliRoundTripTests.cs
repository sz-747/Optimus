using System;
using System.Collections.Generic;
using System.Text.Json;
using Optimus.Cli;
using Xunit;

namespace Optimus.Core.Tests;

/// <summary>
/// Phase 4 integration seam: every frame the CLI builds must be accepted by the real
/// <see cref="CommandRouter"/> and land in the right <see cref="ISocketEffects"/> call. This is
/// the contract test that keeps the two sides of the pipe from drifting apart.
/// </summary>
public sealed class CliRoundTripTests
{
    private sealed class RecordingEffects : ISocketEffects
    {
        public readonly List<string> Calls = [];

        public TreeSnapshot Tree() => throw new NotSupportedException();
        public string Ping() { Calls.Add("ping"); return "pong"; }
        public string Capabilities() { Calls.Add("capabilities"); return "v1,v2"; }
        public bool Authenticate(string credential) { Calls.Add($"auth:{credential}"); return true; }
        public void FocusSurface(SurfaceId surface) => Calls.Add($"focus:{surface.Value}");
        public void SendText(SurfaceId surface, string text) => Calls.Add($"send_text:{surface.Value}:{text}");
        public void SendKey(SurfaceId surface, uint virtualKey, uint modifiers) => Calls.Add($"send_key:{surface.Value}:{virtualKey}:{modifiers}");
        public void CreateNotificationForTarget(string workspaceId, SurfaceId surface, string title, string subtitle, string body) =>
            Calls.Add($"notify_target:{workspaceId}:{surface.Value}:{title}:{body}");
        public void CreateNotification(string title, string subtitle, string body) => Calls.Add($"notify:{title}");
        public void CreateNotificationForCaller(string? preferredSurfaceId, string title, string subtitle, string body) =>
            Calls.Add($"notify_caller:{preferredSurfaceId}:{title}:{body}");
        public IReadOnlyList<TerminalNotification> NotificationList() { Calls.Add("list"); return []; }
        public void NotificationDismiss(Guid notificationId) => Calls.Add($"dismiss:{notificationId}");
        public void NotificationDismissForSurface(SurfaceId surface) => Calls.Add($"dismiss_surface:{surface.Value}");
        public void NotificationDismissAllRead() => Calls.Add("dismiss_all_read");
        public void NotificationClear() => Calls.Add("clear");
        public void NotificationMarkRead(Guid notificationId) => Calls.Add($"mark:{notificationId}");
        public void NotificationMarkRead(SurfaceId surface) => Calls.Add($"mark_surface:{surface.Value}");
        public void NotificationMarkAllRead() => Calls.Add("mark_all");
        public void NotificationOpen(Guid notificationId) => Calls.Add($"open:{notificationId}");
        public bool JumpToUnread() { Calls.Add("jump"); return true; }
        public void SetStatus(string status) => Calls.Add($"status:{status}");
        public void SetProgress(string progress) => Calls.Add($"progress:{progress}");
        public void LogLine(string line) => Calls.Add($"log:{line}");
        public void SidebarState(string payload) => Calls.Add("sidebar");
        public void ReportGitBranch(SurfaceId surface, string branch, bool isDirty) =>
            Calls.Add($"git:{surface.Value}:{branch}:{isDirty}");
        public void ReportPr(SurfaceId surface, string number, string label, string status, string? branch, bool isStale) =>
            Calls.Add($"pr:{surface.Value}:{number}:{status}");
        public void ReportPwd(SurfaceId surface, string path) => Calls.Add($"pwd:{surface.Value}:{path}");
    }

    private static (RecordingEffects Effects, string Response) Run(string[] args, Func<string, string?>? env = null, string? stdin = null)
    {
        object parsed = CliParser.Parse(args, env ?? (_ => null), stdin);
        var invocation = Assert.IsType<CliInvocation>(parsed);
        string frame = Assert.Single(invocation.Frames);

        var effects = new RecordingEffects();
        string? response = CommandRouter.Dispatch(frame, effects, AuthState.Unprotected);
        Assert.NotNull(response);
        return (effects, response!);
    }

    private static void AssertOk(string response)
    {
        JsonElement root = JsonDocument.Parse(response).RootElement;
        Assert.True(root.GetProperty("ok").GetBoolean(), $"server rejected the CLI frame: {response}");
    }

    private static Func<string, string?> Surface(string id) => key => key == CliParser.SurfaceIdEnv ? id : null;

    [Fact]
    public void Notify_for_caller_reaches_create_for_caller_effect()
    {
        (RecordingEffects effects, string response) = Run(
            ["notify", "--title", "T", "--body", "B"], Surface("S7"));

        AssertOk(response);
        Assert.Equal("notify_caller:S7:T:B", Assert.Single(effects.Calls));
    }

    [Fact]
    public void Targeted_notify_reaches_create_for_target_effect()
    {
        (RecordingEffects effects, string response) = Run(
            ["notify", "--title", "T", "--body", "B", "--workspace", "ws1", "--surface", "S3"]);

        AssertOk(response);
        Assert.Equal("notify_target:ws1:3:T:B", Assert.Single(effects.Calls));
    }

    [Fact]
    public void Send_reaches_send_text_effect()
    {
        (RecordingEffects effects, string response) = Run(["send", "S2", "echo hi"]);

        AssertOk(response);
        Assert.Equal("send_text:2:echo hi", Assert.Single(effects.Calls));
    }

    [Fact]
    public void SendKey_reaches_send_key_effect()
    {
        (RecordingEffects effects, string response) = Run(["send-key", "S2", "13", "4"]);

        AssertOk(response);
        Assert.Equal("send_key:2:13:4", Assert.Single(effects.Calls));
    }

    [Fact]
    public void ReportGitBranch_reaches_git_effect()
    {
        (RecordingEffects effects, string response) = Run(
            ["report_git_branch", "main", "--status=dirty"], Surface("S4"));

        AssertOk(response);
        Assert.Equal("git:4:main:True", Assert.Single(effects.Calls));
    }

    [Fact]
    public void ReportPr_and_pwd_reach_their_effects()
    {
        (RecordingEffects pr, string prResponse) = Run(
            ["report_pr", "42", "--pr-status", "open", "--surface", "S1"]);
        AssertOk(prResponse);
        Assert.Equal("pr:1:42:open", Assert.Single(pr.Calls));

        (RecordingEffects pwd, string pwdResponse) = Run(
            ["report_pwd", @"C:\dev\x", "--surface", "S9"]);
        AssertOk(pwdResponse);
        Assert.Equal(@"pwd:9:C:\dev\x", Assert.Single(pwd.Calls));
    }

    [Fact]
    public void Notification_actions_reach_their_effects()
    {
        var id = "0b5c1f2a-0000-0000-0000-000000000001";

        (RecordingEffects dismiss, string r1) = Run(["dismiss-notification", id]);
        AssertOk(r1);
        Assert.Equal($"dismiss:{id}", Assert.Single(dismiss.Calls));

        (RecordingEffects mark, string r2) = Run(["mark-notification", "--all"]);
        AssertOk(r2);
        Assert.Equal("mark_all", Assert.Single(mark.Calls));

        (RecordingEffects open, string r3) = Run(["open-notification", id]);
        AssertOk(r3);
        Assert.Equal($"open:{id}", Assert.Single(open.Calls));

        (RecordingEffects jump, string r4) = Run(["jump-to-unread"]);
        AssertOk(r4);
        Assert.Equal("jump", Assert.Single(jump.Calls));
    }

    [Fact]
    public void Status_progress_and_log_reach_their_effects()
    {
        (RecordingEffects status, string r1) = Run(["set-status", "busy"]);
        AssertOk(r1);
        Assert.Equal("status:busy", Assert.Single(status.Calls));

        (RecordingEffects progress, string r2) = Run(["set-progress", "3/5"]);
        AssertOk(r2);
        Assert.Equal("progress:3/5", Assert.Single(progress.Calls));

        (RecordingEffects log, string r3) = Run(["log", "hello world"]);
        AssertOk(r3);
        Assert.Equal("log:hello world", Assert.Single(log.Calls));
    }

    [Fact] // Plan acceptance: an agent hook fires a notification on stop.
    public void Claude_stop_hook_lands_a_caller_notification()
    {
        (RecordingEffects effects, string response) = Run(
            ["hooks", "claude", "stop"], Surface("S5"), stdin: "{\"message\":\"Done refactoring\"}");

        AssertOk(response);
        Assert.Equal("notify_caller:S5:Claude Code:Done refactoring", Assert.Single(effects.Calls));
    }

    [Fact]
    public void AuthLogin_reaches_authenticate_even_when_auth_required()
    {
        object parsed = CliParser.Parse(["auth", "login", "--password", "pw"], _ => null);
        var invocation = Assert.IsType<CliInvocation>(parsed);
        string frame = Assert.Single(invocation.Frames);

        var effects = new RecordingEffects();
        string? response = CommandRouter.Dispatch(
            frame, effects, new AuthState(RequiresAuthentication: true, IsAuthenticated: false));

        Assert.NotNull(response);
        JsonElement root = JsonDocument.Parse(response!).RootElement;
        Assert.True(root.GetProperty("ok").GetBoolean());
        Assert.Equal("auth:pw", Assert.Single(effects.Calls));
    }
}
