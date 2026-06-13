using System;
using System.Collections.Generic;
using Optimus.Core;
using Xunit;

namespace Optimus.Core.Tests;

/// <summary>
/// Coverage for <see cref="SurfaceManager"/> and <see cref="SurfaceLifecycleGuard"/> (plan Phase 2
/// U2): per-id create idempotency (R1), targeted vs. full teardown counted exactly once (R2/R9),
/// and the attach-once / shutdown-once guard that lets re-parenting leave a live shell untouched
/// (R10/KTD9). Uses a fake factory so no real engine/WinUI host is required.
/// </summary>
public sealed class SurfaceManagerTests
{
    private sealed class FakeSurface : ISurface
    {
        private readonly List<string> _log;

        public FakeSurface(SurfaceId id, List<string> log)
        {
            Id = id;
            _log = log;
        }

        public SurfaceId Id { get; }
        public int ShutdownCount { get; private set; }

        public event Action<string>? TitleChanged;
        public event Action<SurfaceNotification>? NotificationRaised;

        public void RaiseTitle(string title) => TitleChanged?.Invoke(title);
        public void RaiseNotification(SurfaceNotification n) => NotificationRaised?.Invoke(n);

        public void SetActive(bool active) => _log.Add($"active:{Id}:{active}");
        public void FocusSurface() => _log.Add($"focus:{Id}");

        public void Shutdown()
        {
            ShutdownCount++;
            _log.Add($"shutdown:{Id}");
        }
    }

    private sealed class FakeFactory : ISurfaceFactory
    {
        private readonly List<string> _log;
        private readonly string _label;

        public FakeFactory(List<string> log, string label = "create") => (_log, _label) = (log, label);

        public List<FakeSurface> Created { get; } = new();

        public ISurface Create(SurfaceId id, string? cwd, string? cmdline)
        {
            _log.Add($"{_label}:{id}");
            var s = new FakeSurface(id, _log);
            Created.Add(s);
            return s;
        }
    }

    // ---- SurfaceManager ----------------------------------------------------------------------

    [Fact] // Covers R1.
    public void CreateSurface_is_idempotent_per_id_and_independent_across_ids()
    {
        var log = new List<string>();
        var factory = new FakeFactory(log);
        var manager = new SurfaceManager(factory);

        ISurface first = manager.CreateSurface(new SurfaceId(1));
        ISurface firstAgain = manager.CreateSurface(new SurfaceId(1)); // duplicate request
        ISurface second = manager.CreateSurface(new SurfaceId(2));

        Assert.Same(first, firstAgain);     // no second instance for the same id
        Assert.NotSame(first, second);
        Assert.Equal(2, factory.Created.Count);
        Assert.Equal(2, manager.Count);
        Assert.Same(first, manager.Get(new SurfaceId(1))); // creating id 2 left id 1 intact
    }

    [Fact] // Covers R2/R9.
    public void DisposeSurface_tears_down_only_the_target()
    {
        var log = new List<string>();
        var factory = new FakeFactory(log);
        var manager = new SurfaceManager(factory);
        manager.CreateSurface(new SurfaceId(1));
        manager.CreateSurface(new SurfaceId(2));

        manager.DisposeSurface(new SurfaceId(1));

        Assert.Equal(1, factory.Created[0].ShutdownCount); // id 1 shut down once
        Assert.Equal(0, factory.Created[1].ShutdownCount); // id 2 untouched
        Assert.Null(manager.Get(new SurfaceId(1)));
        Assert.NotNull(manager.Get(new SurfaceId(2)));
        Assert.Equal(1, manager.Count);
    }

    [Fact]
    public void DisposeSurface_unknown_id_is_a_noop()
    {
        var manager = new SurfaceManager(new FakeFactory(new List<string>()));
        manager.CreateSurface(new SurfaceId(1));

        manager.DisposeSurface(new SurfaceId(99)); // must not throw

        Assert.Equal(1, manager.Count);
    }

    [Fact] // Covers R9.
    public void DisposeAll_tears_down_every_surface_exactly_once()
    {
        var log = new List<string>();
        var factory = new FakeFactory(log);
        var manager = new SurfaceManager(factory);
        manager.CreateSurface(new SurfaceId(1));
        manager.CreateSurface(new SurfaceId(2));
        manager.CreateSurface(new SurfaceId(3));

        manager.DisposeAll();

        Assert.All(factory.Created, s => Assert.Equal(1, s.ShutdownCount));
        Assert.Equal(0, manager.Count);
    }

    // ---- Per-kind factory routing (p6 U4) ----------------------------------------------------

