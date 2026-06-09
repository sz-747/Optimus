using System;
using System.Collections.Generic;
using System.Collections.Immutable;
using System.Linq;

namespace Cmux.Core;

/// <summary>
/// The mutable controller over an immutable split tree (KTD3). Holds the current root plus the
/// focused pane, exposes the split/tab/divider/focus operations (a port of the macOS bonsplit
/// controller — cmux/Sources/Workspace.swift), and emits value-type <see cref="TreeSnapshot"/>s.
///
/// <para><b>Derived focus (R7):</b> the focused <i>surface</i> is never stored — it is always
/// <c>SelectedTab(FocusedPane)</c>. Changing the selected tab changes the focused surface.</para>
///
/// <para><b>Surface lifecycle</b> is delegated to the host: the controller knows only IDs. It
/// raises <see cref="SurfaceCreated"/> when an operation introduces a new surface and
/// <see cref="SurfaceClosed"/> when one is removed, so the host can create/dispose the matching
/// engine without the model ever touching WinUI or the FFI.</para>
/// </summary>
public sealed class SplitTreeController
{
    private SplitNode? _root;
    private PaneId _focusedPane;
    private PaneId? _zoomedPane;

    private readonly IdAllocator _ids;
    private int _version;

    /// <summary>Raised after any change to the tree, focus, or zoom. Carries the fresh snapshot.</summary>
    public event Action<TreeSnapshot>? SnapshotChanged;

    /// <summary>Raised when an operation introduces a new surface; the host creates its engine.</summary>
    public event Action<SurfaceId>? SurfaceCreated;

    /// <summary>Raised when a surface is removed; the host disposes its engine (R2/R9).</summary>
    public event Action<SurfaceId>? SurfaceClosed;

    /// <summary>
    /// Raised when the last surface of the last pane closes and the tree becomes empty (R6). The
    /// host re-seeds via <see cref="SeedRoot"/> so the window never goes contentless.
    /// </summary>
    public event Action? Emptied;

    /// <summary>
    /// Create a controller seeded with one pane holding one surface, that pane focused. The seed
    /// does <b>not</b> raise events (there are no subscribers yet); the host reads
    /// <see cref="AllSurfaces"/> after construction to create the initial engine.
    /// <paramref name="ids"/> lets multiple workspaces share one allocator so surface ids stay
    /// globally unique (Phase 5); when omitted the controller mints from its own.
    /// </summary>
    public SplitTreeController(IdAllocator? ids = null)
    {
        _ids = ids ?? new IdAllocator();
        SurfaceId surface = NextSurface();
        var pane = new PaneLeaf(NextPane(), ImmutableList.Create(surface), surface);
        _root = pane;
        _focusedPane = pane.Id;
    }

    // ---- Queries (derived; nothing cached) ---------------------------------------------------

    /// <summary><c>true</c> when there are no panes (closed the very last surface — R6).</summary>
    public bool IsEmpty => _root is null;

    /// <summary>The focused pane.</summary>
    public PaneId FocusedPane => _focusedPane;

    /// <summary>The pane rendered full-bleed when zoomed (U7), or <c>null</c>.</summary>
    public PaneId? ZoomedPane => _zoomedPane;

    /// <summary>The focused surface — <b>derived</b> as <c>SelectedTab(FocusedPane)</c> (R7).</summary>
    public SurfaceId? FocusedSurface => _root.FindPane(_focusedPane)?.Selected;

    /// <summary>Every pane id, in document order.</summary>
    public IReadOnlyList<PaneId> AllPaneIds =>
        _root is null ? Array.Empty<PaneId>() : _root.Leaves().Select(l => l.Id).ToImmutableArray();

    /// <summary>Every surface id across all panes, in document order.</summary>
    public IReadOnlyList<SurfaceId> AllSurfaces =>
        _root is null ? Array.Empty<SurfaceId>() : _root.Leaves().SelectMany(l => l.Tabs).ToImmutableArray();

