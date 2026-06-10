using System;

namespace Optimus.Core;

/// <summary>
/// Socket pipe naming helpers shared between CLI and app.
/// </summary>
public static class PipeName
{
    public const string SocketPathEnv = "OPTIMUS_SOCKET_PATH";
    public const string SocketEnv = "OPTIMUS_SOCKET";

    private const string LocalPipePrefix = @"\\.\pipe\";

    public static string BuildPipeName(string variant, string? slug = null)
    {
        string normalizedVariant = NormalizeVariant(variant);
        string suffix = string.IsNullOrWhiteSpace(slug) ? string.Empty : $"-{slug}";
        return $@"\\.\pipe\optimus-{normalizedVariant}{suffix}";
    }

    /// <summary>
    /// Strips the Win32 <c>\\.\pipe\</c> prefix so the name can be handed to
    /// <c>NamedPipeServerStream</c>/<c>NamedPipeClientStream</c>, which expect the bare
    /// pipe name and prepend the prefix themselves.
    /// </summary>
    public static string ToLocalName(string pipePath)
    {
        if (pipePath.StartsWith(LocalPipePrefix, StringComparison.OrdinalIgnoreCase))
        {
            return pipePath[LocalPipePrefix.Length..];
        }

        return pipePath;
    }

    public static string ResolveFromEnvironment(Func<string, string?> getEnv)
    {
        string? explicitPath = getEnv(SocketPathEnv);
        if (!string.IsNullOrWhiteSpace(explicitPath))
        {
            return explicitPath;
        }

        string? fallback = getEnv(SocketEnv);
        if (!string.IsNullOrWhiteSpace(fallback))
        {
            return fallback;
        }

        return string.Empty;
    }

    public static string NormalizeVariant(string variant)
    {
        if (string.IsNullOrWhiteSpace(variant))
        {
            return "stable";
        }

        return variant.Trim().ToLowerInvariant() switch
        {
            "nightly" => "nightly",
            "staging" => "staging",
            "dev" => "dev",
            "stable" => "stable",
            _ => "stable",
        };
    }
}
