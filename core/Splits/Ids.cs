namespace Optimus.Core;

/// <summary>
/// Identity of a pane (a leaf in the split tree that owns an ordered tab list). Wraps a
/// monotonically increasing <see cref="int"/> issued by <see cref="SplitTreeController"/> and
/// <b>never reused</b> within a session (KTD6), so a stale snapshot can never alias a pane that
/// was created after the snapshot was captured (R12).
/// </summary>
public readonly record struct PaneId(int Value)
{
    public override string ToString() => $"P{Value}";
}

/// <summary>
/// Identity of a surface — one terminal, i.e. one engine instance (ConPTY + render thread),
/// presented as one tab. Monotonic and never reused within a session (KTD6).
/// </summary>
public readonly record struct SurfaceId(int Value)
{
    public override string ToString() => $"S{Value}";
}

/// <summary>
/// Identity of a split branch (an interior node). Lets the view map a divider drag back to the
/// exact branch regardless of how the tree is later re-shaped — the Windows analog of bonsplit's
/// per-split UUID (optimus/Sources/Workspace.swift <c>setDividerPosition(_:forSplit:)</c>). Stable
/// across rewrites: an operation that does not touch a branch preserves its id.
/// </summary>
public readonly record struct BranchId(int Value)
{
    public override string ToString() => $"B{Value}";
}

/// <summary>
/// Mints pane/surface/branch ids, monotonic and never reused (KTD6). Phase 5 introduces multiple
/// workspaces, each with its own <see cref="SplitTreeController"/>; sharing one allocator across all
/// of them keeps <see cref="SurfaceId"/>s globally unique, so a surface id alone (e.g. from
/// <c>OPTIMUS_SURFACE_ID</c> over the Phase-4 pipe) unambiguously identifies its workspace.
/// </summary>
public sealed class IdAllocator
{
    private int _pane;
    private int _surface;
    private int _branch;

    public PaneId NextPane() => new(++_pane);
    public SurfaceId NextSurface() => new(++_surface);
    public BranchId NextBranch() => new(++_branch);
}
