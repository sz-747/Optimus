using System;
using System.Collections.Generic;
using System.Collections.Immutable;
using System.Linq;
using Cmux.Core;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Windows.UI;

namespace Cmux.Splits;

/// <summary>
/// The leaf UI for one pane (plan Phase 2 U4): a <see cref="PaneTabStrip"/> above a content host
/// that stacks the pane's surfaces, with only the selected one composited (R3/R11, KTD2). It is the
/// element the <see cref="SplitTreeView"/> caches and re-parents, so its whole subtree — strip,
/// host, and the live <c>SwapChainPanel</c>s within — moves as a unit during restructuring; no
/// surface is ever re-parented by a split (the lifecycle guard still covers the Loaded/Unloaded
/// churn — KTD9/R10).
///
/// <para>PaneView is the bridge between the two ownership planes: the strip raises intent, which it
/// routes to the <see cref="SplitTreeController"/> (model plane); <see cref="Sync"/> then reconciles
/// the hosted surfaces against the resulting snapshot via the <see cref="SurfaceManager"/> (surface
/// plane). It holds no focus state — focus is derived and applied by the host (U5/U6).</para>
/// </summary>
internal sealed class PaneView : UserControl
{
    private static readonly SolidColorBrush ContentBackground = new(Color.FromArgb(0xFF, 0x0C, 0x0C, 0x0C));

    private readonly PaneId _paneId;
    private readonly SplitTreeController _controller;
    private readonly SurfaceManager _surfaces;

    private readonly PaneTabStrip _strip = new();
    private readonly Grid _contentHost = new() { Background = ContentBackground };

    // Surfaces currently parented in this pane's content host, plus the title handler we attached to
    // each (so we can detach on removal). Keyed by surface id.
    private readonly Dictionary<SurfaceId, ISurface> _hosted = new();
    private readonly Dictionary<SurfaceId, Action<string>> _titleHandlers = new();
    private readonly Dictionary<SurfaceId, string> _titles = new();

    /// <summary>The model pane this view renders.</summary>
    public PaneId PaneId => _paneId;

    public PaneView(PaneId paneId, SplitTreeController controller, SurfaceManager surfaces)
    {
        _paneId = paneId;
        _controller = controller;
        _surfaces = surfaces;

        _strip.TabSelected += id => _controller.SelectTab(_paneId, id);
        _strip.TabClosed += id => _controller.CloseTab(id);
        _strip.NewTabRequested += () => _controller.NewTab(_paneId);

        // Pointer/programmatic focus landing anywhere in this pane's subtree (a terminal click)
        // makes it the model's focused pane, so subsequent keyboard ops target it (R7/R8). Guarded
        // so re-focusing the already-focused pane raises no snapshot churn and cannot loop with the
        // host's focus-follows-snapshot step.
        this.GotFocus += OnPaneGotFocus;

        var root = new Grid();
        root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        root.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        Grid.SetRow(_strip, 0);
        Grid.SetRow(_contentHost, 1);
        root.Children.Add(_strip);
        root.Children.Add(_contentHost);
        Content = root;
    }

    /// <summary>
    /// Reconcile the content host and tab strip with <paramref name="snapshot"/> for this pane: host
    /// any newly-spawned surface, drop any closed one, composite only the selected surface, and
    /// re-project the tab headers. No-op if this pane no longer exists (the host drops the view).
    /// </summary>
    public void Sync(TreeSnapshot snapshot)
    {
        PaneLeaf? leaf = snapshot.Root.FindPane(_paneId);
        if (leaf is null)
        {
            return;
        }

        var desired = new HashSet<SurfaceId>(leaf.Tabs);

        // Detach surfaces that left this pane (closed). Their engine was already disposed via
        // SurfaceClosed before this snapshot, so we only unwire and remove the dead element.
        foreach (SurfaceId id in _hosted.Keys.ToList())
        {
            if (!desired.Contains(id))
            {
                RemoveHosted(id);
            }
        }

        // Host every current tab; composite only the selected one.
        foreach (SurfaceId id in leaf.Tabs)
        {
            EnsureHosted(id);
            if (_hosted.TryGetValue(id, out ISurface? surface))
            {
                surface.SetActive(id == leaf.Selected);
            }
        }

        RenderStrip(leaf.Tabs, leaf.Selected);
    }

    private void OnPaneGotFocus(object sender, RoutedEventArgs e)
    {
        if (_controller.FocusedPane != _paneId)
        {
            _controller.FocusPane(_paneId);
        }
    }

    private void EnsureHosted(SurfaceId id)
    {
        if (_hosted.ContainsKey(id))
        {
            return;
        }

        ISurface? surface = _surfaces.Get(id);
        if (surface is not FrameworkElement element)
        {
            return; // host creates surfaces on SurfaceCreated; if absent, a later Sync will catch it.
        }

        _hosted[id] = surface;
        _contentHost.Children.Add(element);

        void OnTitle(string title)
        {
            _titles[id] = title;
            RenderStrip(_controller.Tabs(_paneId), _controller.SelectedTab(_paneId));
        }

        _titleHandlers[id] = OnTitle;
        surface.TitleChanged += OnTitle;
    }

    private void RemoveHosted(SurfaceId id)
    {
        if (_hosted.TryGetValue(id, out ISurface? surface))
        {
            if (_titleHandlers.TryGetValue(id, out Action<string>? handler))
            {
                surface.TitleChanged -= handler;
            }
            if (surface is FrameworkElement element)
            {
                _contentHost.Children.Remove(element);
            }
        }
        _hosted.Remove(id);
        _titleHandlers.Remove(id);
        _titles.Remove(id);
    }

    private void RenderStrip(IReadOnlyList<SurfaceId> tabs, SurfaceId? selected)
    {
        if (selected is not SurfaceId sel)
        {
            _strip.Render(ImmutableArray<TabHeaderDto>.Empty);
            return;
        }
        _strip.Render(TabHeaderProjection.Project(
            tabs, sel, id => _titles.TryGetValue(id, out string? title) ? title : null));
    }
}
