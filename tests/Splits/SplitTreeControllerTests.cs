using System.Collections.Generic;
using System.Linq;
using Cmux.Core;
using Xunit;

namespace Cmux.Core.Tests;

/// <summary>
/// Behavioral coverage for <see cref="SplitTreeController"/> (plan Phase 2 U1): tree shape after
/// split/close, the heal-on-close-last-tab invariant, divider clamping/equalize, derived focus,
/// tab cycling, and snapshot/id stability.
/// </summary>
public sealed class SplitTreeControllerTests
{
    private static IEnumerable<SplitBranch> Branches(SplitNode? node)
    {
        if (node is SplitBranch b)
        {
            yield return b;
            foreach (SplitBranch child in Branches(b.First).Concat(Branches(b.Second)))
            {
                yield return child;
            }
        }
    }

    private static int BranchCount(SplitNode? node) => Branches(node).Count();

    private static int LeafCount(SplitNode? node) => node?.Leaves().Count() ?? 0;

    // ---- R4: initial state + splits ----------------------------------------------------------

    [Fact] // Covers R4.
    public void Initial_controller_has_one_focused_pane_with_one_selected_surface()
    {
        var c = new SplitTreeController();

        SplitNode? root = c.Snapshot().Root;
        PaneLeaf leaf = Assert.IsType<PaneLeaf>(root);
        Assert.Single(leaf.Tabs);
        Assert.Equal(leaf.Tabs[0], leaf.Selected);
        Assert.Equal(leaf.Id, c.FocusedPane);
        Assert.Equal(leaf.Selected, c.FocusedSurface);
        Assert.Single(c.AllPaneIds);
    }

    [Fact] // Covers R4.
    public void Split_vertical_creates_branch_with_original_first_and_new_second()
    {
        var c = new SplitTreeController();
        PaneId original = c.FocusedPane;
        SurfaceId originalSurface = c.FocusedSurface!.Value;

        c.Split(original, Orientation.Vertical, insertFirst: false);

        SplitBranch branch = Assert.IsType<SplitBranch>(c.Snapshot().Root);
        Assert.Equal(Orientation.Vertical, branch.Orientation);
        Assert.Equal(0.5, branch.DividerPosition);

        PaneLeaf first = Assert.IsType<PaneLeaf>(branch.First);
        Assert.Equal(original, first.Id);
        Assert.Equal(originalSurface, first.Selected);

        PaneLeaf second = Assert.IsType<PaneLeaf>(branch.Second);
        Assert.Single(second.Tabs);
        Assert.NotEqual(originalSurface, second.Selected);

        // New pane is focused and its lone surface is the focused surface.
        Assert.Equal(second.Id, c.FocusedPane);
        Assert.Equal(second.Selected, c.FocusedSurface);
    }

    [Fact] // Covers R4.
    public void Split_insertFirst_places_new_pane_first()
    {
        var c = new SplitTreeController();
        PaneId original = c.FocusedPane;

        c.Split(original, Orientation.Horizontal, insertFirst: true);

        SplitBranch branch = Assert.IsType<SplitBranch>(c.Snapshot().Root);
        PaneLeaf first = Assert.IsType<PaneLeaf>(branch.First);
        PaneLeaf second = Assert.IsType<PaneLeaf>(branch.Second);
        Assert.Equal(original, second.Id);       // the original is now Second
        Assert.Equal(first.Id, c.FocusedPane);   // the new pane is First and focused
    }

    [Fact] // Covers R4.
    public void Nested_split_yields_depth_two_tree_with_three_leaves()
    {
        var c = new SplitTreeController();
        c.Split(c.FocusedPane, Orientation.Vertical);   // 2 leaves
        c.Split(c.FocusedPane, Orientation.Horizontal); // split the new pane → 3 leaves

        SplitNode? root = c.Snapshot().Root;
        Assert.Equal(3, LeafCount(root));
        Assert.Equal(2, BranchCount(root));
        Assert.Equal(3, c.AllPaneIds.Count);
    }

    // ---- R6: close / heal --------------------------------------------------------------------

    [Fact] // Covers R6.
    public void CloseTab_on_multi_tab_pane_keeps_pane_and_moves_selection()
    {
        var c = new SplitTreeController();
        PaneId pane = c.FocusedPane;
        c.NewTab(pane); // pane now has 2 tabs, second selected
        IReadOnlyList<SurfaceId> tabs = c.Tabs(pane);
        SurfaceId selected = c.SelectedTab(pane)!.Value;
        Assert.Equal(2, tabs.Count);

        c.CloseTab(selected);

        Assert.Single(c.Tabs(pane));                 // pane survives with one tab
        Assert.Equal(pane, c.FocusedPane);
        Assert.NotEqual(selected, c.SelectedTab(pane)!.Value); // selection moved to the adjacent tab
    }