    [Fact] // A registered kind is realised by its own factory; the default kind by the ctor factory.
    public void TryCreateSurface_routes_each_kind_to_its_registered_factory()
    {
        var log = new List<string>();
        var terminalFactory = new FakeFactory(log, "terminal");
        var webFactory = new FakeFactory(log, "web");
        var manager = new SurfaceManager(terminalFactory); // ctor factory == default (Terminal)
        manager.RegisterFactory(SurfaceKind.Web, webFactory);

        manager.TryCreateSurface(new SurfaceId(1), SurfaceKind.Terminal);
        manager.TryCreateSurface(new SurfaceId(2), SurfaceKind.Web);

        Assert.Single(terminalFactory.Created); // id 1 came from the terminal factory
        Assert.Single(webFactory.Created);      // id 2 came from the web factory
        Assert.Equal(new SurfaceId(1), terminalFactory.Created[0].Id);
        Assert.Equal(new SurfaceId(2), webFactory.Created[0].Id);
    }

    [Fact] // The default overload (and CreateSurface) still produce terminals — no behavior drift.
    public void TryCreateSurface_default_overload_uses_the_terminal_factory()
    {
        var log = new List<string>();
        var terminalFactory = new FakeFactory(log, "terminal");
        var webFactory = new FakeFactory(log, "web");
        var manager = new SurfaceManager(terminalFactory);
        manager.RegisterFactory(SurfaceKind.Web, webFactory);

        manager.TryCreateSurface(new SurfaceId(1)); // kind-less overload

        Assert.Single(terminalFactory.Created);
        Assert.Empty(webFactory.Created);
    }

    [Fact] // Idempotency ignores kind: a live id returns its first instance and consumes no second slot.
    public void TryCreateSurface_is_idempotent_per_id_regardless_of_kind()
    {
        var log = new List<string>();
        var terminalFactory = new FakeFactory(log, "terminal");
        var webFactory = new FakeFactory(log, "web");
        var manager = new SurfaceManager(terminalFactory);
        manager.RegisterFactory(SurfaceKind.Web, webFactory);

        ISurface? first = manager.TryCreateSurface(new SurfaceId(1), SurfaceKind.Terminal);
        ISurface? again = manager.TryCreateSurface(new SurfaceId(1), SurfaceKind.Web); // same id, other kind

        Assert.Same(first, again);              // first kind wins; no rebuild
        Assert.Single(terminalFactory.Created);
        Assert.Empty(webFactory.Created);       // the web factory was never invoked
        Assert.Equal(1, manager.Count);
    }

    [Fact] // An unregistered kind is a programmer error — fail loudly, do not silently fall back.
    public void TryCreateSurface_throws_for_an_unregistered_kind()
    {
        var manager = new SurfaceManager(new FakeFactory(new List<string>()));

        Assert.Throws<InvalidOperationException>(
            () => manager.TryCreateSurface(new SurfaceId(1), SurfaceKind.Web)); // Web never registered
    }

    [Fact] // RegisterFactory rejects null so a misconfigured host fails at wiring, not at spawn time.
    public void RegisterFactory_rejects_a_null_factory()
    {
        var manager = new SurfaceManager(new FakeFactory(new List<string>()));

        Assert.Throws<ArgumentNullException>(() => manager.RegisterFactory(SurfaceKind.Web, null!));
    }

    // ---- SurfaceLifecycleGuard (R10 / KTD9 — re-parent correctness) --------------------------

    [Fact] // Covers R10.
    public void Guard_attaches_once_and_ignores_reparent_reload()
    {
        var guard = new SurfaceLifecycleGuard();

        Assert.True(guard.TryAttach());  // first Loaded → attach
        Assert.True(guard.IsAttached);
        Assert.False(guard.TryAttach()); // re-parent's second Loaded → no re-attach
    }

    [Fact] // Covers R2/R9.
    public void Guard_shutdown_is_idempotent_and_blocks_later_attach()
    {
        var guard = new SurfaceLifecycleGuard();
        guard.TryAttach();

        Assert.True(guard.TryShutdown());  // explicit teardown
        Assert.True(guard.IsDisposed);
        Assert.False(guard.TryShutdown()); // a second close is a no-op
        Assert.False(guard.TryAttach());   // and we never re-attach after shutdown
    }

    [Fact] // Covers R10 — the canonical re-parent sequence.
    public void Reparent_sequence_attaches_once_and_never_disposes()
    {
        // Simulates Loaded → Unloaded (which, post-U2, no longer shuts down) → Loaded.
        var guard = new SurfaceLifecycleGuard();
        int attachCount = 0;

        if (guard.TryAttach()) { attachCount++; } // Loaded #1
        // Unloaded fires here but the control no longer calls Shutdown.
        if (guard.TryAttach()) { attachCount++; } // Loaded #2 from re-parent

        Assert.Equal(1, attachCount);   // attached exactly once
        Assert.False(guard.IsDisposed); // engine never disposed by the re-parent
    }
}
