using System;

namespace Cmux.Core;

/// <summary>
/// Socket pipe naming helpers shared between CLI and app.
/// </summary>
public static class PipeName
{
    public const string SocketPathEnv = "CMUX_SOCKET_PATH";
    public const string SocketEnv = "CMUX_SOCKET";

    public static string BuildPipeName(string variant, string? slug = null)
    {
        string normalizedVariant = NormalizeVariant(variant);
        string suffix = string.IsNullOrWhiteSpace(slug) ? string.Empty : $"-{slug}";
        return $@"\\\\.\\pipe\\cmux-{normalizedVariant}{suffix}";
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
