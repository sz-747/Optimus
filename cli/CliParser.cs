using System;
using System.Collections.Generic;
using System.IO;
using System.Text.Json;

namespace Cmux.Cli;

/// <summary>
/// What a parsed CLI invocation wants the process to do. Pure data so tests can assert on it
/// without touching a pipe: <see cref="Frames"/> are the exact newline-framed lines that will be
/// written to the socket (V2 JSON envelopes matching <c>CommandRouter</c>'s param contract).
/// </summary>
public sealed record CliInvocation(
    IReadOnlyList<string> Frames,
    string? ExplicitSocket = null,
    string? Variant = null,
    IReadOnlyList<LocalFileWrite>? FileWrites = null,
    string? StdOut = null,
    string? InstallDir = null);

/// <summary>A file the invocation wants written locally (hook installer output).</summary>
public sealed record LocalFileWrite(string RelativePath, string Content);

/// <summary>Parse failure with the message to print and the exit code to use.</summary>
public sealed record CliError(string Message, int ExitCode = 2);

/// <summary>
/// Pure argv → wire-frame parser for the cmux CLI (plan Phase 4 U4). No I/O: environment access
/// goes through the injected resolver so tests can drive CMUX_SURFACE_ID / socket env precedence.
/// </summary>
public static class CliParser
{
    public const string SurfaceIdEnv = "CMUX_SURFACE_ID";

    private static readonly string Usage = string.Join('\n',
        "usage: cmux [--socket <pipe>] [--variant <name>] <command> [options]",
        "",
        "commands:",
        "  notify --title <t> [--subtitle <s>] [--body <b>] [--workspace <w>] [--surface <S#>]",
        "  send <surface> <text…>",
        "  send-key <surface> <key> [modifiers]",
        "  list-notifications",
        "  dismiss-notification (<id> | --surface <S#> | --all-read)",
        "  mark-notification (<id> | --surface <S#> | --all)",
        "  open-notification <id>",
        "  jump-to-unread",
        "  report_git_branch <branch> [--status dirty|clean] [--surface <S#>]",
        "  report_pr <number> [--label <l>] [--url <u>] [--pr-status open|merged|closed] [--branch <b>] [--stale] [--surface <S#>]",
        "  report_pwd <path> [--surface <S#>]",
        "  set-status <status…>",
        "  set-progress <progress…>",
        "  log <message…>",
        "  ping | capabilities",
        "  auth login --password <p>",
        "  hooks <agent> <event>            (runtime; called by installed hooks)",
        "  hooks install <agent> [--dir <d>]",
        "  hooks print <agent> [--format ps1|cmd]");

    public static object Parse(string[] args, Func<string, string?> getEnv, string? stdin = null)
    {
        string? socket = null;
        string? variant = null;
        var rest = new List<string>();

        for (int i = 0; i < args.Length; i++)
        {
            string arg = args[i];
            if (arg is "--socket" && i + 1 < args.Length)
            {
                socket = args[++i];
            }
            else if (arg.StartsWith("--socket=", StringComparison.Ordinal))
            {
                socket = arg["--socket=".Length..];
            }
            else if (arg is "--variant" && i + 1 < args.Length)
            {
                variant = args[++i];
            }
            else if (arg.StartsWith("--variant=", StringComparison.Ordinal))
            {
                variant = arg["--variant=".Length..];
            }
            else
            {
                rest.Add(arg);
            }
        }

        if (rest.Count == 0 || rest[0] is "help" or "--help" or "-h")
        {
            return new CliInvocation([], socket, variant, StdOut: Usage);
        }

        string verb = rest[0];
        var tail = rest.GetRange(1, rest.Count - 1);

        object result = verb switch
        {
            "notify" => ParseNotify(tail, getEnv),
            "send" => ParseSend(tail),
            "send-key" or "send_key" => ParseSendKey(tail),
            "list-notifications" or "notification.list" => Single(V2("notification.list", w => { })),
            "dismiss-notification" or "notification.dismiss" => ParseNotificationIdVerb(tail, "notification.dismiss", allScope: "all_read", allFlag: "--all-read"),
            "mark-notification" or "notification.mark_read" => ParseNotificationIdVerb(tail, "notification.mark_read", allScope: "all", allFlag: "--all"),
            "open-notification" or "notification.open" => ParseOpenNotification(tail),
            "jump-to-unread" or "notification.jump_to_unread" => Single(V2("notification.jump_to_unread", w => { })),
            "report_git_branch" or "report-git-branch" => ParseReportGitBranch(tail, getEnv),
            "report_pr" or "report-pr" => ParseReportPr(tail, getEnv),
            "report_pwd" or "report-pwd" => ParseReportPwd(tail, getEnv),
            "set-status" or "set_status" => ParseSingleString(tail, "set-status", "status"),
            "set-progress" or "set_progress" => ParseSingleString(tail, "set-progress", "progress"),
            "log" => ParseSingleString(tail, "log", "message"),
            "ping" => Single(V2("system.ping", w => { })),
            "capabilities" => Single(V2("system.capabilities", w => { })),
            "auth" => ParseAuth(tail),
            "hooks" => HooksCommand.Parse(tail, getEnv, stdin),
            _ => new CliError($"unknown command \"{verb}\"\n{Usage}"),
        };