    /// <summary>The tab list of a pane (empty if the pane does not exist).</summary>
    public IReadOnlyList<SurfaceId> Tabs(PaneId pane) =>
        (IReadOnlyList<SurfaceId>?)_root.FindPane(pane)?.Tabs ?? ImmutableArray<SurfaceId>.Empty;

    /// <summary>The selected surface of a pane, or <c>null</c> if the pane does not exist.</summary>
    public SurfaceId? SelectedTab(PaneId pane) => _root.FindPane(pane)?.Selected;

    /// <summary>Capture an immutable snapshot of the current state.</summary>
    public TreeSnapshot Snapshot() => new(_root, _focusedPane, _zoomedPane, _version);

    // ---- Structural operations ---------------------------------------------------------------

    /// <summary>
    /// Split <paramref name="pane"/> into a branch of the given <paramref name="orientation"/>,
    /// adding a new pane with one new surface. The new pane becomes <c>First</c> when
    /// <paramref name="insertFirst"/> is set, else <c>Second</c>; the new pane is focused (R4).
    /// </summary>
    public void Split(PaneId pane, Orientation orientation, bool insertFirst = false)
    {
        PaneLeaf? leaf = _root.FindPane(pane);
        if (leaf is null)
        {
            return;
        }

        SurfaceId surface = NextSurface();
        var newLeaf = new PaneLeaf(NextPane(), ImmutableList.Create(surface), surface);
        SplitNode branch = insertFirst
            ? new SplitBranch(NextBranch(), orientation, newLeaf, leaf, 0.5)
            : new SplitBranch(NextBranch(), orientation, leaf, newLeaf, 0.5);

        _root = ReplaceNode(_root!, pane, branch);
        _focusedPane = newLeaf.Id;
        SurfaceCreated?.Invoke(surface);
        Emit();
    }

    /// <summary>Add a new surface (tab) to <paramref name="pane"/>, select it, and focus the pane (R1).</summary>
    public void NewTab(PaneId pane)
    {
        PaneLeaf? leaf = _root.FindPane(pane);
        if (leaf is null)
        {
            return;
        }

        SurfaceId surface = NextSurface();
        _root = ReplaceNode(_root!, pane, leaf with { Tabs = leaf.Tabs.Add(surface), Selected = surface });
        _focusedPane = pane;
        SurfaceCreated?.Invoke(surface);
        Emit();
    }

    /// <summary>
    /// Close the tab backing <paramref name="surface"/>. If the pane has other tabs, the surface is
    /// removed and selection moves to an adjacent tab (R2). If it was the pane's last tab, the pane
    /// is removed and the tree heals — the sibling is promoted into the parent's slot (R6) — and
    /// focus reconciles to a surviving pane. Closing the last surface of the last pane empties the
    /// tree and raises <see cref="Emptied"/>.
    /// </summary>
    public void CloseTab(SurfaceId surface)
    {
        PaneLeaf? leaf = _root.FindContaining(surface);
        if (leaf is null)
        {
            return;
        }

        if (leaf.Tabs.Count > 1)
        {
            int idx = leaf.Tabs.IndexOf(surface);
            ImmutableList<SurfaceId> remaining = leaf.Tabs.RemoveAt(idx);
            SurfaceId selected = leaf.Selected;
            if (leaf.Selected == surface)
            {
                // Prefer the tab that shifts into the closed slot; fall back to the new last tab.
                selected = remaining[Math.Min(idx, remaining.Count - 1)];
            }
            _root = ReplaceNode(_root!, leaf.Id, leaf with { Tabs = remaining, Selected = selected });
            SurfaceClosed?.Invoke(surface);
            Emit();
            return;
        }

        // Last tab in the pane → remove the pane.
        SurfaceClosed?.Invoke(surface);

        if (_root is PaneLeaf rootLeaf && rootLeaf.Id == leaf.Id)
        {
            // The root itself was the only pane → the tree is now empty.
            _root = null;
            _zoomedPane = null;
            Emptied?.Invoke();
            return;
        }

        _root = RemoveLeafPromotingSibling(_root!, leaf.Id, out PaneId? survivor);
        if (_zoomedPane == leaf.Id)
        {
            _zoomedPane = null;
        }
        if (_root.FindPane(_focusedPane) is null)
        {
            _focusedPane = survivor ?? _root!.Leaves().First().Id;
        }
        Emit();
    }

