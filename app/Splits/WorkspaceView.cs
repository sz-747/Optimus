using System;
using System.Collections.Generic;
using System.Linq;
using Cmux.Core;
using Microsoft.UI.Xaml.Controls;

namespace Cmux.Splits;

/// <summary>
/// The single hosted workspace (plan Phase 2 U6): it owns the model plane (a
/// <see cref="SplitTreeController"/>), the surface plane (a <see cref="SurfaceManager"/> over real
/// <c>TerminalPane</c>s), the <see cref="SplitTreeView"/> that renders them, and the
/// <see cref="ShortcutRouter"/> that drives them by keyboard. It seeds one pane + one surface on
/// construction (preserving Phase-1 launch behavior) and is the sole place the two planes are wired
/// together.
///
/// <para><b>Event flow.</b> Controller operations raise <see cref="SplitTreeController.SurfaceCreated"/>/
/// <see cref="SplitTreeController.SurfaceClosed"/> (→ create/dispose the matching engine) and then a
/// <see cref="SplitTreeController.SnapshotChanged"/> (→ re-render the tree, reconcile each pane, and
/// move OS focus to the derived focused surface — R7). Closing the last surface raises
/// <see cref="SplitTreeController.Emptied"/>, which re-seeds so the window never goes contentless
/// (R6). On window close, <see cref="ShutdownAll"/> tears every engine down in order (R9).</para>
/// </summary>
public sealed class WorkspaceView : UserControl
{
    private readonly SplitTreeController _controller = new();
    private readonly SurfaceManager _surfaces = new(new TerminalPaneSurfaceFactory());
    private readonly SplitTreeView _tree;
    private readonly ShortcutRouter _shortcuts;

    private readonly Dictionary<PaneId, PaneView> _panes = new();
    private readonly Dictionary<SurfaceId, string> _titles = new();

    private SurfaceId? _lastFocusedSurface;

    /// <summary>Raised when the focused surface's title (or the focus itself) changes — drives the window chrome.</summary>
    public event Action<string>? ActiveTitleChanged;

    public WorkspaceView()
    {
        _tree = new SplitTreeView(
            paneFactory: GetOrCreatePane,
            onDividerChanged: (branch, fraction) => _controller.SetDividerPosition(branch, fraction));

        _controller.SurfaceCreated += OnSurfaceCreated;
        _controller.SurfaceClosed += OnSurfaceClosed;
        _controller.Emptied += OnEmptied;
        _controller.SnapshotChanged += OnSnapshotChanged;

        _shortcuts = new ShortcutRouter(_controller, _surfaces);
        _shortcuts.Attach(this);

        Content = _tree;

        // The controller's constructor seeds the first pane + surface silently (no subscribers yet),
        // so create that engine here and drive the first render manually.
        foreach (SurfaceId id in _controller.AllSurfaces)
        {
            CreateSurfaceEngine(id);
        }
        OnSnapshotChanged(_controller.Snapshot());
    }

    /// <summary>Tear down every surface engine in order (R9). Called from the window's Closed handler.</summary>
    public void ShutdownAll() => _surfaces.DisposeAll();

    // ---- Surface plane -----------------------------------------------------------------------

    private void OnSurfaceCreated(SurfaceId id) => CreateSurfaceEngine(id);

    private void CreateSurfaceEngine(SurfaceId id)
    {
        ISurface surface = _surfaces.CreateSurface(id); // default shell, inherited cwd (Phase-1 parity)
        surface.TitleChanged += title => OnSurfaceTitleChanged(id, title);
    }

    private void OnSurfaceClosed(SurfaceId id)
    {
        _surfaces.DisposeSurface(id);
        _titles.Remove(id);
    }

    private void OnSurfaceTitleChanged(SurfaceId id, string title)
    {
        _titles[id] = title;
        if (_controller.FocusedSurface == id)
        {
            RaiseActiveTitle();
        }
    }

    // ---- Model → view ------------------------------------------------------------------------

    private void OnEmptied() => _controller.SeedRoot(); // re-seeds and emits SurfaceCreated + a snapshot.

    private void OnSnapshotChanged(TreeSnapshot snapshot)
    {
        _tree.Render(snapshot);

        if (snapshot.Root is SplitNode root)
        {
            var present = new HashSet<PaneId>();
            foreach (PaneLeaf leaf in root.Leaves())
            {
                present.Add(leaf.Id);
                GetOrCreatePane(leaf.Id).Sync(snapshot);
            }
            foreach (PaneId id in _panes.Keys.Where(k => !present.Contains(k)).ToList())
            {
                _panes.Remove(id);
            }
        }

        // OS focus follows the derived focused surface, but only when it actually changes — so a
        // divider drag, equalize, or zoom never steals focus from the terminal (R7).
        if (_controller.FocusedSurface != _lastFocusedSurface)
        {
            _lastFocusedSurface = _controller.FocusedSurface;
            if (_lastFocusedSurface is SurfaceId focused)
            {
                _surfaces.Get(focused)?.FocusSurface();
            }
        }

        RaiseActiveTitle();
    }

    private PaneView GetOrCreatePane(PaneId id)
    {
        if (!_panes.TryGetValue(id, out PaneView? pane))
        {
            pane = new PaneView(id, _controller, _surfaces);
            _panes[id] = pane;
        }
        return pane;
    }

    private void RaiseActiveTitle()
    {
        string title = _controller.FocusedSurface is SurfaceId id
            && _titles.TryGetValue(id, out string? t)
            && !string.IsNullOrEmpty(t)
                ? t
                : "cmux";
        ActiveTitleChanged?.Invoke(title);
    }
}
