using System;
using System.Collections.Generic;
using System.Linq;
using System.Text.Json;

namespace Optimus.Cli;

/// <summary>
/// Agent-hook support (plan Phase 4 U5): the runtime verb installed hooks call
/// (<c>optimus hooks &lt;agent&gt; &lt;event&gt;</c>) plus the installer/printer that emits the
/// <c>.ps1</c>/<c>.cmd</c> snippets agents are configured to run. Pure: returns frames/file
/// writes; Program does the I/O.
/// </summary>
public static class HooksCommand
{
    /// <summary>One supported agent integration (ported from the macOS AgentHookDef model).</summary>
    public sealed record AgentDef(string Name, string DisplayName, string StatusKey, string[] Aliases)
    {
        public bool Matches(string candidate) =>
            string.Equals(Name, candidate, StringComparison.OrdinalIgnoreCase)
            || Aliases.Any(a => string.Equals(a, candidate, StringComparison.OrdinalIgnoreCase));
    }

    public static readonly IReadOnlyList<AgentDef> Agents =
    [
        new("claude", "Claude Code", "claude", ["hermes", "claude-code"]),
        new("codex", "Codex", "codex", []),
        new("gemini", "Gemini", "gemini", []),
        new("cursor", "Cursor", "cursor", []),
        new("copilot", "Copilot", "copilot", []),
    ];

    public static readonly IReadOnlyList<string> Events =
    [
        "session-start", "prompt-submit", "stop", "notification", "session-end", "session-finalize",
    ];

    public static object Parse(List<string> args, Func<string, string?> getEnv, string? stdin)
    {
        if (args.Count == 0)
        {
            return new CliError("hooks: expected `hooks <agent> <event>`, `hooks install <agent>`, or `hooks print <agent>`");
        }

        return args[0] switch
        {
            "install" => ParseInstall(args.GetRange(1, args.Count - 1)),
            "print" => ParsePrint(args.GetRange(1, args.Count - 1)),
            _ => ParseRuntime(args, getEnv, stdin),
        };
    }

    // ---- runtime: `optimus hooks <agent> <event>` -------------------------------------------------

    private static object ParseRuntime(List<string> args, Func<string, string?> getEnv, string? stdin)
    {
        if (args.Count < 2)
        {
            return new CliError("hooks: expected `hooks <agent> <event>`");
        }

        AgentDef? agent = Find(args[0]);
        if (agent is null)
        {
            return new CliError($"hooks: unknown agent \"{args[0]}\" (known: {string.Join(", ", Agents.Select(a => a.Name))})");
        }

        string hookEvent = args[1];
        if (!Events.Contains(hookEvent, StringComparer.OrdinalIgnoreCase))
        {
            return new CliError($"hooks: unknown event \"{hookEvent}\" (known: {string.Join(", ", Events)})");
        }

        // Hooks are gated on the surface id shell integration injects: outside a optimus pane the
        // hook is a silent no-op (exit 0, nothing sent) so agents work unchanged elsewhere.
        string? surfaceId = getEnv(CliParser.SurfaceIdEnv);
        if (string.IsNullOrEmpty(surfaceId))
        {
            return new CliInvocation([]);
        }

        return new CliInvocation(BuildRuntimeFrames(agent, hookEvent.ToLowerInvariant(), surfaceId, stdin));
    }

    /// <summary>
    /// Maps a lifecycle event to wire frames. stop/notification raise a caller-targeted
    /// notification (macOS parity: title = agent display name); the rest update agent status.
    /// </summary>
    public static IReadOnlyList<string> BuildRuntimeFrames(AgentDef agent, string hookEvent, string surfaceId, string? stdin)
    {
        switch (hookEvent)
        {
            case "stop":
            case "notification":
            {
                string body = ExtractMessage(stdin) ?? (hookEvent == "stop" ? "Agent finished" : "Agent needs attention");
                return [CliParser.V2("notification.create_for_caller", w =>
                {
                    w.WriteString("title", agent.DisplayName);
                    w.WriteString("subtitle", string.Empty);
                    w.WriteString("body", body);
                    w.WriteString("preferred_surface_id", surfaceId);
                })];
            }
            case "session-start":
                return [StatusFrame(agent, "start")];
            case "prompt-submit":
                return [StatusFrame(agent, "busy")];
            case "session-end":
            case "session-finalize":
                return [StatusFrame(agent, "idle")];
            default:
                return [];
        }
    }