    /// <summary>Select <paramref name="surface"/> within <paramref name="pane"/> and focus the pane (R3).</summary>
    public void SelectTab(PaneId pane, SurfaceId surface)
    {
        PaneLeaf? leaf = _root.FindPane(pane);
        if (leaf is null || !leaf.Tabs.Contains(surface))
        {
            return;
        }
        if (leaf.Selected != surface)
        {
            _root = ReplaceNode(_root!, pane, leaf with { Selected = surface });
        }
        _focusedPane = pane;
        Emit();
    }

    /// <summary>Select the next tab in the focused pane, wrapping; never crosses panes (R7).</summary>
    public void SelectNextTab() => CycleTab(+1);

    /// <summary>Select the previous tab in the focused pane, wrapping; never crosses panes (R7).</summary>
    public void SelectPreviousTab() => CycleTab(-1);

    private void CycleTab(int step)
    {
        PaneLeaf? leaf = _root.FindPane(_focusedPane);
        if (leaf is null || leaf.Tabs.Count <= 1)
        {
            return;
        }
        int n = leaf.Tabs.Count;
        int idx = leaf.Tabs.IndexOf(leaf.Selected);
        int next = (((idx + step) % n) + n) % n;
        _root = ReplaceNode(_root!, leaf.Id, leaf with { Selected = leaf.Tabs[next] });
        Emit();
    }

    /// <summary>Focus a pane explicitly (e.g. a pointer click). No-op if the pane does not exist.</summary>
    public void FocusPane(PaneId pane)
    {
        if (_root.FindPane(pane) is null)
        {
            return;
        }
        _focusedPane = pane;
        Emit();
    }

    /// <summary>
    /// Move focus to the nearest pane in <paramref name="direction"/> (R8). No-op (no snapshot) when
    /// there is no pane that way.
    /// </summary>
    public void MoveFocus(Direction direction)
    {
        if (_root is null)
        {
            return;
        }
        PaneId? target = LayoutGeometry.NavigateFrom(_root, _focusedPane, direction);
        if (target is PaneId t && t != _focusedPane)
        {
            _focusedPane = t;
            Emit();
        }
    }

    /// <summary>Set a branch's divider fraction, clamped to [0,1] (R5/KTD7).</summary>
    public void SetDividerPosition(BranchId branch, double fraction)
    {
        if (_root is null)
        {
            return;
        }
        double clamped = LayoutGeometry.Clamp01(fraction);
        SplitNode updated = SetDivider(_root, branch, clamped, out bool changed);
        if (changed)
        {
            _root = updated;
            Emit();
        }
    }

    /// <summary>Reset every branch divider to 0.5 (R5).</summary>
    public void Equalize()
    {
        if (_root is null)
        {
            return;
        }
        _root = EqualizeNode(_root);
        Emit();
    }

    /// <summary>Toggle full-bleed zoom of the focused pane (U7). Transient view state, not tree shape.</summary>
    public void ToggleZoom()
    {
        if (_root is null)
        {
            return;
        }
        _zoomedPane = _zoomedPane == _focusedPane ? null : _focusedPane;
        Emit();
    }

    /// <summary>Clear any active zoom (U7).</summary>
    public void ClearZoom()
    {
        if (_zoomedPane is null)
        {
            return;
        }
        _zoomedPane = null;
        Emit();
    }

