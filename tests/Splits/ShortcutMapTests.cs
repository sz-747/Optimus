using System;
using System.Collections.Generic;
using System.Linq;
using Optimus.Core;
using Xunit;

namespace Optimus.Core.Tests;

/// <summary>
/// Coverage for <see cref="ShortcutMap"/> (plan Phase 2 U5): the chord→action table resolves the
/// intended action, and applying an action drives the expected controller operation against a real
/// <see cref="SplitTreeController"/> (the plan's "each chord resolves to the intended controller
/// call" scenario, exercised through the pure dispatcher rather than a fake).
/// </summary>
public sealed class ShortcutMapTests
{
    private const ChordModifiers CtrlShift = ChordModifiers.Ctrl | ChordModifiers.Shift;

    // ---- The chord table ---------------------------------------------------------------------

    [Theory]
    [InlineData(0x25, ShortcutAction.FocusLeft)]
    [InlineData(0x27, ShortcutAction.FocusRight)]
    [InlineData(0x26, ShortcutAction.FocusUp)]
    [InlineData(0x28, ShortcutAction.FocusDown)]
    [InlineData(0x54, ShortcutAction.NewTab)]
    [InlineData(0x57, ShortcutAction.CloseTab)]
    [InlineData(0x44, ShortcutAction.SplitRight)]
    [InlineData(0x45, ShortcutAction.SplitDown)]
    [InlineData(0x30, ShortcutAction.Equalize)]
    [InlineData(0x5A, ShortcutAction.ToggleZoom)]
    public void Resolve_maps_ctrl_shift_chords(int keyCode, ShortcutAction expected)
    {
        Assert.Equal(expected, ShortcutMap.Resolve(CtrlShift, keyCode));
    }

    [Fact]
    public void Resolve_distinguishes_tab_cycle_by_shift()
    {
        Assert.Equal(ShortcutAction.NextTab, ShortcutMap.Resolve(ChordModifiers.Ctrl, 0x09));
        Assert.Equal(ShortcutAction.PreviousTab, ShortcutMap.Resolve(CtrlShift, 0x09));
    }

    [Fact]
    public void Resolve_returns_null_for_an_unbound_chord()
    {
        Assert.Null(ShortcutMap.Resolve(ChordModifiers.None, 0x44)); // plain 'D' — not bound
        Assert.Null(ShortcutMap.Resolve(ChordModifiers.Ctrl, 0x43)); // Ctrl+C — owned by the terminal
    }

    [Fact]
    public void Defaults_cover_every_action_with_no_duplicate_chords()
    {
        IEnumerable<ShortcutAction> bound = ShortcutMap.Defaults.Values.Distinct();
        Assert.Equal(Enum.GetValues<ShortcutAction>().Length, bound.Count()); // every action reachable
        Assert.Equal(ShortcutMap.Defaults.Count, ShortcutMap.Defaults.Keys.Distinct().Count()); // unique chords
    }

    // ---- The dispatcher ----------------------------------------------------------------------

    [Fact]
    public void Apply_split_right_adds_a_side_by_side_pane_and_focuses_it()
    {
        var c = new SplitTreeController();
        PaneId original = c.FocusedPane;

        ShortcutMap.Apply(c, ShortcutAction.SplitRight);

        Assert.Equal(2, c.AllPaneIds.Count);
        Assert.NotEqual(original, c.FocusedPane); // R4: the new pane is focused
    }

    [Fact]
    public void Apply_new_then_close_tab_round_trips_the_focused_pane()
    {
        var c = new SplitTreeController();
        PaneId pane = c.FocusedPane;
        Assert.Single(c.Tabs(pane));

        ShortcutMap.Apply(c, ShortcutAction.NewTab);
        Assert.Equal(2, c.Tabs(pane).Count);

        ShortcutMap.Apply(c, ShortcutAction.CloseTab); // closes the just-added, selected tab
        Assert.Single(c.Tabs(pane));
    }

    [Fact]
    public void Apply_directional_focus_moves_across_a_split_and_back()
    {
        var c = new SplitTreeController();
        PaneId left = c.FocusedPane;
        ShortcutMap.Apply(c, ShortcutAction.SplitRight); // focus moves to the new right pane
        PaneId right = c.FocusedPane;
        Assert.NotEqual(left, right);

        ShortcutMap.Apply(c, ShortcutAction.FocusLeft);
        Assert.Equal(left, c.FocusedPane);

        ShortcutMap.Apply(c, ShortcutAction.FocusRight);
        Assert.Equal(right, c.FocusedPane);
    }

    [Fact]
    public void Apply_toggle_zoom_flips_the_zoomed_pane()
    {
        var c = new SplitTreeController();

        Assert.Null(c.ZoomedPane);
        ShortcutMap.Apply(c, ShortcutAction.ToggleZoom);
        Assert.Equal(c.FocusedPane, c.ZoomedPane);
        ShortcutMap.Apply(c, ShortcutAction.ToggleZoom);
        Assert.Null(c.ZoomedPane);
    }

    // ---- Chord description (tooltip text) ----------------------------------------------------

    [Theory]
    [InlineData(ShortcutAction.SplitRight, "Ctrl+Shift+D")]
    [InlineData(ShortcutAction.SplitDown, "Ctrl+Shift+E")]
    [InlineData(ShortcutAction.ToggleZoom, "Ctrl+Shift+Z")]
    [InlineData(ShortcutAction.NewTab, "Ctrl+Shift+T")]
    [InlineData(ShortcutAction.CloseTab, "Ctrl+Shift+W")]
    [InlineData(ShortcutAction.Equalize, "Ctrl+Shift+0")]
    [InlineData(ShortcutAction.FocusLeft, "Ctrl+Shift+Left")]
    [InlineData(ShortcutAction.FocusDown, "Ctrl+Shift+Down")]
    public void DescribeChord_formats_the_bound_chord(ShortcutAction action, string expected)
    {
        Assert.Equal(expected, ShortcutMap.DescribeChord(action));
    }

    [Fact]
    public void DescribeChord_uses_a_single_modifier_when_the_chord_has_one()
    {
        // Ctrl+Tab carries no Shift; the formatter must not emit a stray modifier.
        Assert.Equal("Ctrl+Tab", ShortcutMap.DescribeChord(ShortcutAction.NextTab));
        Assert.Equal("Ctrl+Shift+Tab", ShortcutMap.DescribeChord(ShortcutAction.PreviousTab));
    }

    [Fact]
    public void DescribeChord_returns_a_nonempty_string_for_every_action()
    {
        // Coverage parity with Defaults: every action is reachable, so every action describes.
        foreach (ShortcutAction action in Enum.GetValues<ShortcutAction>())
        {
            Assert.False(string.IsNullOrEmpty(ShortcutMap.DescribeChord(action)), $"no chord text for {action}");
        }
    }

    [Fact]
    public void DescribeChord_orders_modifiers_ctrl_before_shift()
    {
        // Stable order regardless of how the flags enum happens to combine.
        Assert.StartsWith("Ctrl+Shift+", ShortcutMap.DescribeChord(ShortcutAction.SplitRight));
    }
}