    private static string StatusFrame(AgentDef agent, string state) =>
        CliParser.V2("set-status", w => w.WriteString("status", $"{agent.StatusKey}:{state}"));

    /// <summary>Pulls a human-readable message out of the hook's stdin JSON, if any.</summary>
    public static string? ExtractMessage(string? stdin)
    {
        if (string.IsNullOrWhiteSpace(stdin))
        {
            return null;
        }

        try
        {
            using JsonDocument doc = JsonDocument.Parse(stdin);
            if (doc.RootElement.ValueKind != JsonValueKind.Object)
            {
                return null;
            }

            foreach (string field in (string[])["message", "body", "title"])
            {
                if (doc.RootElement.TryGetProperty(field, out JsonElement el)
                    && el.ValueKind == JsonValueKind.String
                    && !string.IsNullOrWhiteSpace(el.GetString()))
                {
                    return el.GetString();
                }
            }
        }
        catch (JsonException)
        {
            // Hook stdin isn't required to be JSON; fall through to the default body.
        }

        return null;
    }

    // ---- installer: `optimus hooks install <agent>` / `optimus hooks print <agent>` ------------------

    private static object ParseInstall(List<string> args)
    {
        if (args.Count == 0)
        {
            return new CliError("hooks install: expected <agent>");
        }

        AgentDef? agent = Find(args[0]);
        if (agent is null)
        {
            return new CliError($"hooks install: unknown agent \"{args[0]}\"");
        }

        string? dir = null;
        for (int i = 1; i < args.Count; i++)
        {
            if (args[i] == "--dir" && i + 1 < args.Count) dir = args[++i];
        }

        string baseName = $"optimus-{agent.Name}-hook";
        var writes = new List<LocalFileWrite>
        {
            new($"{baseName}.ps1", Snippet(agent, "ps1")),
            new($"{baseName}.cmd", Snippet(agent, "cmd")),
        };

        return new CliInvocation(
            [],
            FileWrites: writes,
            StdOut: $"installed {baseName}.ps1 and {baseName}.cmd",
            InstallDir: dir);
    }

    private static object ParsePrint(List<string> args)
    {
        if (args.Count == 0)
        {
            return new CliError("hooks print: expected <agent>");
        }

        AgentDef? agent = Find(args[0]);
        if (agent is null)
        {
            return new CliError($"hooks print: unknown agent \"{args[0]}\"");
        }

        string format = "ps1";
        for (int i = 1; i < args.Count; i++)
        {
            if (args[i] == "--format" && i + 1 < args.Count) format = args[++i];
            else if (args[i].StartsWith("--format=", StringComparison.Ordinal)) format = args[i]["--format=".Length..];
        }

        if (format is not ("ps1" or "cmd"))
        {
            return new CliError("hooks print: --format must be ps1 or cmd");
        }

        return new CliInvocation([], StdOut: Snippet(agent, format));
    }

    /// <summary>
    /// The hook snippet an agent is configured to execute. Forwards its stdin (hook payload JSON)
    /// to the optimus runtime verb and never fails the agent (always exits 0). The OPTIMUS_SURFACE_ID
    /// gate is duplicated here so the snippet is a no-op outside optimus even before optimus.exe runs.
    /// </summary>
    public static string Snippet(AgentDef agent, string format) => format switch
    {
        "ps1" => string.Join("\r\n",
            $"# optimus {agent.Name} hook (generated by `optimus hooks install {agent.Name}`)",
            "param([Parameter(Mandatory = $true)][string]$HookEvent)",
            "if ([string]::IsNullOrEmpty($env:OPTIMUS_SURFACE_ID)) { exit 0 }",
            $"$input | & optimus hooks {agent.Name} $HookEvent",
            "exit 0",
            ""),
        "cmd" => string.Join("\r\n",
            "@echo off",
            $"rem optimus {agent.Name} hook (generated by `optimus hooks install {agent.Name}`)",
            "if \"%OPTIMUS_SURFACE_ID%\"==\"\" exit /b 0",
            $"optimus hooks {agent.Name} %1",
            "exit /b 0",
            ""),
        _ => throw new ArgumentOutOfRangeException(nameof(format)),
    };

    public static AgentDef? Find(string name) => Agents.FirstOrDefault(a => a.Matches(name));
}