    [Fact] // Covers R6.
    public void CloseTab_on_non_root_last_tab_promotes_sibling_and_reconciles_focus()
    {
        var c = new SplitTreeController();
        c.Split(c.FocusedPane, Orientation.Vertical);   // root B1{P1, P2}; focus P2
        c.Split(c.FocusedPane, Orientation.Horizontal); // root B1{P1, B2{P2, P3}}; focus P3
        Assert.Equal(2, BranchCount(c.Snapshot().Root));

        PaneId removed = c.FocusedPane;                 // P3
        SurfaceId surface = c.FocusedSurface!.Value;
        c.CloseTab(surface);

        SplitNode? root = c.Snapshot().Root;
        Assert.Equal(2, LeafCount(root));               // P3 gone
        Assert.Equal(1, BranchCount(root));             // B2 collapsed (its divider is gone)
        Assert.DoesNotContain(removed, c.AllPaneIds);
        Assert.Contains(c.FocusedPane, c.AllPaneIds);   // focus reconciled to a survivor
    }

    [Fact] // Covers R6.
    public void CloseTab_on_only_panes_last_tab_empties_tree_and_signals_once()
    {
        var c = new SplitTreeController();
        int emptied = 0;
        c.Emptied += () => emptied++;
        SurfaceId only = c.FocusedSurface!.Value;

        c.CloseTab(only);

        Assert.True(c.IsEmpty);
        Assert.Null(c.Snapshot().Root);
        Assert.Equal(1, emptied);
    }

    [Fact] // Covers R6 — host re-seed path.
    public void SeedRoot_after_empty_restores_a_single_focused_pane()
    {
        var c = new SplitTreeController();
        c.CloseTab(c.FocusedSurface!.Value);
        Assert.True(c.IsEmpty);

        SurfaceId seeded = c.SeedRoot();

        Assert.False(c.IsEmpty);
        PaneLeaf leaf = Assert.IsType<PaneLeaf>(c.Snapshot().Root);
        Assert.Equal(seeded, leaf.Selected);
        Assert.Equal(leaf.Id, c.FocusedPane);
    }

    // ---- R5: dividers ------------------------------------------------------------------------

    [Theory] // Covers R5.
    [InlineData(-0.4, 0.0)]
    [InlineData(1.7, 1.0)]
    [InlineData(0.3, 0.3)]
    public void SetDividerPosition_clamps_to_unit_interval(double input, double expected)
    {
        var c = new SplitTreeController();
        c.Split(c.FocusedPane, Orientation.Vertical);
        BranchId branch = ((SplitBranch)c.Snapshot().Root!).Id;

        c.SetDividerPosition(branch, input);

        Assert.Equal(expected, ((SplitBranch)c.Snapshot().Root!).DividerPosition);
    }

    [Fact] // Covers R5.
    public void Equalize_resets_every_divider_in_a_nested_tree()
    {
        var c = new SplitTreeController();
        c.Split(c.FocusedPane, Orientation.Vertical);
        c.Split(c.FocusedPane, Orientation.Horizontal);
        foreach (SplitBranch b in Branches(c.Snapshot().Root))
        {
            c.SetDividerPosition(b.Id, 0.2);
        }
        Assert.All(Branches(c.Snapshot().Root), b => Assert.Equal(0.2, b.DividerPosition));

        c.Equalize();

        Assert.All(Branches(c.Snapshot().Root), b => Assert.Equal(0.5, b.DividerPosition));
    }

    // ---- R7: derived focus + tab cycling -----------------------------------------------------

    [Fact] // Covers R7.
    public void FocusedSurface_is_derived_from_selected_tab()
    {
        var c = new SplitTreeController();
        PaneId pane = c.FocusedPane;
        c.NewTab(pane);
        c.NewTab(pane);
        IReadOnlyList<SurfaceId> tabs = c.Tabs(pane);

        c.SelectTab(pane, tabs[0]);

        Assert.Equal(tabs[0], c.SelectedTab(pane));
        Assert.Equal(c.SelectedTab(c.FocusedPane), c.FocusedSurface); // never stored separately
    }

    [Fact] // Covers R7.
    public void SelectNextPreviousTab_wraps_within_focused_pane_only()
    {
        var c = new SplitTreeController();
        PaneId left = c.FocusedPane;
        c.Split(left, Orientation.Vertical); // focus moves to the new right pane
        PaneId right = c.FocusedPane;

        // Give the right (focused) pane three tabs.
        c.NewTab(right);
        c.NewTab(right);
        IReadOnlyList<SurfaceId> tabs = c.Tabs(right);
        c.SelectTab(right, tabs[0]);

        c.SelectPreviousTab(); // wraps to the last tab
        Assert.Equal(tabs[^1], c.SelectedTab(right));
        c.SelectNextTab();     // wraps back to the first
        Assert.Equal(tabs[0], c.SelectedTab(right));

        // The other pane is never touched and focus never crosses panes.
        Assert.Equal(right, c.FocusedPane);
        Assert.Single(c.Tabs(left));
    }

