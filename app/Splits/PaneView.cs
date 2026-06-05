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

    // Focus indicator (R7): the focused pane is outlined in teal; every other pane's border is
    // transparent. The border lives on the pane's root grid so it frames the whole pane (tab strip
    // and terminal). The engine has no focus concept and draws a solid cursor in every surface, so
    // this outline is the only cmux-level cue for which pane currently receives keystrokes. The
    // thickness is constant — only the brush toggles — so gaining/losing focus never reflows the pane.
    private const double FocusBorderThickness = 2.0;
    private static readonly SolidColorBrush FocusedBorder = new(Color.FromArgb(0xFF, 0x2D, 0xD4, 0xBF));
    private static readonly SolidColorBrush UnfocusedBorder = new(Color.FromArgb(0x00, 0x00, 0x00, 0x00));

    private readonly PaneId _paneId;
    private readonly SplitTreeController _controller;
    private readonly SurfaceManager _surfaces;

    private readonly PaneTabStrip _strip = new();
    private readonly Grid _contentHost = new() { Background = ContentBackground };
    private readonly Grid _root = new();

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

        // Discoverable split/zoom buttons: focus this pane, then run the same action the keyboard
        // chord runs, so a button can never drift from its shortcut (R2) and always acts on the pane
        // whose strip was clicked (R4).
        _strip.SplitRightRequested += () => DispatchOnThisPane(ShortcutAction.SplitRight);
        _strip.SplitDownRequested += () => DispatchOnThisPane(ShortcutAction.SplitDown);
        _strip.ZoomToggleRequested += () => DispatchOnThisPane(ShortcutAction.ToggleZoom);

        // Pointer/programmatic focus landing anywhere in this pane's subtree (a terminal click)
        // makes it the model's focused pane, so subsequent keyboard ops target it (R7/R8). Guarded
        // so re-focusing the already-focused pane raises no snapshot churn and cannot loop with the
        // host's focus-follows-snapshot step.
        this.GotFocus += OnPaneGotFocus;

        _root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        _root.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        // Constant teal-border slot for the focus outline: starts transparent and Sync paints it on
        // the focused pane from the first snapshot (toggling colour, never thickness, avoids reflow).
        _root.BorderThickness = new Thickness(FocusBorderThickness);
        _root.BorderBrush = UnfocusedBorder;
        Grid.SetRow(_strip, 0);
        Grid.SetRow(_contentHost, 1);
        _root.Children.Add(_strip);
        _root.Children.Add(_contentHost);
        Content = _root;
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
        _strip.SetZoomActive(snapshot.ZoomedPane == _paneId);
        _root.BorderBrush = snapshot.FocusedPane == _paneId ? FocusedBorder : UnfocusedBorder;
    }

    /// <summary>
    /// Focus this pane (if it isn't already) and run <paramref name="action"/> through the same
    /// dispatch path the keyboard uses, so a strip button is a true alternative entry point to its
    /// chord (R2) and targets the clicked pane regardless of prior focus (R4).
    /// </summary>
    private void DispatchOnThisPane(ShortcutAction action)
    {
        if (_controller.FocusedPane != _paneId)
        {
            _controller.FocusPane(_paneId);
        }
        ShortcutMap.Apply(_controller, action);
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
