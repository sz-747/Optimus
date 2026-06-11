using Microsoft.UI.Xaml.Media;
using Windows.UI;

namespace Optimus.Design;

/// <summary>
/// Named design tokens from DESIGN.md ("Graphite") — the one place in the app assembly where raw
/// hex values are allowed to live. View code consumes these by name and never writes
/// <c>Color.FromArgb</c> / inline <c>FontSize</c> literals (CLAUDE.md hard rule).
///
/// <para>Seeded with what the capacity indicator (plan U6) needs; DESIGN.md's "Implementation
/// note" tracks migrating the remaining inline literals in the older views here.</para>
/// </summary>
internal static class Tokens
{
    // ---- Color: surfaces / text (DESIGN.md "Color") ---------------------------------------------

    /// <summary>`hairline` #2B2B2B — dividers; also the capacity bar's empty track.</summary>
    public static readonly SolidColorBrush Hairline = Brush(0x2B, 0x2B, 0x2B);

    /// <summary>`text-muted` #8A8A8A — metadata, inactive labels.</summary>
    public static readonly SolidColorBrush TextMuted = Brush(0x8A, 0x8A, 0x8A);

    // ---- Color: capacity indicator (DESIGN.md "Thesis": calm → git-dirty amber → pr-closed red) -

    /// <summary>Capacity below 75% of the safe zone — maps to `text-muted` (calm, recedes).</summary>
    public static readonly SolidColorBrush CapacityCalm = TextMuted;

    /// <summary>Capacity ≥ 75% of the safe zone — maps to `git-dirty` #D9A04E.</summary>
    public static readonly SolidColorBrush CapacityWarn = Brush(0xD9, 0xA0, 0x4E);

    /// <summary>Capacity at the safe-zone cap — maps to `pr-closed` #D96A6A.</summary>
    public static readonly SolidColorBrush CapacityCap = Brush(0xD9, 0x6A, 0x6A);

    // ---- Typography (DESIGN.md "Typography" scale + families) -----------------------------------

    /// <summary>Cascadia Mono — metadata and counts (tabular figures align "3/5", "#42").</summary>
    public static readonly FontFamily Mono = new("Cascadia Mono");

    /// <summary>`caption` 10px — badge text, smallest meta.</summary>
    public const double FontCaption = 10;

    /// <summary>`meta` 11px — row metadata (branch · PR · status · counts).</summary>
    public const double FontMeta = 11;

    /// <summary>`body` 12px — tab chip labels.</summary>
    public const double FontBody = 12;

    /// <summary>`title` 13px — sidebar row title.</summary>
    public const double FontTitle = 13;

    private static SolidColorBrush Brush(byte r, byte g, byte b) =>
        new(Color.FromArgb(0xFF, r, g, b));
}
