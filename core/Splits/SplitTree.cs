using System.Collections.Generic;
using System.Collections.Immutable;
using System.Linq;

namespace Cmux.Core;

/// <summary>
/// Split orientation. The convention is stated explicitly here (KTD7) to avoid the perennial axis
/// confusion:
/// <list type="bullet">
///   <item><see cref="Vertical"/> — panes sit <b>side-by-side</b>, separated by a <b>vertical</b>
///   divider. This is the "split right" action.</item>
///   <item><see cref="Horizontal"/> — panes are <b>stacked</b>, separated by a <b>horizontal</b>
///   divider. This is the "split down" action.</item>
/// </list>
/// </summary>
public enum Orientation
{
    Vertical,
    Horizontal,
}

/// <summary>Direction for keyboard pane-focus navigation (R8).</summary>
public enum Direction
{
    Left,
    Right,
    Up,
    Down,
}

/// <summary>
/// An immutable node in the split tree — either a <see cref="PaneLeaf"/> or a
/// <see cref="SplitBranch"/>. Operations never mutate a node; they build new nodes and the
/// controller swaps its root, which is what makes a captured <see cref="TreeSnapshot"/> stable
/// (R12).
/// </summary>
public abstract record SplitNode;

/// <summary>
/// A leaf pane: an ordered tab list of surfaces plus the currently selected surface. The selected
/// surface is the only one composited for this pane (R3/R11). Closing the leaf's last tab removes
/// the pane and heals the tree (R6).
/// </summary>
public sealed record PaneLeaf(PaneId Id, ImmutableList<SurfaceId> Tabs, SurfaceId Selected) : SplitNode;

/// <summary>
/// An interior split: two children separated by a divider. <see cref="DividerPosition"/> is the
/// first child's share, a fraction in [0,1] clamped on every write (KTD7); the second child gets
/// <c>1 - DividerPosition</c>.
/// </summary>
public sealed record SplitBranch(
    BranchId Id,
    Orientation Orientation,
    SplitNode First,
    SplitNode Second,
    double DividerPosition) : SplitNode;

/// <summary>
/// An immutable value-type snapshot of the controller state handed to the view (KTD3/KTD5). Because
/// <see cref="Root"/> is an immutable node tree, a captured snapshot is unaffected by subsequent
/// controller mutations. <see cref="Root"/> is <c>null</c> when the tree is empty (the host re-seeds
/// — see <see cref="SplitTreeController.Emptied"/>).
/// </summary>
public sealed record TreeSnapshot(SplitNode? Root, PaneId FocusedPane, PaneId? ZoomedPane, int Version);

/// <summary>Pure read-only queries over a node tree, shared by the controller and the view.</summary>
public static class SplitNodeExtensions
{
    /// <summary>Every leaf pane in document order (first-before-second, depth-first).</summary>
    public static IEnumerable<PaneLeaf> Leaves(this SplitNode node)
    {
        switch (node)
        {
            case PaneLeaf leaf:
                yield return leaf;
                break;
            case SplitBranch branch:
                foreach (PaneLeaf l in branch.First.Leaves())
                {
                    yield return l;
                }
                foreach (PaneLeaf l in branch.Second.Leaves())
                {
                    yield return l;
                }
                break;
        }
    }

    /// <summary>The leaf with the given <see cref="PaneId"/>, or <c>null</c>.</summary>
    public static PaneLeaf? FindPane(this SplitNode? node, PaneId id) =>
        node?.Leaves().FirstOrDefault(l => l.Id == id);

    /// <summary>The leaf whose tab list contains <paramref name="surface"/>, or <c>null</c>.</summary>
    public static PaneLeaf? FindContaining(this SplitNode? node, SurfaceId surface) =>
        node?.Leaves().FirstOrDefault(l => l.Tabs.Contains(surface));
}
