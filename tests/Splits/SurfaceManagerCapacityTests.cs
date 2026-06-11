using System;
using System.Collections.Generic;
using Optimus.Core;
using Xunit;

namespace Optimus.Core.Tests;

/// <summary>
/// Coverage for the safe-zone admission gate in <see cref="SurfaceManager"/> (RAM safe-zone plan
/// U5): reserve-then-commit around the single spawn choke point. At cap, creation is refused
/// gracefully (typed null result, factory never invoked); idempotent re-create never double-
/// reserves; disposal releases the slot; a factory exception releases the reservation; and a
/// null <see cref="CapacityModel"/> leaves the manager ungoverned (existing behavior).
/// </summary>
public sealed class SurfaceManagerCapacityTests
{
    private const ulong Gb = 1024UL * 1024 * 1024;

    private sealed class FakeCapacityProvider : ICapacityProvider
    {
        public ulong TotalPhysBytes { get; set; } = 16 * Gb;
        public ulong AvailablePhysBytes { get; set; }
        public ulong CommitHeadroomBytes { get; set; }
        public bool IsLowMemorySignaled { get; set; }

        public event Action? LowMemorySignal;

        public void RaiseLowMemory()
        {
            IsLowMemorySignaled = true;
            LowMemorySignal?.Invoke();
        }

        public ulong? MeasureProcessPrivateBytes(int pid) => null;
    }

    private sealed class FakeSurface : ISurface
    {
        public FakeSurface(SurfaceId id) => Id = id;

        public SurfaceId Id { get; }

        public event Action<string>? TitleChanged;
        public event Action<SurfaceNotification>? NotificationRaised;

        public void SetActive(bool active) { }
        public void FocusSurface() { }
        public void Shutdown() { }

        public void RaiseTitle(string title) => TitleChanged?.Invoke(title);
        public void RaiseNotification(SurfaceNotification n) => NotificationRaised?.Invoke(n);
    }

    private sealed class FakeFactory : ISurfaceFactory
    {
        public int CreateCalls { get; private set; }
        public Func<SurfaceId, ISurface>? OnCreate { get; set; }

        public ISurface Create(SurfaceId id, string? cwd, string? cmdline)
        {
            CreateCalls++;
            return OnCreate is not null ? OnCreate(id) : new FakeSurface(id);
        }
    }

    /// <summary>A model whose safe zone fits exactly <paramref name="maxTerminals"/> seed budgets.</summary>
    private static CapacityModel ModelWithCap(int maxTerminals)
    {
        ulong safeZone = (ulong)maxTerminals * CapacityModel.SeedBudgetBytes;
        var provider = new FakeCapacityProvider
        {
            AvailablePhysBytes = safeZone + CapacityModel.OsReserveBytes,
            CommitHeadroomBytes = safeZone + CapacityModel.OsReserveBytes,
        };
        return new CapacityModel(provider);
    }

    [Fact]
    public void Third_create_at_cap_is_refused_and_factory_is_not_invoked()
    {
        var factory = new FakeFactory();
        var capacity = ModelWithCap(2);
        var manager = new SurfaceManager(factory, capacity);

        ISurface? first = manager.TryCreateSurface(new SurfaceId(1));
        ISurface? second = manager.TryCreateSurface(new SurfaceId(2));
        ISurface? third = manager.TryCreateSurface(new SurfaceId(3));

        Assert.NotNull(first);
        Assert.NotNull(second);
        Assert.Null(third);                    // graceful typed refusal, no throw
        Assert.Equal(2, factory.CreateCalls);  // factory never invoked for the refused id
        Assert.Equal(2, manager.Count);
        Assert.Equal(CapacityLevel.Cap, capacity.State.Level);
    }

    [Fact]
    public void Recreating_an_existing_surface_at_cap_returns_it_without_consuming_a_slot()
    {
        var factory = new FakeFactory();
        var capacity = ModelWithCap(2);
        var manager = new SurfaceManager(factory, capacity);

        ISurface? first = manager.TryCreateSurface(new SurfaceId(1));
        manager.TryCreateSurface(new SurfaceId(2)); // now at cap

        ISurface? firstAgain = manager.TryCreateSurface(new SurfaceId(1)); // duplicate at cap

        Assert.Same(first, firstAgain);              // not refused, same instance
        Assert.Equal(2, factory.CreateCalls);        // no second build
        Assert.Equal(2, capacity.State.Used + capacity.State.Reserved); // slot count unchanged
    }

    [Fact]
    public void Disposing_a_surface_releases_its_slot_so_the_next_create_succeeds()
    {
        var factory = new FakeFactory();
        var capacity = ModelWithCap(2);
        var manager = new SurfaceManager(factory, capacity);
        manager.TryCreateSurface(new SurfaceId(1));
        manager.TryCreateSurface(new SurfaceId(2));
        Assert.Null(manager.TryCreateSurface(new SurfaceId(3))); // proves we were at cap

        manager.DisposeSurface(new SurfaceId(1));

        Assert.NotNull(manager.TryCreateSurface(new SurfaceId(3))); // freed slot is reusable
        Assert.Equal(2, manager.Count);
    }

    [Fact]
    public void DisposeAll_releases_every_slot()
    {
        var factory = new FakeFactory();
        var capacity = ModelWithCap(2);
        var manager = new SurfaceManager(factory, capacity);
        manager.TryCreateSurface(new SurfaceId(1));
        manager.TryCreateSurface(new SurfaceId(2));

        manager.DisposeAll();

        Assert.Equal(0, capacity.State.Used + capacity.State.Reserved);
        Assert.NotNull(manager.TryCreateSurface(new SurfaceId(3)));
        Assert.NotNull(manager.TryCreateSurface(new SurfaceId(4)));
    }

    [Fact]
    public void Factory_exception_releases_the_reservation_so_the_slot_is_reusable()
    {
        var factory = new FakeFactory
        {
            OnCreate = _ => throw new InvalidOperationException("spawn failed"),
        };
        var capacity = ModelWithCap(1);
        var manager = new SurfaceManager(factory, capacity);

        Assert.Throws<InvalidOperationException>(() => manager.TryCreateSurface(new SurfaceId(1)));

        Assert.Equal(0, capacity.State.Used + capacity.State.Reserved); // slot freed
        factory.OnCreate = null;
        Assert.NotNull(manager.TryCreateSurface(new SurfaceId(1))); // retry succeeds
    }

    [Fact]
    public void Null_capacity_model_leaves_the_manager_ungoverned()
    {
        var factory = new FakeFactory();
        var manager = new SurfaceManager(factory); // no model — existing single-arg ctor shape

        for (int i = 1; i <= 50; i++)
        {
            Assert.NotNull(manager.TryCreateSurface(new SurfaceId(i)));
        }
        Assert.Equal(50, manager.Count);
    }
}
