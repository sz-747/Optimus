using System;
using System.Collections.Generic;
using System.Linq;
using System.Text.Json;
using Cmux.Cli;
using Xunit;

namespace Cmux.Core.Tests;

/// <summary>
/// Phase 4 U5: agent hook runtime verb + snippet generation. Plan acceptance: "an agent hook
/// fires a notification on stop", gated on CMUX_SURFACE_ID.
/// </summary>
public sealed class HooksCommandTests
{
    private static Func<string, string?> WithSurface(string id) =>
        key => key == CliParser.SurfaceIdEnv ? id : null;

    private static CliInvocation ParseOk(string[] args, Func<string, string?>? env = null, string? stdin = null)
    {
        object result = CliParser.Parse(args, env ?? (_ => null), stdin);
        return Assert.IsType<CliInvocation>(result);
    }

    [Fact]
    public void Stop_hook_fires_caller_notification_with_agent_display_name()
    {
        CliInvocation inv = ParseOk(["hooks", "claude", "stop"], WithSurface("S5"));

        string frame = Assert.Single(inv.Frames);
        JsonElement root = JsonDocument.Parse(frame).RootElement;
        Assert.Equal("notification.create_for_caller", root.GetProperty("method").GetString());
        JsonElement p = root.GetProperty("params");
        Assert.Equal("Claude Code", p.GetProperty("title").GetString());
        Assert.Equal("S5", p.GetProperty("preferred_surface_id").GetString());
        Assert.False(string.IsNullOrEmpty(p.GetProperty("body").GetString()));
    }

    [Fact]
    public void Stop_hook_prefers_message_from_stdin_json()
    {
        CliInvocation inv = ParseOk(
            ["hooks", "claude", "stop"],
            WithSurface("S5"),
            stdin: "{\"message\":\"Refactor finished, 3 files changed\"}");

        JsonElement p = JsonDocument.Parse(inv.Frames[0]).RootElement.GetProperty("params");
        Assert.Equal("Refactor finished, 3 files changed", p.GetProperty("body").GetString());
    }

    [Fact] // The CMUX_SURFACE_ID gate: outside a cmux pane hooks are silent no-ops.
    public void Hook_without_surface_env_sends_nothing()
    {
        CliInvocation inv = ParseOk(["hooks", "claude", "stop"]);
        Assert.Empty(inv.Frames);
    }

    [Fact]
    public void Agent_aliases_resolve()
    {
        CliInvocation inv = ParseOk(["hooks", "hermes", "stop"], WithSurface("S1"));
        JsonElement p = JsonDocument.Parse(inv.Frames[0]).RootElement.GetProperty("params");
        Assert.Equal("Claude Code", p.GetProperty("title").GetString());
    }

    [Fact]
    public void Unknown_agent_and_unknown_event_are_errors()
    {
        Assert.IsType<CliError>(CliParser.Parse(["hooks", "clippy", "stop"], _ => null));
        Assert.IsType<CliError>(CliParser.Parse(["hooks", "claude", "explode"], _ => null));
    }

    [Fact]
    public void Lifecycle_events_map_to_status_updates()
    {
        foreach ((string hookEvent, string expected) in new[]
        {
            ("session-start", "claude:start"),
            ("prompt-submit", "claude:busy"),
            ("session-end", "claude:idle"),
            ("session-finalize", "claude:idle"),
        })
        {
            CliInvocation inv = ParseOk(["hooks", "claude", hookEvent], WithSurface("S1"));
            JsonElement root = JsonDocument.Parse(Assert.Single(inv.Frames)).RootElement;
            Assert.Equal("set-status", root.GetProperty("method").GetString());
            Assert.Equal(expected, root.GetProperty("params").GetProperty("status").GetString());
        }
    }

    [Fact]
    public void Install_emits_ps1_and_cmd_snippets()
    {
        CliInvocation inv = ParseOk(["hooks", "install", "codex", "--dir", @"C:\tmp\hooks"]);

        Assert.NotNull(inv.FileWrites);
        Assert.Equal(2, inv.FileWrites!.Count);
        Assert.Equal(@"C:\tmp\hooks", inv.InstallDir);
        Assert.Contains(inv.FileWrites, w => w.RelativePath == "cmux-codex-hook.ps1");
        Assert.Contains(inv.FileWrites, w => w.RelativePath == "cmux-codex-hook.cmd");
        Assert.Empty(inv.Frames);
    }

    [Fact]
    public void Ps1_snippet_gates_on_surface_env_and_forwards_stdin()
    {
        string snippet = HooksCommand.Snippet(HooksCommand.Find("gemini")!, "ps1");

        Assert.Contains("$env:CMUX_SURFACE_ID", snippet);
        Assert.Contains("cmux hooks gemini", snippet);
        Assert.Contains("$input |", snippet);
        Assert.Contains("exit 0", snippet);
    }

    [Fact]
    public void Cmd_snippet_gates_on_surface_env()
    {
        string snippet = HooksCommand.Snippet(HooksCommand.Find("cursor")!, "cmd");

        Assert.Contains("%CMUX_SURFACE_ID%", snippet);
        Assert.Contains("cmux hooks cursor", snippet);
        Assert.Contains("exit /b 0", snippet);
    }

    [Fact]
    public void Print_outputs_snippet_to_stdout()
    {
        CliInvocation inv = ParseOk(["hooks", "print", "copilot", "--format", "cmd"]);

        Assert.Empty(inv.Frames);
        Assert.Contains("cmux hooks copilot", inv.StdOut);
    }

    [Fact]
    public void All_planned_agents_are_covered()
    {
        string[] expected = ["claude", "codex", "gemini", "cursor", "copilot"];
        Assert.Equal(expected, HooksCommand.Agents.Select(a => a.Name).ToArray());
    }
}
