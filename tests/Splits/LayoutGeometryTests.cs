using System.Collections.Generic;
using System.Collections.Immutable;
using Cmux.Core;
using Xunit;

namespace Cmux.Core.Tests;

/// <summary>
/// Coverage for the pure geometry (plan Phase 2 U1): leaf-rect computation, directional pane
/// navigation, and the star-ratio ↔ divider-fraction conversion the WinUI GridSplitter relies on
/// (U3).
/// </summary>
public sealed class LayoutGeometryTests
{
    private static PaneLeaf Pane(int id) =>
        new(new PaneId(id), ImmutableList.Create(new SurfaceId(id)), new SurfaceId(id));

    private static SplitBranch Branch(int id, Orientation o, SplitNode first, SplitNode second, double divider) =>
        new(new BranchId(id), o, first, second, divider);

    // ---- ComputeLeafRects --------------------------------------------------------------------

    [Fact]
    public void Vertical_split_lays_panes_side_by_side()
    {
        SplitNode root = Branch(1, Orientation.Vertical, Pane(1), Pane(2), 0.5);

        IReadOnlyDictionary<PaneId, LayoutRect> rects =
            LayoutGeometry.ComputeLeafRects(root, new LayoutRect(0, 0, 100, 100));

        Assert.Equal(new LayoutRect(0, 0, 50, 100), rects[new PaneId(1)]);
        Assert.Equal(new LayoutRect(50, 0, 50, 100), rects[new PaneId(2)]);
    }

    [Fact]
    public void Horizontal_split_stacks_panes_with_divider_fraction()
    {
        SplitNode root = Branch(1, Orientation.Horizontal, Pane(1), Pane(2), 0.25);

        IReadOnlyDictionary<PaneId, LayoutRect> rects =
            LayoutGeometry.ComputeLeafRects(root, new LayoutRect(0, 0, 100, 100));

        Assert.Equal(new LayoutRect(0, 0, 100, 25), rects[new PaneId(1)]);
        Assert.Equal(new LayoutRect(0, 25, 100, 75), rects[new PaneId(2)]);
    }

    // ---- NavigateFrom ------------------------------------------------------------------------

    [Fact]
    public void MoveFocus_right_from_left_pane_of_vertical_split_finds_right_pane()
    {
        SplitNode root = Branch(1, Orientation.Vertical, Pane(1), Pane(2), 0.5);

        Assert.Equal(new PaneId(2), LayoutGeometry.NavigateFrom(root, new PaneId(1), Direction.Right));
        Assert.Equal(new PaneId(1), LayoutGeometry.NavigateFrom(root, new PaneId(2), Direction.Left));
    }

    [Fact]
    public void MoveFocus_with_no_pane_in_direction_is_noop()
    {
        SplitNode root = Branch(1, Orientation.Vertical, Pane(1), Pane(2), 0.5);

        Assert.Null(LayoutGeometry.NavigateFrom(root, new PaneId(1), Direction.Left));
        Assert.Null(LayoutGeometry.NavigateFrom(root, new PaneId(1), Direction.Up));
        Assert.Null(LayoutGeometry.NavigateFrom(root, new PaneId(2), Direction.Right));
    }

    [Fact]
    public void Navigation_in_nested_tree_picks_directionally_correct_leaf()
    {
        // horizontal{ top = vertical{ p1, p2 }, bottom = p3 }
        SplitNode root = Branch(
            1,
            Orientation.Horizontal,
            Branch(2, Orientation.Vertical, Pane(1), Pane(2), 0.5),
            Pane(3),
            0.5);

        Assert.Equal(new PaneId(2), LayoutGeometry.NavigateFrom(root, new PaneId(1), Direction.Right));
        Assert.Equal(new PaneId(1), LayoutGeometry.NavigateFrom(root, new PaneId(2), Direction.Left));
        Assert.Equal(new PaneId(3), LayoutGeometry.NavigateFrom(root, new PaneId(1), Direction.Down));
        Assert.Equal(new PaneId(3), LayoutGeometry.NavigateFrom(root, new PaneId(2), Direction.Down));
        Assert.Null(LayoutGeometry.NavigateFrom(root, new PaneId(1), Direction.Up));
    }

    [Fact]
    public void Navigation_prefers_the_geometrically_nearest_overlapping_leaf()
    {
        // vertical{ left = p1 (full height), right = horizontal{ p2 (thin top), p3 (tall bottom) } }
        // p1's vertical center (0.5) is nearer p3's center (0.6) than p2's (0.1), so Right → p3.
        SplitNode root = Branch(
            1,
            Orientation.Vertical,
            Pane(1),
            Branch(2, Orientation.Horizontal, Pane(2), Pane(3), 0.2),
            0.5);

        Assert.Equal(new PaneId(3), LayoutGeometry.NavigateFrom(root, new PaneId(1), Direction.Right));
    }

    // ---- Star ratio ↔ fraction ---------------------------------------------------------------

    [Theory]
    [InlineData(1.0, 1.0, 0.5)]
    [InlineData(3.0, 1.0, 0.75)]
    [InlineData(1.0, 3.0, 0.25)]
    [InlineData(0.0, 0.0, 0.5)] // degenerate → centered
    public void FractionFromStars_maps_track_weights_to_unit_fraction(double a, double b, double expected)
    {
        Assert.Equal(expected, LayoutGeometry.FractionFromStars(a, b), precision: 9);
    }

    [Fact]
    public void FractionFromStars_round_trips_with_the_views_star_mapping()
    {
        const double fraction = 0.3;
        // The view sizes tracks as `fraction*` / `(1-fraction)*`; reading them back must recover it.
        double recovered = LayoutGeometry.FractionFromStars(fraction, 1 - fraction);
        Assert.Equal(fraction, recovered, precision: 9);
    }

    [Theory]
    [InlineData(-0.5, 0.0)]
    [InlineData(1.5, 1.0)]
    [InlineData(0.42, 0.42)]
    public void Clamp01_constrains_to_unit_interval(double input, double expected)
    {
        Assert.Equal(expected, LayoutGeometry.Clamp01(input), precision: 9);
    }
}
