using System;
using System.Collections.Generic;
using System.Collections.Immutable;
using System.Linq;
using Optimus.Core;
using Optimus.Design;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;

namespace Optimus.Splits;

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
    // Focus indicator (R7): the focused pane is outlined in attention-teal (DESIGN.md RISK #2:
    // teal always means "active / live"); every other pane's border is transparent. The border
    // lives on the pane's root grid so it frames the whole pane (tab strip and terminal). The
    // engine has no focus concept and draws a solid cursor in every surface, so this outline is
    // the only optimus-level cue for which pane currently receives keystrokes. The thickness is
    // constant — only the brush toggles — so gaining/losing focus never reflows the pane.
    private const double FocusBorderThickness = 2.0;

    // Notification pane-flash (plan Phase 3 U7): briefly paint an attention ring when a
    // notification lands on the pane, then hide it. Per DESIGN.md the flash is the attention pulse
    // "layered briefly over" whatever border the pane already shows — so it lives on a dedicated
    // overlay ring just inside the focus border, not on the focus border itself. A focused pane's
    // border is already attention-teal; reusing that slot would make the flash invisible exactly
    // when the notification targets the pane you're looking at (codex review, PR #8). The overlay
    // doesn't participate in layout, so the flash still never reflows the pane.
    private static readonly TimeSpan FlashDuration = TimeSpan.FromMilliseconds(700);

    private readonly PaneId _paneId;
    private readonly SplitTreeController _controller;
    private readonly SurfaceManager _surfaces;
    private readonly Func<SurfaceId, bool> _isUnread;
    // Opening a web pane needs the surface plane (a typed factory), which only WorkspaceView owns;
    // PaneView routes the strip's globe button up through this callback (p6 U4).
    private readonly Action<PaneId> _openWebPane;

    private DispatcherTimer? _flashTimer;

    private readonly PaneTabStrip _strip = new();
    private readonly Grid _contentHost = new() { Background = Tokens.Surface0 };
    private readonly Grid _root = new();

    // The flash overlay: a hit-test-invisible ring spanning both rows, drawn just inside the focus
    // border. Transparent except during a flash, when it paints attention-teal — visibly thickening
    // the frame of a focused pane and outlining an unfocused one.
    private readonly Border _flashOverlay = new()
    {
        BorderThickness = new Thickness(FocusBorderThickness),
        BorderBrush = Tokens.Transparent,
        IsHitTestVisible = false,
    };

    // Surfaces currently parented in this pane's content host, plus the title handler we attached to
    // each (so we can detach on removal). Keyed by surface id.
    private readonly Dictionary<SurfaceId, ISurface> _hosted = new();
    private readonly Dictionary<SurfaceId, Action<string>> _titleHandlers = new();
    private readonly Dictionary<SurfaceId, string> _titles = new();

    /// <summary>The model pane this view renders.</summary>
    public PaneId PaneId => _paneId;

    public PaneView(
        PaneId paneId,
        SplitTreeController controller,
        SurfaceManager surfaces,
        Func<SurfaceId, bool> isUnread,
        Action<PaneId> openWebPane)
    {
        _paneId = paneId;
        _controller = controller;
        _surfaces = surfaces;
        _isUnread = isUnread;
        _openWebPane = openWebPane;

        _strip.TabSelected += id => _controller.SelectTab(_paneId, id);
        _strip.TabClosed += id => _controller.CloseTab(id);
        _strip.NewTabRequested += () => _controller.NewTab(_paneId);
        // Web pane (p6 U4): routed up to WorkspaceView, which sets the pending surface kind and then
        // creates the tab — acting on this pane (the strip the button belongs to), like the splits.
        _strip.NewWebPaneRequested += () => _openWebPane(_paneId);

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
        _root.BorderBrush = Tokens.Transparent;
        Grid.SetRow(_strip, 0);
        Grid.SetRow(_contentHost, 1);
        Grid.SetRow(_flashOverlay, 0);
        Grid.SetRowSpan(_flashOverlay, 2);
        _root.Children.Add(_strip);
        _root.Children.Add(_contentHost);
        _root.Children.Add(_flashOverlay); // last child → composites above strip and content
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
        _root.BorderBrush = snapshot.FocusedPane == _paneId ? Tokens.Attention : Tokens.Transparent;
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
            tabs, sel,
            id => _titles.TryGetValue(id, out string? title) ? title : null,
            _isUnread));
    }

    /// <summary>
    /// Re-project this pane's tab headers from current controller state (plan Phase 3 U7). The host
    /// calls this when unread state changes without a snapshot change — a notification arriving or
    /// being cleared on focus — so the unread dot appears/disappears immediately.
    /// </summary>
    public void RefreshHeaders() => RenderStrip(_controller.Tabs(_paneId), _controller.SelectedTab(_paneId));

    /// <summary>
    /// Briefly show the attention overlay ring to signal a notification landed on this pane (plan
    /// Phase 3 U7). The ring layers inside the focus border so it stays visible even when the pane
    /// is focused (whose border is already attention-teal); it hides after <see cref="FlashDuration"/>.
    /// </summary>
    public void Flash()
    {
        _flashOverlay.BorderBrush = Tokens.Attention;
        _flashTimer ??= CreateFlashTimer();
        _flashTimer.Stop();
        _flashTimer.Start();
    }

    /// <summary>Cancel any in-progress flash and hide the overlay ring immediately (AE6).</summary>
    public void ClearFlash()
    {
        _flashTimer?.Stop();
        _flashOverlay.BorderBrush = Tokens.Transparent;
    }

    private DispatcherTimer CreateFlashTimer()
    {
        var timer = new DispatcherTimer { Interval = FlashDuration };
        timer.Tick += (_, _) =>
        {
            timer.Stop();
            _flashOverlay.BorderBrush = Tokens.Transparent;
        };
        return timer;
    }
}