        if (result is CliInvocation inv)
        {
            return inv with { ExplicitSocket = socket, Variant = variant };
        }

        return result;
    }

    private static object ParseNotify(List<string> args, Func<string, string?> getEnv)
    {
        string? title = null, subtitle = null, body = null, workspace = null, surface = null;
        for (int i = 0; i < args.Count; i++)
        {
            switch (args[i])
            {
                case "--title" when i + 1 < args.Count: title = args[++i]; break;
                case "--subtitle" when i + 1 < args.Count: subtitle = args[++i]; break;
                case "--body" when i + 1 < args.Count: body = args[++i]; break;
                case "--workspace" when i + 1 < args.Count: workspace = args[++i]; break;
                case "--surface" or "--window" when i + 1 < args.Count: surface = args[++i]; break;
                default:
                    return new CliError($"notify: unexpected argument \"{args[i]}\"");
            }
        }

        if (string.IsNullOrEmpty(title))
        {
            return new CliError("notify: --title is required");
        }

        // Routing (plan U4): explicit surface → targeted notify; otherwise create_for_caller with
        // the surface id shell integration injected into the environment (Windows has no TTY).
        if (!string.IsNullOrEmpty(surface))
        {
            return Single(V2("notify", w =>
            {
                w.WriteString("surface_id", surface);
                if (!string.IsNullOrEmpty(workspace)) w.WriteString("workspace_id", workspace);
                w.WriteString("title", title);
                w.WriteString("subtitle", subtitle ?? string.Empty);
                w.WriteString("body", body ?? string.Empty);
            }));
        }

        string? callerSurface = getEnv(SurfaceIdEnv);
        return Single(V2("notification.create_for_caller", w =>
        {
            w.WriteString("title", title);
            w.WriteString("subtitle", subtitle ?? string.Empty);
            w.WriteString("body", body ?? string.Empty);
            if (!string.IsNullOrEmpty(callerSurface)) w.WriteString("preferred_surface_id", callerSurface);
        }));
    }

    private static object ParseSend(List<string> args)
    {
        if (args.Count < 2)
        {
            return new CliError("send: expected <surface> <text…>");
        }

        string surface = args[0];
        string text = string.Join(' ', args.GetRange(1, args.Count - 1));
        return Single(V2("surface.send_text", w =>
        {
            w.WriteString("surface_id", surface);
            w.WriteString("text", text);
        }));
    }

    private static object ParseSendKey(List<string> args)
    {
        if (args.Count < 2 || !uint.TryParse(args[1], out uint key))
        {
            return new CliError("send-key: expected <surface> <key> [modifiers]");
        }

        uint modifiers = 0;
        if (args.Count > 2 && !uint.TryParse(args[2], out modifiers))
        {
            return new CliError("send-key: invalid modifiers");
        }

        string surface = args[0];
        return Single(V2("surface.send_key", w =>
        {
            w.WriteString("surface_id", surface);
            w.WriteNumber("key", key);
            w.WriteNumber("modifiers", modifiers);
        }));
    }

    private static object ParseNotificationIdVerb(List<string> args, string method, string allScope, string allFlag)
    {
        string? surface = null;
        string? id = null;
        bool all = false;

        for (int i = 0; i < args.Count; i++)
        {
            if (args[i] == "--surface" && i + 1 < args.Count) surface = args[++i];
            else if (args[i] == allFlag) all = true;
            else if (id is null) id = args[i];
            else return new CliError($"{method}: unexpected argument \"{args[i]}\"");
        }

        if (id is null && surface is null && !all)
        {
            return new CliError($"{method}: expected <id>, --surface <S#>, or {allFlag}");
        }

        return Single(V2(method, w =>
        {
            if (id is not null) w.WriteString("id", id);
            else if (surface is not null) w.WriteString("surface_id", surface);
            else w.WriteString("scope", allScope);
        }));
    }

    private static object ParseOpenNotification(List<string> args)
    {
        if (args.Count != 1)
        {
            return new CliError("open-notification: expected <id>");
        }

        string id = args[0];
        return Single(V2("notification.open", w => w.WriteString("id", id)));
    }

    private static object ParseReportGitBranch(List<string> args, Func<string, string?> getEnv)
    {
        string? branch = null, status = null, surface = null;
        for (int i = 0; i < args.Count; i++)
        {
            if (args[i] == "--surface" && i + 1 < args.Count) surface = args[++i];
            else if (args[i] == "--status" && i + 1 < args.Count) status = args[++i];
            else if (args[i].StartsWith("--status=", StringComparison.Ordinal)) status = args[i]["--status=".Length..];
            else if (branch is null) branch = args[i];
            else return new CliError($"report_git_branch: unexpected argument \"{args[i]}\"");
        }

