using System;
using System.Collections.Generic;
using System.Text.Json;
using Optimus.Cli;
using Xunit;

namespace Optimus.Core.Tests;

/// <summary>
/// Phase 4 U4: argv → wire-frame parsing. Asserts on the exact V2 JSON the CLI will write to the
/// pipe, using the param names CommandRouter reads on the other end.
/// </summary>
public sealed class CliParserTests
{
    private static Func<string, string?> Env(params (string Key, string Value)[] pairs) =>
        key =>
        {
            foreach ((string k, string v) in pairs)
            {
                if (k == key)
                {
                    return v;
                }
            }

            return null;
        };

    private static CliInvocation ParseOk(string[] args, Func<string, string?>? env = null, string? stdin = null)
    {
        object result = CliParser.Parse(args, env ?? (_ => null), stdin);
        return Assert.IsType<CliInvocation>(result);
    }

    private static JsonElement SingleFrame(CliInvocation invocation)
    {
        string frame = Assert.Single(invocation.Frames);
        return JsonDocument.Parse(frame).RootElement;
    }

    [Fact] // Plan acceptance: `optimus notify --title X --body Y` from inside a pane targets the caller.
    public void Notify_without_surface_routes_to_create_for_caller_with_env_surface()
    {
        CliInvocation inv = ParseOk(
            ["notify", "--title", "Build done", "--body", "All green"],
            Env((CliParser.SurfaceIdEnv, "S7")));

        JsonElement root = SingleFrame(inv);
        Assert.Equal("notification.create_for_caller", root.GetProperty("method").GetString());
        JsonElement p = root.GetProperty("params");
        Assert.Equal("Build done", p.GetProperty("title").GetString());
        Assert.Equal("All green", p.GetProperty("body").GetString());
        Assert.Equal("S7", p.GetProperty("preferred_surface_id").GetString());
    }

    [Fact]
    public void Notify_with_explicit_surface_routes_to_targeted_notify()
    {
        CliInvocation inv = ParseOk(
            ["notify", "--title", "T", "--workspace", "ws1", "--surface", "S3"]);

        JsonElement root = SingleFrame(inv);
        Assert.Equal("notify", root.GetProperty("method").GetString());
        JsonElement p = root.GetProperty("params");
        Assert.Equal("S3", p.GetProperty("surface_id").GetString());
        Assert.Equal("ws1", p.GetProperty("workspace_id").GetString());
    }

    [Fact]
    public void Notify_without_title_is_an_error()
    {
        object result = CliParser.Parse(["notify", "--body", "B"], _ => null);
        Assert.IsType<CliError>(result);
    }

    [Fact]
    public void Send_builds_surface_send_text()
    {
        CliInvocation inv = ParseOk(["send", "S2", "echo", "hi"]);

        JsonElement root = SingleFrame(inv);
        Assert.Equal("surface.send_text", root.GetProperty("method").GetString());
        Assert.Equal("S2", root.GetProperty("params").GetProperty("surface_id").GetString());
        Assert.Equal("echo hi", root.GetProperty("params").GetProperty("text").GetString());
    }

    [Fact]
    public void SendKey_builds_surface_send_key_with_modifiers()
    {
        CliInvocation inv = ParseOk(["send-key", "S2", "13", "4"]);

        JsonElement root = SingleFrame(inv);
        Assert.Equal("surface.send_key", root.GetProperty("method").GetString());
        Assert.Equal(13u, root.GetProperty("params").GetProperty("key").GetUInt32());
        Assert.Equal(4u, root.GetProperty("params").GetProperty("modifiers").GetUInt32());
    }

    [Fact] // Plan acceptance: `optimus report_git_branch main --status=dirty` updates app state.
    public void ReportGitBranch_uses_env_surface_and_dirty_flag()
    {
        CliInvocation inv = ParseOk(
            ["report_git_branch", "main", "--status=dirty"],
            Env((CliParser.SurfaceIdEnv, "S4")));

        JsonElement root = SingleFrame(inv);
        Assert.Equal("report_git_branch", root.GetProperty("method").GetString());
        JsonElement p = root.GetProperty("params");
        Assert.Equal("S4", p.GetProperty("surface_id").GetString());
        Assert.Equal("main", p.GetProperty("branch").GetString());
        Assert.True(p.GetProperty("is_dirty").GetBoolean());
    }

