using System;
using System.Collections.Generic;
using System.Linq;

namespace Optimus.Core;

/// <summary>
/// The single owner of live surfaces (KTD9): maps each <see cref="SurfaceId"/> to its
/// <see cref="ISurface"/> and is the only thing that creates or disposes one. Wiring the
/// controller's <see cref="SplitTreeController.SurfaceCreated"/> / <see cref="SplitTreeController.SurfaceClosed"/>
/// events into this class is what decouples engine lifetime from XAML <c>Loaded</c>/<c>Unloaded</c>,
/// so the tree can be restructured without destroying surviving shells (R10). Holds no WinUI types,
/// so it is unit-testable with a fake factory.
/// </summary>
public sealed class SurfaceManager : IDisposable
{
    private readonly ISurfaceFactory _factory;
    private readonly Dictionary<SurfaceId, ISurface> _surfaces = new();

    public SurfaceManager(ISurfaceFactory factory)
    {
        _factory = factory ?? throw new ArgumentNullException(nameof(factory));
    }

    /// <summary>The live surfaces, keyed by id.</summary>
    public IReadOnlyDictionary<SurfaceId, ISurface> Surfaces => _surfaces;

    /// <summary>Number of live surfaces.</summary>
    public int Count => _surfaces.Count;

    /// <summary>
    /// Create (or return the existing) surface for <paramref name="id"/>. Idempotent per id (R1):
    /// a duplicate request for a live id returns the same instance rather than building a second.
    /// </summary>
    public ISurface CreateSurface(SurfaceId id, string? cwd = null, string? cmdline = null)
    {
        if (_surfaces.TryGetValue(id, out ISurface? existing))
        {
            return existing;
        }
        ISurface surface = _factory.Create(id, cwd, cmdline);
        _surfaces[id] = surface;
        return surface;
    }

    /// <summary>The surface for <paramref name="id"/>, or <c>null</c> if it is not live.</summary>
    public ISurface? Get(SurfaceId id) => _surfaces.TryGetValue(id, out ISurface? s) ? s : null;

    /// <summary>Tear down only the surface for <paramref name="id"/>, if live (R2). No-op otherwise.</summary>
    public void DisposeSurface(SurfaceId id)
    {
        if (_surfaces.Remove(id, out ISurface? surface))
        {
            surface.Shutdown();
        }
    }

    /// <summary>Tear down every live surface exactly once and clear the registry (R9).</summary>
    public void DisposeAll()
    {
        // Snapshot first: Shutdown() must not be able to mutate the dictionary mid-iteration.
        List<ISurface> all = _surfaces.Values.ToList();
        _surfaces.Clear();
        foreach (ISurface surface in all)
        {
            surface.Shutdown();
        }
    }

    public void Dispose() => DisposeAll();
}