        if (branch is null)
        {
            return new CliError("report_git_branch: expected <branch>");
        }

        if (!TryResolveSurface(surface, getEnv, out string resolved))
        {
            return new CliError("report_git_branch: no --surface and CMUX_SURFACE_ID is not set");
        }

        bool isDirty = string.Equals(status, "dirty", StringComparison.OrdinalIgnoreCase);
        return Single(V2("report_git_branch", w =>
        {
            w.WriteString("surface_id", resolved);
            w.WriteString("branch", branch);
            w.WriteBoolean("is_dirty", isDirty);
        }));
    }

    private static object ParseReportPr(List<string> args, Func<string, string?> getEnv)
    {
        string? number = null, label = null, url = null, status = null, branch = null, surface = null;
        bool stale = false;
        for (int i = 0; i < args.Count; i++)
        {
            switch (args[i])
            {
                case "--label" when i + 1 < args.Count: label = args[++i]; break;
                case "--url" when i + 1 < args.Count: url = args[++i]; break;
                case "--pr-status" when i + 1 < args.Count: status = args[++i]; break;
                case "--branch" when i + 1 < args.Count: branch = args[++i]; break;
                case "--surface" when i + 1 < args.Count: surface = args[++i]; break;
                case "--stale": stale = true; break;
                default:
                    if (number is null) { number = args[i]; break; }
                    return new CliError($"report_pr: unexpected argument \"{args[i]}\"");
            }
        }

        if (number is null)
        {
            return new CliError("report_pr: expected <number>");
        }

        if (!TryResolveSurface(surface, getEnv, out string resolved))
        {
            return new CliError("report_pr: no --surface and CMUX_SURFACE_ID is not set");
        }

        return Single(V2("report_pr", w =>
        {
            w.WriteString("surface_id", resolved);
            w.WriteString("number", number);
            w.WriteString("label", label ?? string.Empty);
            w.WriteString("url", url ?? string.Empty);
            w.WriteString("status", status ?? string.Empty);
            if (branch is not null) w.WriteString("branch", branch);
            w.WriteBoolean("is_stale", stale);
        }));
    }

    private static object ParseReportPwd(List<string> args, Func<string, string?> getEnv)
    {
        string? path = null, surface = null;
        for (int i = 0; i < args.Count; i++)
        {
            if (args[i] == "--surface" && i + 1 < args.Count) surface = args[++i];
            else if (path is null) path = args[i];
            else return new CliError($"report_pwd: unexpected argument \"{args[i]}\"");
        }

        if (path is null)
        {
            return new CliError("report_pwd: expected <path>");
        }

        if (!TryResolveSurface(surface, getEnv, out string resolved))
        {
            return new CliError("report_pwd: no --surface and CMUX_SURFACE_ID is not set");
        }

        return Single(V2("report_pwd", w =>
        {
            w.WriteString("surface_id", resolved);
            w.WriteString("path", path);
        }));
    }

    private static object ParseSingleString(List<string> args, string method, string field)
    {
        if (args.Count == 0)
        {
            return new CliError($"{method}: expected <{field}…>");
        }

        string value = string.Join(' ', args);
        return Single(V2(method, w => w.WriteString(field, value)));
    }

    private static object ParseAuth(List<string> args)
    {
        if (args.Count >= 1 && args[0] == "login")
        {
            string? password = null;
            for (int i = 1; i < args.Count; i++)
            {
                if (args[i] == "--password" && i + 1 < args.Count) password = args[++i];
                else if (args[i].StartsWith("--password=", StringComparison.Ordinal)) password = args[i]["--password=".Length..];
            }

            if (password is null)
            {
                return new CliError("auth login: --password is required");
            }

            return Single(V2("auth.login", w => w.WriteString("credential", password)));
        }

        return new CliError("auth: expected `auth login --password <p>`");
    }

    internal static bool TryResolveSurface(string? explicitSurface, Func<string, string?> getEnv, out string surface)
    {
        if (!string.IsNullOrEmpty(explicitSurface))
        {
            surface = explicitSurface;
            return true;
        }

        string? env = getEnv(SurfaceIdEnv);
        if (!string.IsNullOrEmpty(env))
        {
            surface = env;
            return true;
        }

        surface = string.Empty;
        return false;
    }

    internal static CliInvocation Single(string frame) => new([frame]);

    /// <summary>Builds one V2 JSON request frame: {"id":"1","method":…,"params":{…}}.</summary>
    internal static string V2(string method, Action<Utf8JsonWriter> writeParams, string id = "1")
    {
        using var stream = new MemoryStream();
        using (var writer = new Utf8JsonWriter(stream))
        {
            writer.WriteStartObject();
            writer.WriteString("id", id);
            writer.WriteString("method", method);
            writer.WriteStartObject("params");
            writeParams(writer);
            writer.WriteEndObject();
            writer.WriteEndObject();
        }

        return System.Text.Encoding.UTF8.GetString(stream.ToArray());
    }
}
