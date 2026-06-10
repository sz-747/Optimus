using System;
using System.Collections.Generic;
using System.Linq;

namespace Optimus.Core;

/// <summary>
/// Pipe discovery order + probe helper for connecting to a live server.
/// </summary>
public static class SocketDiscovery
{
    private static readonly string[] VariantFallbackOrder = ["stable", "nightly", "staging", "dev"];

    public static IReadOnlyList<string> BuildCandidatePipes(
        string? explicitPath,
        string? explicitVariant,
        string? slug = null)
    {
        var candidates = new List<string>();

        if (!string.IsNullOrWhiteSpace(explicitPath))
        {
            candidates.Add(explicitPath!);
        }

        string preferredVariant = PipeName.NormalizeVariant(explicitVariant ?? string.Empty);
        candidates.Add(PipeName.BuildPipeName(preferredVariant, slug));

        foreach (string variant in VariantFallbackOrder)
        {
            if (string.Equals(variant, preferredVariant, StringComparison.Ordinal))
            {
                continue;
            }

            candidates.Add(PipeName.BuildPipeName(variant, slug));
        }

        return candidates;
    }

    public static string ResolveWithProbe(
        IEnumerable<string> candidates,
        Func<string, bool> canConnect)
    {
        foreach (string candidate in candidates)
        {
            if (canConnect(candidate))
            {
                return candidate;
            }
        }

        return candidates.First();
    }
}