    // ---- R12 / KTD6: snapshot + id stability -------------------------------------------------

    [Fact] // Covers R12 / KTD6.
    public void Captured_snapshot_is_unchanged_by_later_mutations()
    {
        var c = new SplitTreeController();
        TreeSnapshot snap = c.Snapshot();
        PaneLeaf before = Assert.IsType<PaneLeaf>(snap.Root);

        c.Split(before.Id, Orientation.Vertical);
        c.NewTab(c.FocusedPane);

        // The previously captured snapshot still describes the single-leaf tree.
        PaneLeaf stillLeaf = Assert.IsType<PaneLeaf>(snap.Root);
        Assert.Single(stillLeaf.Tabs);
        Assert.IsType<SplitBranch>(c.Snapshot().Root); // the live tree has moved on
    }

    [Fact] // Covers KTD6.
    public void Closed_ids_are_never_reissued()
    {
        var c = new SplitTreeController();
        SurfaceId s1 = c.FocusedSurface!.Value;
        c.Split(c.FocusedPane, Orientation.Vertical);
        SurfaceId s2 = c.FocusedSurface!.Value;

        c.CloseTab(s2);              // removes the new pane + surface s2
        c.NewTab(c.FocusedPane);
        SurfaceId s3 = c.FocusedSurface!.Value;

        Assert.NotEqual(s1, s3);
        Assert.NotEqual(s2, s3);
        Assert.True(s3.Value > s2.Value); // monotonic, never reused
    }

    // ---- U7: zoom (transient view state) -----------------------------------------------------

    [Fact] // Covers U7.
    public void ToggleZoom_zooms_the_focused_pane_and_toggles_off()
    {
        var c = new SplitTreeController();
        c.Split(c.FocusedPane, Orientation.Vertical);
        PaneId focused = c.FocusedPane;

        Assert.Null(c.ZoomedPane);
        c.ToggleZoom();
        Assert.Equal(focused, c.ZoomedPane);
        Assert.Equal(focused, c.Snapshot().ZoomedPane); // carried on the snapshot
        c.ToggleZoom();
        Assert.Null(c.ZoomedPane);
    }

    [Fact] // Covers U7.
    public void ClearZoom_clears_when_zoomed_and_is_a_noop_otherwise()
    {
        var c = new SplitTreeController();
        c.ToggleZoom();
        Assert.NotNull(c.ZoomedPane);

        c.ClearZoom();
        Assert.Null(c.ZoomedPane);

        c.ClearZoom(); // no-op, must not throw
        Assert.Null(c.ZoomedPane);
    }

    [Fact] // Covers U7 — zoom must not survive its pane being healed away.
    public void Closing_the_zoomed_pane_clears_the_zoom()
    {
        var c = new SplitTreeController();
        c.Split(c.FocusedPane, Orientation.Vertical); // focus the new pane
        c.ToggleZoom();
        Assert.Equal(c.FocusedPane, c.ZoomedPane);

        c.CloseTab(c.FocusedSurface!.Value); // remove the zoomed pane's last tab → pane heals away

        Assert.Null(c.ZoomedPane);
    }

    // ---- Surface lifecycle events (host-wiring contract; supports U2) ------------------------

    [Fact]
    public void Split_and_close_raise_surface_created_and_closed_events()
    {
        var c = new SplitTreeController();
        var created = new List<SurfaceId>();
        var closed = new List<SurfaceId>();
        c.SurfaceCreated += created.Add;
        c.SurfaceClosed += closed.Add;

        c.Split(c.FocusedPane, Orientation.Vertical);
        SurfaceId newSurface = c.FocusedSurface!.Value;
        Assert.Equal(new[] { newSurface }, created);

        c.CloseTab(newSurface);
        Assert.Contains(newSurface, closed);
    }

    // ---- Shared id allocation (Phase 5: multiple workspaces, globally unique ids) -------------

    [Fact]
    public void Controllers_sharing_an_allocator_never_mint_colliding_ids()
    {
        var ids = new IdAllocator();
        var a = new SplitTreeController(ids);
        var b = new SplitTreeController(ids);

        a.NewTab(a.FocusedPane);
        b.Split(b.FocusedPane, Orientation.Horizontal);
        b.NewTab(b.FocusedPane);

        var surfaces = a.AllSurfaces.Concat(b.AllSurfaces).ToList();
        Assert.Equal(surfaces.Count, surfaces.Distinct().Count());

        var panes = a.AllPaneIds.Concat(b.AllPaneIds).ToList();
        Assert.Equal(panes.Count, panes.Distinct().Count());
    }
}
