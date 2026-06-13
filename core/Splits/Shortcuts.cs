using System;
using System.Collections.Generic;

namespace Optimus.Core;

/// <summary>Modifier flags for a keyboard chord — platform-neutral so the table is unit-testable.</summary>
[Flags]
public enum ChordModifiers
{
    None = 0,
    Ctrl = 1,
    Shift = 2,
    Alt = 4,
    Super = 8,
}

/// <summary>
/// A platform-neutral key chord: a modifier set plus a key code. <see cref="KeyCode"/> is the
/// numeric Windows virtual-key value (e.g. <c>0x44</c> = 'D', <c>0x25</c> = Left) — the app casts
/// its <c>VirtualKey</c> straight to <see cref="int"/>, and <see cref="VKey"/> documents the codes
/// the default table uses. Kept free of WinRT types so it lives in <c>Optimus.Core</c> and is testable.
/// </summary>
public readonly record struct KeyChord(ChordModifiers Modifiers, int KeyCode);

/// <summary>
/// The workspace actions reachable by keyboard (R8) — a faithful port of the macOS bonsplit
/// shortcut set (optimus/Sources/KeyboardShortcutContext.swift: splitright/splitdown, focus*, newTab,
/// closeTab, selectNext/Previous, equalize, zoom).
/// </summary>
public enum ShortcutAction
{
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    NextTab,
    PreviousTab,
    NewTab,
    NewWebTab, // open a WebView2 pane (p6 U4) — host-handled, not a model-plane op (see Apply)
    CloseTab,
    SplitRight, // side-by-side / vertical divider (KTD7)
    SplitDown,  // stacked / horizontal divider (KTD7)
    Equalize,
    ToggleZoom,
}

/// <summary>
/// The pure chord→action table and action→controller dispatcher (plan Phase 2 U5). Living in
/// <c>Optimus.Core</c> keeps both halves unit-testable: <see cref="Resolve"/> against the table and
/// <see cref="Apply"/> against a real <see cref="SplitTreeController"/>. The app's
/// <c>ShortcutRouter</c> is a thin adapter that builds <c>KeyboardAccelerator</c>s from
/// <see cref="Defaults"/> and routes their invocations through <see cref="Apply"/>.
/// </summary>
public static class ShortcutMap
{
    /// <summary>Numeric Windows virtual-key codes used by the default chords (stable Win32 VK_* values).</summary>
    private static class VKey
    {
        public const int Tab = 0x09;
        public const int Left = 0x25;
        public const int Up = 0x26;
        public const int Right = 0x27;
        public const int Down = 0x28;
        public const int D0 = 0x30;
        public const int D = 0x44;
        public const int E = 0x45;
        public const int G = 0x47;
        public const int T = 0x54;
        public const int W = 0x57;
        public const int Z = 0x5A;
    }

    private const ChordModifiers CtrlShift = ChordModifiers.Ctrl | ChordModifiers.Shift;

    /// <summary>
    /// The default chord→action bindings. All sit in the Ctrl(+Shift) namespace so they never shadow
    /// a plain key headed for the terminal; the Ctrl+Shift+C/V copy-paste chords (owned by the
    /// terminal surface) are deliberately avoided here.
    /// </summary>
    public static IReadOnlyDictionary<KeyChord, ShortcutAction> Defaults { get; } =
        new Dictionary<KeyChord, ShortcutAction>
        {
            // Directional focus — Ctrl+Shift+Arrow (plain Ctrl+Arrow is left for shell word-nav).
            [new(CtrlShift, VKey.Left)] = ShortcutAction.FocusLeft,
            [new(CtrlShift, VKey.Right)] = ShortcutAction.FocusRight,
            [new(CtrlShift, VKey.Up)] = ShortcutAction.FocusUp,
            [new(CtrlShift, VKey.Down)] = ShortcutAction.FocusDown,

            // Tab cycling — classic Ctrl+Tab / Ctrl+Shift+Tab.
            [new(ChordModifiers.Ctrl, VKey.Tab)] = ShortcutAction.NextTab,
            [new(CtrlShift, VKey.Tab)] = ShortcutAction.PreviousTab,

            // Tab lifecycle.
            [new(CtrlShift, VKey.T)] = ShortcutAction.NewTab,
            [new(CtrlShift, VKey.G)] = ShortcutAction.NewWebTab, // G = "go to the web" (p6 U4)
            [new(CtrlShift, VKey.W)] = ShortcutAction.CloseTab,

            // Splits — D = split right (side-by-side), E = split down (stacked).
            [new(CtrlShift, VKey.D)] = ShortcutAction.SplitRight,
            [new(CtrlShift, VKey.E)] = ShortcutAction.SplitDown,

            // Layout.
            [new(CtrlShift, VKey.D0)] = ShortcutAction.Equalize,
            [new(CtrlShift, VKey.Z)] = ShortcutAction.ToggleZoom,
        };

