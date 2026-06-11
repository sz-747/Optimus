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
    private readonly CapacityModel? _capacity;
    private readonly Dictionary<SurfaceId, ISurface> _surfaces = new();

    /// <summary>
    /// <paramref name="capacity"/> is the RAM safe-zone admission governor (plan U5); null leaves
    /// the manager ungoverned (tests, degraded startup — see <c>App.StartCapacityGovernor</c>).
    /// </summary>
    public SurfaceManager(ISurfaceFactory factory, CapacityModel? capacity = null)
    {
        _factory = factory ?? throw new ArgumentNullException(nameof(factory));
        _capacity = capacity;
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
        => TryCreateSurface(id, cwd, cmdline)
           ?? throw new InvalidOperationException(
               $"Safe-zone cap reached; surface {id} refused. Governed callers must use TryCreateSurface.");

    /// <summary>
    /// Capacity-gated create (RAM safe-zone plan U5): the single spawn choke point. Returns the
    /// existing surface for a live id (idempotent, never consumes a second slot), <c>null</c> when
    /// the <see cref="CapacityModel"/> refuses a slot (graceful refusal — no factory call, no
    /// throw), or the freshly built surface otherwise. A factory exception releases the
    /// reservation before rethrowing. With no model attached, never refuses.
    /// </summary>
    public ISurface? TryCreateSurface(SurfaceId id, string? cwd = null, string? cmdline = null)
    {
        if (_surfaces.TryGetValue(id, out ISurface? existing))
        {
            return existing; // idempotent per id (R1) — no capacity consumed
        }

        ReservationToken? token = null;
        if (_capacity is not null)
        {
            token = _capacity.TryReserve(id);
            if (token is null)
            {
                return null; // at cap — refuse without invoking the factory
            }
        }

        ISurface surface;
        try
        {
            surface = _factory.Create(id, cwd, cmdline);
        }
        catch
        {
            _capacity?.Release(id); // failed spawn must not strand a reserved slot
            throw;
        }

        _surfaces[id] = surface;
        // The engine spawns its child lazily (TerminalPane.Configure), so no PID exists at this
        // layer yet — commit with pid 0 to settle the slot accounting; the pane reports the real
        // PID's memory via RecordMeasurement during calibration (U4).
        // ACCEPTED AS DESIGNED: capacity is committed at *surface creation* — the surface is the
        // capacity unit, and its slot is freed only on surface removal (DisposeSurface/Dispose →
        // Release). A pane that never configures (never spawns a shell) still holds its slot;
        // that is intentional: the slot accounts for the surface's eventual cost.
        if (token is not null)
        {
            _capacity!.Commit(token, pid: 0);
        }
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
            _capacity?.Release(id); // free the safe-zone slot (U5)
        }
    }

    /// <summary>Tear down every live surface exactly once and clear the registry (R9).</summary>
    public void DisposeAll()
    {
        // Snapshot first: Shutdown() must not be able to mutate the dictionary mid-iteration.
        List<ISurface> all = _surfaces.Values.ToList();
        List<SurfaceId> ids = _surfaces.Keys.ToList();
        _surfaces.Clear();
        foreach (ISurface surface in all)
        {
            surface.Shutdown();
        }
        if (_capacity is not null)
        {
            foreach (SurfaceId id in ids)
            {
                _capacity.Release(id); // free every safe-zone slot (U5)
            }
        }
    }

    public void Dispose() => DisposeAll();
}
