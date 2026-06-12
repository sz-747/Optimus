using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text.RegularExpressions;
using Xunit;

namespace Optimus.Core.Tests;

/// <summary>
/// Guard for CLAUDE.md R5 / DESIGN.md "Color": chrome view code must consume named tokens from
/// <c>app/Design/Tokens.cs</c>, never raw <c>Color.FromArgb</c> or inline numeric <c>FontSize</c>
/// literals. Tokens.cs itself is the one allowed home for raw hex / size literals and is whitelisted.
///
/// <para>res U3 landed the initial migration of <c>SidebarView</c>, <c>PaneTabStrip</c>,
/// <c>PaneView</c>, and <c>SplitTreeView</c>; this test fails the build if any future chrome change
/// regresses by re-introducing inline literals anywhere under <c>app/</c>.</para>
/// </summary>
public sealed class TokensGuardTests
{
    // FontSize = <number>  (allow Tokens.* and any other identifier; only numeric literals are banned).
    private static readonly Regex InlineFontSize =
        new(@"FontSize\s*=\s*-?\d", RegexOptions.Compiled);

    // Color.FromArgb(...)  (matches calls anywhere on the line).
    private static readonly Regex InlineColorFromArgb =
        new(@"Color\.FromArgb\s*\(", RegexOptions.Compiled);

    [Fact]
    public void View_code_uses_named_tokens_for_color_and_fontsize()
    {
        string appDir = LocateAppDirectory();
        string tokensPath = Path.GetFullPath(Path.Combine(appDir, "Design", "Tokens.cs"));

        List<string> offenders = new();

        foreach (string file in Directory.EnumerateFiles(appDir, "*.cs", SearchOption.AllDirectories))
        {
            // Whitelist: Tokens.cs is the one place raw literals live.
            if (string.Equals(Path.GetFullPath(file), tokensPath, StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }

            string[] lines = File.ReadAllLines(file);
            for (int i = 0; i < lines.Length; i++)
            {
                string line = lines[i];
                // Skip comment lines so doc-comments mentioning the banned APIs don't trip the guard.
                string trimmed = line.TrimStart();
                if (trimmed.StartsWith("//") || trimmed.StartsWith("///") || trimmed.StartsWith("*"))
                {
                    continue;
                }
                if (InlineColorFromArgb.IsMatch(line))
                {
                    offenders.Add($"{file}:{i + 1}: raw Color.FromArgb — use a token from app/Design/Tokens.cs");
                }
                if (InlineFontSize.IsMatch(line))
                {
                    offenders.Add($"{file}:{i + 1}: inline FontSize literal — use Tokens.FontCaption/FontMeta/FontBody/FontTitle");
                }
            }
        }

        Assert.True(
            offenders.Count == 0,
            "Inline color/FontSize literals found in chrome view code (CLAUDE.md R5):\n  "
                + string.Join("\n  ", offenders));
    }

    /// <summary>
    /// Walk up from the test assembly's directory until we find a sibling <c>app/</c> directory that
    /// also has a <c>Design/Tokens.cs</c>. Tests build under <c>tests/bin/&lt;config&gt;/net9.0/</c>;
    /// the repo root is therefore a few levels up. Walking instead of hard-coding keeps the test
    /// robust against moved output paths.
    /// </summary>
    private static string LocateAppDirectory()
    {
        DirectoryInfo? dir = new(AppContext.BaseDirectory);
        while (dir is not null)
        {
            string candidate = Path.Combine(dir.FullName, "app");
            if (Directory.Exists(candidate)
                && File.Exists(Path.Combine(candidate, "Design", "Tokens.cs")))
            {
                return candidate;
            }
            dir = dir.Parent;
        }
        throw new InvalidOperationException(
            $"Could not locate the app/ directory by walking up from {AppContext.BaseDirectory}; "
                + "the TokensGuardTests test expects the repo layout app/Design/Tokens.cs to exist.");
    }
}