    /// <summary>Resolve a chord to its action, or <c>null</c> if unbound.</summary>
    public static ShortcutAction? Resolve(ChordModifiers modifiers, int keyCode) =>
        Defaults.TryGetValue(new KeyChord(modifiers, keyCode), out ShortcutAction action) ? action : null;

    /// <summary>
    /// Describe the default chord bound to <paramref name="action"/> as a display string
    /// (e.g. <c>"Ctrl+Shift+D"</c>), sourced from <see cref="Defaults"/> so tooltip text never
    /// drifts from the live bindings (R3). Returns an empty string if the action is unbound.
    /// </summary>
    public static string DescribeChord(ShortcutAction action)
    {
        foreach (KeyValuePair<KeyChord, ShortcutAction> entry in Defaults)
        {
            if (entry.Value == action)
            {
                return Describe(entry.Key);
            }
        }
        return string.Empty;
    }

    /// <summary>Format a chord as <c>Mod(+Mod)+Key</c> with a stable modifier order.</summary>
    private static string Describe(KeyChord chord)
    {
        var parts = new List<string>(4);
        if (chord.Modifiers.HasFlag(ChordModifiers.Ctrl)) parts.Add("Ctrl");
        if (chord.Modifiers.HasFlag(ChordModifiers.Shift)) parts.Add("Shift");
        if (chord.Modifiers.HasFlag(ChordModifiers.Alt)) parts.Add("Alt");
        if (chord.Modifiers.HasFlag(ChordModifiers.Super)) parts.Add("Super");
        parts.Add(KeyName(chord.KeyCode));
        return string.Join("+", parts);
    }

    /// <summary>Display name for a virtual-key code used by the default table.</summary>
    private static string KeyName(int keyCode) => keyCode switch
    {
        VKey.Tab => "Tab",
        VKey.Left => "Left",
        VKey.Up => "Up",
        VKey.Right => "Right",
        VKey.Down => "Down",
        VKey.D0 => "0",
        VKey.D => "D",
        VKey.E => "E",
        VKey.G => "G",
        VKey.T => "T",
        VKey.W => "W",
        VKey.Z => "Z",
        _ => $"0x{keyCode:X2}",
    };

    /// <summary>
    /// Apply <paramref name="action"/> to the focused pane/surface of <paramref name="controller"/>.
    /// Operations that have no target (e.g. close with no focused surface) are silent no-ops.
    /// </summary>
    public static void Apply(SplitTreeController controller, ShortcutAction action)
    {
        switch (action)
        {
            case ShortcutAction.FocusLeft:
                controller.MoveFocus(Direction.Left);
                break;
            case ShortcutAction.FocusRight:
                controller.MoveFocus(Direction.Right);
                break;
            case ShortcutAction.FocusUp:
                controller.MoveFocus(Direction.Up);
                break;
            case ShortcutAction.FocusDown:
                controller.MoveFocus(Direction.Down);
                break;
            case ShortcutAction.NextTab:
                controller.SelectNextTab();
                break;
            case ShortcutAction.PreviousTab:
                controller.SelectPreviousTab();
                break;
            case ShortcutAction.NewTab:
                controller.NewTab(controller.FocusedPane);
                break;
            case ShortcutAction.NewWebTab:
                // Intentionally a no-op here. Opening a WebView2 pane needs the surface plane (a
                // typed factory + the per-user UDF), which the model-plane controller has no access
                // to. The host (WorkspaceView, via ShortcutRouter) handles this action; it lives in
                // this table only so the chord is declared in one place and DescribeChord can label
                // the affordance tooltip (p6 U4).
                break;
            case ShortcutAction.CloseTab:
                if (controller.FocusedSurface is SurfaceId surface)
                {
                    controller.CloseTab(surface);
                }
                break;
            case ShortcutAction.SplitRight:
                controller.Split(controller.FocusedPane, Orientation.Vertical);
                break;
            case ShortcutAction.SplitDown:
                controller.Split(controller.FocusedPane, Orientation.Horizontal);
                break;
            case ShortcutAction.Equalize:
                controller.Equalize();
                break;
            case ShortcutAction.ToggleZoom:
                controller.ToggleZoom();
                break;
        }
    }
}