    /// <summary>
    /// Re-seed a fresh root pane + surface after the tree emptied (R6). Returns the new surface so
    /// the host can create its engine. Raises <see cref="SurfaceCreated"/> and a snapshot.
    /// </summary>
    public SurfaceId SeedRoot()
    {
        SurfaceId surface = NextSurface();
        var pane = new PaneLeaf(NextPane(), ImmutableList.Create(surface), surface);
        _root = pane;
        _focusedPane = pane.Id;
        _zoomedPane = null;
        SurfaceCreated?.Invoke(surface);
        Emit();
        return surface;
    }

    // ---- Tree rewrites (pure, immutable) -----------------------------------------------------

    private static SplitNode ReplaceNode(SplitNode node, PaneId target, SplitNode replacement)
    {
        switch (node)
        {
            case PaneLeaf leaf:
                return leaf.Id == target ? replacement : leaf;
            case SplitBranch branch:
                SplitNode first = ReplaceNode(branch.First, target, replacement);
                SplitNode second = ReplaceNode(branch.Second, target, replacement);
                return ReferenceEquals(first, branch.First) && ReferenceEquals(second, branch.Second)
                    ? branch
                    : branch with { First = first, Second = second };
            default:
                return node;
        }
    }

    /// <summary>
    /// Remove the leaf <paramref name="target"/> by promoting its sibling into the parent branch's
    /// slot (R6). <paramref name="survivor"/> receives the first leaf of the promoted subtree, used
    /// to reconcile focus when the focused pane was the one removed.
    /// </summary>
    private static SplitNode RemoveLeafPromotingSibling(SplitNode node, PaneId target, out PaneId? survivor)
    {
        survivor = null;
        if (node is not SplitBranch branch)
        {
            return node;
        }

        if (branch.First is PaneLeaf fl && fl.Id == target)
        {
            survivor = branch.Second.Leaves().First().Id;
            return branch.Second;
        }
        if (branch.Second is PaneLeaf sl && sl.Id == target)
        {
            survivor = branch.First.Leaves().First().Id;
            return branch.First;
        }

        SplitNode first = RemoveLeafPromotingSibling(branch.First, target, out PaneId? s1);
        if (!ReferenceEquals(first, branch.First))
        {
            survivor = s1;
            return branch with { First = first };
        }
        SplitNode second = RemoveLeafPromotingSibling(branch.Second, target, out PaneId? s2);
        if (!ReferenceEquals(second, branch.Second))
        {
            survivor = s2;
            return branch with { Second = second };
        }
        return node;
    }

    private static SplitNode SetDivider(SplitNode node, BranchId id, double value, out bool changed)
    {
        changed = false;
        if (node is not SplitBranch branch)
        {
            return node;
        }
        if (branch.Id == id)
        {
            changed = branch.DividerPosition != value;
            return changed ? branch with { DividerPosition = value } : branch;
        }
        SplitNode first = SetDivider(branch.First, id, value, out bool c1);
        if (c1)
        {
            changed = true;
            return branch with { First = first };
        }
        SplitNode second = SetDivider(branch.Second, id, value, out bool c2);
        if (c2)
        {
            changed = true;
            return branch with { Second = second };
        }
        return node;
    }

    private static SplitNode EqualizeNode(SplitNode node) => node switch
    {
        SplitBranch branch => branch with
        {
            First = EqualizeNode(branch.First),
            Second = EqualizeNode(branch.Second),
            DividerPosition = 0.5,
        },
        _ => node,
    };

    // ---- Id minting + emit -------------------------------------------------------------------

    private PaneId NextPane() => _ids.NextPane();
    private SurfaceId NextSurface() => _ids.NextSurface();
    private BranchId NextBranch() => _ids.NextBranch();

    private void Emit()
    {
        _version++;
        SnapshotChanged?.Invoke(Snapshot());
    }
}