    [Fact]
    public void ReportGitBranch_without_surface_or_env_is_an_error()
    {
        object result = CliParser.Parse(["report_git_branch", "main"], _ => null);
        Assert.IsType<CliError>(result);
    }

    [Fact]
    public void ReportPr_carries_all_fields()
    {
        CliInvocation inv = ParseOk(
            ["report_pr", "42", "--label", "feat", "--url", "https://x/pr/42",
             "--pr-status", "open", "--branch", "feat/x", "--stale", "--surface", "S1"]);

        JsonElement p = SingleFrame(inv).GetProperty("params");
        Assert.Equal("42", p.GetProperty("number").GetString());
        Assert.Equal("feat", p.GetProperty("label").GetString());
        Assert.Equal("https://x/pr/42", p.GetProperty("url").GetString());
        Assert.Equal("open", p.GetProperty("status").GetString());
        Assert.Equal("feat/x", p.GetProperty("branch").GetString());
        Assert.True(p.GetProperty("is_stale").GetBoolean());
    }

    [Fact]
    public void ReportPwd_builds_path_payload()
    {
        CliInvocation inv = ParseOk(["report_pwd", @"C:\dev\x", "--surface", "S9"]);

        JsonElement root = SingleFrame(inv);
        Assert.Equal("report_pwd", root.GetProperty("method").GetString());
        Assert.Equal(@"C:\dev\x", root.GetProperty("params").GetProperty("path").GetString());
    }

    [Fact]
    public void DismissNotification_by_id_surface_and_scope()
    {
        JsonElement byId = SingleFrame(ParseOk(["dismiss-notification", "0b5c1f2a-0000-0000-0000-000000000001"]));
        Assert.Equal("0b5c1f2a-0000-0000-0000-000000000001", byId.GetProperty("params").GetProperty("id").GetString());

        JsonElement bySurface = SingleFrame(ParseOk(["dismiss-notification", "--surface", "S2"]));
        Assert.Equal("S2", bySurface.GetProperty("params").GetProperty("surface_id").GetString());

        JsonElement byScope = SingleFrame(ParseOk(["dismiss-notification", "--all-read"]));
        Assert.Equal("all_read", byScope.GetProperty("params").GetProperty("scope").GetString());
    }

    [Fact]
    public void MarkNotification_all_uses_scope_all()
    {
        JsonElement root = SingleFrame(ParseOk(["mark-notification", "--all"]));
        Assert.Equal("notification.mark_read", root.GetProperty("method").GetString());
        Assert.Equal("all", root.GetProperty("params").GetProperty("scope").GetString());
    }

    [Fact]
    public void SetStatus_joins_remaining_args()
    {
        JsonElement root = SingleFrame(ParseOk(["set-status", "codex:", "running", "tests"]));
        Assert.Equal("set-status", root.GetProperty("method").GetString());
        Assert.Equal("codex: running tests", root.GetProperty("params").GetProperty("status").GetString());
    }

    [Fact]
    public void Global_socket_and_variant_flags_are_extracted()
    {
        CliInvocation inv = ParseOk(["--socket", @"\\.\pipe\optimus-dev", "--variant", "dev", "ping"]);

        Assert.Equal(@"\\.\pipe\optimus-dev", inv.ExplicitSocket);
        Assert.Equal("dev", inv.Variant);
        Assert.Equal("system.ping", SingleFrame(inv).GetProperty("method").GetString());
    }

    [Fact]
    public void No_args_prints_usage_with_no_frames()
    {
        CliInvocation inv = ParseOk([]);
        Assert.Empty(inv.Frames);
        Assert.Contains("usage:", inv.StdOut);
    }

    [Fact]
    public void Unknown_verb_is_an_error()
    {
        object result = CliParser.Parse(["frobnicate"], _ => null);
        CliError error = Assert.IsType<CliError>(result);
        Assert.Contains("unknown command", error.Message);
    }

    [Fact]
    public void AuthLogin_builds_credential_payload()
    {
        JsonElement root = SingleFrame(ParseOk(["auth", "login", "--password", "hunter2"]));
        Assert.Equal("auth.login", root.GetProperty("method").GetString());
        Assert.Equal("hunter2", root.GetProperty("params").GetProperty("credential").GetString());
    }
}
