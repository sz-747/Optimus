using System;
using System.Collections.Generic;
using System.Linq;
using CommunityToolkit.WinUI.Controls;
using Cmux.Core;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Windows.UI;

namespace Cmux.Splits;

/// <summary>
/// Renders a <see cref="TreeSnapshot"/> into a nested <see cref="Grid"/> / <see cref="GridSplitter"/>
/// visual (plan Phase 2 U3). Branches become a 3-track grid — <c>fraction*</c>, a fixed splitter
/// track, <c>(1-fraction)*</c> (KTD7) — and leaves become the caller-supplied pane view.
///
/// <para><b>Re-parent, don't rebuild (R10):</b> pane views are cached by <see cref="PaneId"/>. On
/// every render the same instances are detached from their old grids and placed into freshly built
/// ones, so a surviving surface's <c>SwapChainPanel</c>/engine is never recreated — only moved.
/// Structural changes (split/close/divider-commit) are infrequent, so a full grid rebuild is
/// acceptable; the lifecycle guard (U2) keeps re-parenting safe.</para>
/// </summary>
internal sealed class SplitTreeView : Grid
{
    private const double DividerThickness = 6.0;

    private readonly Func<PaneId, FrameworkElement> _paneFactory;
    private readonly Action<BranchId, double> _onDividerChanged;
    private readonly Dictionary<PaneId, FrameworkElement> _paneViews = new();

    // The (root, zoom) pair last materialized into the visual tree. Tree rewrites are immutable and
    // focus lives *outside* the node tree, so a focus-only change (FocusPane/MoveFocus) leaves
    // snapshot.Root reference-identical to what we last rendered. Detecting that lets us skip the
    // whole detach/rebuild, which is both a latency win (no re-parenting live SwapChainPanels for a
    // change that doesn't alter layout) and a correctness fix: a strip button raises GotFocus →
    // FocusPane → snapshot *before* its own Click fires, and re-parenting the pane subtree mid-click
    // would otherwise swallow that Click and the action would never dispatch (R4).
    private SplitNode? _renderedRoot;
    private PaneId? _renderedZoom;
    private bool _hasRendered;

    /// <param name="paneFactory">Builds the leaf view for a pane (a <c>PaneView</c> in U4+).</param>
    /// <param name="onDividerChanged">Called with the committed [0,1] fraction after a divider drag.</param>
    public SplitTreeView(Func<PaneId, FrameworkElement> paneFactory, Action<BranchId, double> onDividerChanged)
    {
        _paneFactory = paneFactory;
        _onDividerChanged = onDividerChanged;
    }

    /// <summary>Rebuild the visual tree from <paramref name="snapshot"/>, reusing cached pane views.</summary>
    public void Render(TreeSnapshot snapshot)
    {
        // Focus-only change → identical layout. Skip the rebuild so we neither re-parent panes
        // (which would swallow an in-flight strip-button Click — R4) nor pay the composition churn.
        if (_hasRendered
            && ReferenceEquals(snapshot.Root, _renderedRoot)
            && snapshot.ZoomedPane == _renderedZoom)
        {
            return;
        }
        _renderedRoot = snapshot.Root;
        _renderedZoom = snapshot.ZoomedPane;
        _hasRendered = true;

        // Detach every cached view from its current parent so it can be re-placed (a WinUI element
        // may only have one parent at a time).
        foreach (FrameworkElement view in _paneViews.Values)
        {
            if (view.Parent is Panel parent)
            {
                parent.Children.Remove(view);
            }
        }
        this.Children.Clear();

        SplitNode? root = snapshot.Root;
        if (root is null)
        {
            return; // empty tree — the host re-seeds (WorkspaceView, U6).
        }

        var present = new HashSet<PaneId>(root.Leaves().Select(l => l.Id));

        // Zoom (U7): show only the focused/zoomed leaf full-bleed; other panes stay cached (their
        // engines keep running) but are absent from the visual tree.
        if (snapshot.ZoomedPane is PaneId zoomed && present.Contains(zoomed))
        {
            this.Children.Add(GetOrCreatePane(zoomed));
        }
        else
        {
            this.Children.Add(Materialize(root));
        }

        PrunePanes(present);
    }

