using System;
using System.Collections.Generic;
using System.IO;
using System.Text.Json;
using Cmux.Core;

namespace Cmux.Cli;

/// <summary>
/// The cmux CLI entry point (plan Phase 4 U4). All decisions live in the pure
/// <see cref="CliParser"/>/<see cref="HooksCommand"/> layer; this shell only does I/O:
/// read stdin when redirected, resolve the pipe, send frames, print, set the exit code.
/// </summary>
public static class Program
{
    public static int Main(string[] args)
    {
        string? stdin = Console.IsInputRedirected ? Console.In.ReadToEnd() : null;

        object parsed = CliParser.Parse(args, Environment.GetEnvironmentVariable, stdin);
        if (parsed is CliError error)
        {
            Console.Error.WriteLine(error.Message);
            return error.ExitCode;
        }

        var invocation = (CliInvocation)parsed;

        if (invocation.StdOut is not null)
        {
            Console.WriteLine(invocation.StdOut);
        }

        if (invocation.FileWrites is { Count: > 0 })
        {
            string dir = invocation.InstallDir
                ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".cmuxterm", "hooks");
            Directory.CreateDirectory(dir);
            foreach (LocalFileWrite write in invocation.FileWrites)
            {
                string path = Path.Combine(dir, write.RelativePath);
                File.WriteAllText(path, write.Content);
                Console.WriteLine(path);
            }
        }

        if (invocation.Frames.Count == 0)
        {
            return 0;
        }

        string pipe = ResolvePipe(invocation);
        try
        {
            IReadOnlyList<string?> responses = PipeClient.Send(pipe, invocation.Frames);
            return ReportResponses(responses);
        }
        catch (Exception ex) when (ex is TimeoutException or IOException or UnauthorizedAccessException)
        {
            Console.Error.WriteLine($"cmux: cannot reach the app on \"{pipe}\": {ex.Message}");
            return 1;
        }
    }

    private static string ResolvePipe(CliInvocation invocation)
    {
        if (!string.IsNullOrEmpty(invocation.ExplicitSocket))
        {
            return invocation.ExplicitSocket;
        }

        string fromEnv = PipeName.ResolveFromEnvironment(Environment.GetEnvironmentVariable);
        IReadOnlyList<string> candidates = SocketDiscovery.BuildCandidatePipes(
            string.IsNullOrEmpty(fromEnv) ? null : fromEnv,
            invocation.Variant);
        return SocketDiscovery.ResolveWithProbe(candidates, PipeClient.CanConnect);
    }

    private static int ReportResponses(IReadOnlyList<string?> responses)
    {
        int exit = 0;
        foreach (string? response in responses)
        {
            if (response is null)
            {
                Console.Error.WriteLine("cmux: connection closed before a response arrived");
                exit = 1;
                continue;
            }

            if (!TryReportV2(response, ref exit))
            {
                // V1 text response: "OK[: detail]" or "ERROR: detail".
                Console.WriteLine(response);
                if (response.StartsWith("ERROR", StringComparison.Ordinal))
                {
                    exit = 1;
                }
            }
        }

        return exit;
    }

    private static bool TryReportV2(string response, ref int exit)
    {
        if (response.Length == 0 || response[0] != '{')
        {
            return false;
        }

        try
        {
            using JsonDocument doc = JsonDocument.Parse(response);
            JsonElement root = doc.RootElement;
            bool ok = root.TryGetProperty("ok", out JsonElement okEl) && okEl.ValueKind == JsonValueKind.True;
            if (ok)
            {
                Console.WriteLine(root.TryGetProperty("result", out JsonElement result) && result.ValueKind != JsonValueKind.Null
                    ? result.GetRawText()
                    : "ok");
            }
            else
            {
                string message = root.TryGetProperty("error", out JsonElement err)
                    && err.ValueKind == JsonValueKind.Object
                    && err.TryGetProperty("message", out JsonElement msg)
                        ? msg.GetString() ?? "unknown error"
                        : "unknown error";
                Console.Error.WriteLine($"cmux: {message}");
                exit = 1;
            }

            return true;
        }
        catch (JsonException)
        {
            return false;
        }
    }
}
