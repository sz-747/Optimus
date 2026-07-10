using Microsoft.UI.Xaml.Media;
using Windows.UI;

namespace Optimus.Design;

/// <summary>
/// Named design tokens from DESIGN.md ("Graphite") — the one place in the app assembly where raw
/// hex values are allowed to live. View code consumes these by name and never writes
/// <c>Color.FromArgb</c> / inline <c>FontSize</c> literals (CLAUDE.md hard rule, enforced by
/// <c>TokensGuardTests</c>).
///
/// <para>Seeded by the capacity indicator (plan U6); res U3 extended the registry to cover the
/// older views (<c>SidebarView</c>, <c>PaneTabStrip</c>, <c>PaneView</c>, <c>SplitTreeView</c>)
/// and applied DESIGN.md's RISK #2 split — <see cref="Attention"/> is the focus / notification-
/// flash teal, <see cref="Unread"/> is the dedicated unread-badge magenta, and <see cref="PrOpen"/>
/// stops doubling as either of those.</para>
/// </summary>
internal static class Tokens
{
    // ---- Color: surfaces (DESIGN.md "Surfaces" — value-step elevation, no blur) -----------------

    /// <summary>`surface-0` #0C0C0C — pane content background (deepest).</summary>
    public static readonly SolidColorBrush Surface0 = Brush(0x0C, 0x0C, 0x0C);

    /// <summary>`surface-1` #121212 — sidebar panel.</summary>
    public static readonly SolidColorBrush Surface1 = Brush(0x12, 0x12, 0x12);

    /// <summary>`surface-2` #161616 — tab strip.</summary>
    public static readonly SolidColorBrush Surface2 = Brush(0x16, 0x16, 0x16);

    /// <summary>`surface-selected` #2D2D2D — selected sidebar row / active tab chip.</summary>
    public static readonly SolidColorBrush SurfaceSelected = Brush(0x2D, 0x2D, 0x2D);

    /// <summary>`hairline` #2B2B2B — dividers; also the capacity bar's empty track and split gutters.</summary>
    public static readonly SolidColorBrush Hairline = Brush(0x2B, 0x2B, 0x2B);

    /// <summary>Fully transparent — for chip / button backgrounds that should not paint a surface.</summary>
    public static readonly SolidColorBrush Transparent = new(Color.FromArgb(0x00, 0x00, 0x00, 0x00));

    // ---- Color: text (DESIGN.md "Text") --------------------------------------------------------

    /// <summary>`text-primary` #E6E6E6 — titles, active labels.</summary>
    public static readonly SolidColorBrush TextPrimary = Brush(0xE6, 0xE6, 0xE6);

    /// <summary>`text-muted` #8A8A8A — metadata, inactive labels.</summary>
    public static readonly SolidColorBrush TextMuted = Brush(0x8A, 0x8A, 0x8A);

    /// <summary>`text-on-accent` #FFFFFF — text/glyphs sitting on a colored badge.</summary>
    public static readonly SolidColorBrush TextOnAccent = Brush(0xFF, 0xFF, 0xFF);

    // ---- Color: semantic (DESIGN.md "Semantic" — keep, maps to dev conventions) ----------------

    /// <summary>`git-branch` #7FB36E — branch name (clean tree).</summary>
    public static readonly SolidColorBrush GitBranch = Brush(0x7F, 0xB3, 0x6E);

    /// <summary>`git-dirty` #D9A04E — dirty working-tree marker.</summary>
    public static readonly SolidColorBrush GitDirty = Brush(0xD9, 0xA0, 0x4E);

    /// <summary>`pr-open` #4D9CF0 — PR open. After RISK #2, this hue means **only** pr-open.</summary>
    public static readonly SolidColorBrush PrOpen = Brush(0x4D, 0x9C, 0xF0);

    /// <summary>`pr-merged` #A87FE0 — PR merged.</summary>
    public static readonly SolidColorBrush PrMerged = Brush(0xA8, 0x7F, 0xE0);

    /// <summary>`pr-closed` #D96A6A — PR closed.</summary>
    public static readonly SolidColorBrush PrClosed = Brush(0xD9, 0x6A, 0x6A);

    // ---- Color: attention (DESIGN.md "Attention" — unified per RISK #2) ------------------------

    /// <summary>`attention` #2DD4BF — focused-pane border and the notification-flash pulse. Teal
    /// always means "active / live"; no other token uses this hue.</summary>
    public static readonly SolidColorBrush Attention = Brush(0x2D, 0xD4, 0xBF);

    /// <summary>`unread` #D86FB0 — unread notification dot / sidebar badge. Magenta is dedicated to
    /// "unread" so it never collides with <see cref="PrOpen"/> or <see cref="Attention"/>.</summary>
    public static readonly SolidColorBrush Unread = Brush(0xD8, 0x6F, 0xB0);

    // ---- Color: capacity indicator (semantic aliases — DESIGN.md "Thesis") ---------------------

    /// <summary>Capacity below 75% of the safe zone — maps to `text-muted` (calm, recedes).</summary>
    public static readonly SolidColorBrush CapacityCalm = TextMuted;

    /// <summary>Capacity ≥ 75% of the safe zone — maps to `git-dirty`.</summary>
    public static readonly SolidColorBrush CapacityWarn = GitDirty;

    /// <summary>Capacity at the safe-zone cap — maps to `pr-closed`.</summary>
    public static readonly SolidColorBrush CapacityCap = PrClosed;

    // ---- Typography (DESIGN.md "Typography" scale + families) -----------------------------------

    /// <summary>Cascadia Mono — metadata and counts (tabular figures align "3/5", "#42").</summary>
    public static readonly FontFamily Mono = new("Cascadia Mono");

    /// <summary>Segoe MDL2 Assets — the Windows system icon font, for monochrome chrome affordance
    /// glyphs (e.g. the tab-strip "web" globe, p6 U4). Ships with Windows 10+; keeps affordance icons
    /// crisp and on-theme without an emoji color glyph. See DESIGN.md typography note.</summary>
    public static readonly FontFamily IconFont = new("Segoe MDL2 Assets");

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