    private FrameworkElement Materialize(SplitNode node) => node switch
    {
        PaneLeaf leaf => GetOrCreatePane(leaf.Id),
        SplitBranch branch => MaterializeBranch(branch),
        _ => throw new InvalidOperationException($"Unknown split node: {node.GetType().Name}"),
    };

    private FrameworkElement GetOrCreatePane(PaneId id)
    {
        if (!_paneViews.TryGetValue(id, out FrameworkElement? view))
        {
            view = _paneFactory(id);
            _paneViews[id] = view;
        }
        return view;
    }

    private Grid MaterializeBranch(SplitBranch branch)
    {
        var grid = new Grid();
        FrameworkElement first = Materialize(branch.First);
        FrameworkElement second = Materialize(branch.Second);
        double fraction = LayoutGeometry.Clamp01(branch.DividerPosition);

        var splitter = new GridSplitter
        {
            Background = new SolidColorBrush(Color.FromArgb(0xFF, 0x2B, 0x2B, 0x2B)),
            HorizontalAlignment = HorizontalAlignment.Stretch,
            VerticalAlignment = VerticalAlignment.Stretch,
            ResizeBehavior = GridSplitter.GridResizeBehavior.PreviousAndNext,
        };

        if (branch.Orientation == Cmux.Core.Orientation.Vertical)
        {
            // Side-by-side: [fraction*] [splitter] [(1-fraction)*]
            var c0 = new ColumnDefinition { Width = new GridLength(fraction, GridUnitType.Star) };
            var cs = new ColumnDefinition { Width = new GridLength(DividerThickness) };
            var c1 = new ColumnDefinition { Width = new GridLength(1 - fraction, GridUnitType.Star) };
            grid.ColumnDefinitions.Add(c0);
            grid.ColumnDefinitions.Add(cs);
            grid.ColumnDefinitions.Add(c1);

            splitter.ResizeDirection = GridSplitter.GridResizeDirection.Columns;
            splitter.Width = DividerThickness;
            Grid.SetColumn(first, 0);
            Grid.SetColumn(splitter, 1);
            Grid.SetColumn(second, 2);
            WireDividerCommit(splitter, branch.Id, () => LayoutGeometry.FractionFromStars(c0.ActualWidth, c1.ActualWidth));
        }
        else
        {
            // Stacked: [fraction*] / [splitter] / [(1-fraction)*]
            var r0 = new RowDefinition { Height = new GridLength(fraction, GridUnitType.Star) };
            var rs = new RowDefinition { Height = new GridLength(DividerThickness) };
            var r1 = new RowDefinition { Height = new GridLength(1 - fraction, GridUnitType.Star) };
            grid.RowDefinitions.Add(r0);
            grid.RowDefinitions.Add(rs);
            grid.RowDefinitions.Add(r1);

            splitter.ResizeDirection = GridSplitter.GridResizeDirection.Rows;
            splitter.Height = DividerThickness;
            Grid.SetRow(first, 0);
            Grid.SetRow(splitter, 1);
            Grid.SetRow(second, 2);
            WireDividerCommit(splitter, branch.Id, () => LayoutGeometry.FractionFromStars(r0.ActualHeight, r1.ActualHeight));
        }

        grid.Children.Add(first);
        grid.Children.Add(splitter);
        grid.Children.Add(second);
        return grid;
    }

    /// <summary>
    /// Persist a divider's position back to the controller once the user finishes a drag (pointer
    /// release) or keyboard resize. We read the rendered track sizes (robust whether the splitter
    /// keeps star or pixel units) and convert to a [0,1] fraction; <c>handledEventsToo</c> is
    /// required because the splitter marks the input as handled.
    /// </summary>
    private void WireDividerCommit(GridSplitter splitter, BranchId branch, Func<double> readFraction)
    {
        void Commit() => _onDividerChanged(branch, readFraction());
        splitter.AddHandler(
            UIElement.PointerReleasedEvent,
            new PointerEventHandler((_, _) => Commit()),
            handledEventsToo: true);
        splitter.AddHandler(
            UIElement.KeyUpEvent,
            new KeyEventHandler((_, _) => Commit()),
            handledEventsToo: true);
    }

    private void PrunePanes(HashSet<PaneId> present)
    {
        List<PaneId> stale = _paneViews.Keys.Where(id => !present.Contains(id)).ToList();
        foreach (PaneId id in stale)
        {
            _paneViews.Remove(id);
        }
    }
}
