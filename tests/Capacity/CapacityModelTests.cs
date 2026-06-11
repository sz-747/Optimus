using System;
using System.Collections.Generic;
using Optimus.Core;
using Xunit;

namespace Optimus.Core.Tests;

/// <summary>
/// Coverage for <see cref="CapacityModel"/> (RAM safe-zone plan U2): safe-zone math, the
/// reserve-then-commit ledger (TOCTOU guard for parallel spawns), empirical budget calibration
/// with the hardware fingerprint, monotone cap-tightening under pressure with two-tick recovery
/// hysteresis, and the Calm/Warn/Cap level mapping the chrome indicator binds to. Uses a
/// deterministic fake provider so no Win32 is required.
/// </summary>
public sealed class CapacityModelTests
{
    private const ulong Gb = 1024UL * 1024 * 1024;
    private const ulong Mb = 1024UL * 1024;

    private sealed class FakeCapacityProvider : ICapacityProvider
    {
        public ulong TotalPhysBytes { get; set; } = 16 * Gb;
        public ulong AvailablePhysBytes { get; set; } = 10 * Gb;
        public ulong CommitHeadroomBytes { get; set; } = 10 * Gb;
        public bool IsLowMemorySignaled { get; set; }

        public event Action? LowMemorySignal;

        public Dictionary<int, ulong> ProcessPrivateBytes { get; } = new();

        public void RaiseLowMemory()
        {
            IsLowMemorySignaled = true;
            LowMemorySignal?.Invoke();
        }

        public ulong? MeasureProcessPrivateBytes(int pid)
            => ProcessPrivateBytes.TryGetValue(pid, out ulong bytes) ? bytes : null;
    }

    private sealed class FakeCalibrationStore : ICalibrationStore
    {
        public CapacityCalibration? Stored { get; set; }

        public CapacityCalibration? Load() => Stored;
        public void Save(CapacityCalibration calibration) => Stored = calibration;
    }

    /// <summary>Avail/commit such that the safe zone is exactly <paramref name="safeZone"/>.</summary>
    private static FakeCapacityProvider ProviderWithSafeZone(ulong safeZone) => new()
    {
        AvailablePhysBytes = safeZone + CapacityModel.OsReserveBytes,
        CommitHeadroomBytes = safeZone + CapacityModel.OsReserveBytes,
    };

    // ---- Safe-zone math ------------------------------------------------------------------------

    [Fact]
    public void Happy_path_max_terminals_is_floor_of_safe_zone_over_seed_budget()
    {
        // 8 GB safe zone / 200 MB seed = 40.96 → 40.
        var model = new CapacityModel(ProviderWithSafeZone(8 * Gb));

        Assert.Equal(8 * Gb, model.SafeZoneBytes);
        Assert.Equal(CapacityModel.SeedBudgetBytes, model.PerTerminalBudgetBytes);
        Assert.Equal(40, model.MaxTerminals);
    }

    [Fact]
    public void Safe_zone_takes_the_min_of_phys_and_commit_headroom()
    {
        var provider = new FakeCapacityProvider
        {
            AvailablePhysBytes = 10 * Gb,
            CommitHeadroomBytes = 4 * Gb, // tighter side
        };
        var model = new CapacityModel(provider);

        Assert.Equal(4 * Gb - CapacityModel.OsReserveBytes, model.SafeZoneBytes);
    }

    [Fact]
    public void Safe_zone_saturates_at_zero_when_below_the_os_reserve()
    {
        var provider = new FakeCapacityProvider
        {
            AvailablePhysBytes = 1 * Gb, // < 2 GB reserve
            CommitHeadroomBytes = 1 * Gb,
        };
        var model = new CapacityModel(provider);

        Assert.Equal(0UL, model.SafeZoneBytes);
        Assert.Equal(0, model.MaxTerminals);
        Assert.Null(model.TryReserve(new SurfaceId(1)));
    }

    // ---- Reservation ledger ----------------------------------------------------------------------

    [Fact]
    public void Reserve_then_commit_toctou_at_cap_minus_one_only_first_reserve_wins()
    {
        // Safe zone 400 MB / 200 MB budget → cap 2; one committed → at cap−1.
        var model = new CapacityModel(ProviderWithSafeZone(400 * Mb));
        Assert.Equal(2, model.MaxTerminals);
        ReservationToken? first = model.TryReserve(new SurfaceId(1));
        Assert.NotNull(first);
        model.Commit(first!, pid: 100);

        ReservationToken? second = model.TryReserve(new SurfaceId(2));
        ReservationToken? third = model.TryReserve(new SurfaceId(3));

        Assert.NotNull(second); // slot 2 of 2
        Assert.Null(third);     // reserved-but-uncommitted still counts — no TOCTOU window
    }

    [Fact]
    public void Release_frees_a_slot()
    {
        var model = new CapacityModel(ProviderWithSafeZone(400 * Mb)); // cap 2
        ReservationToken? a = model.TryReserve(new SurfaceId(1));
        ReservationToken? b = model.TryReserve(new SurfaceId(2));
        model.Commit(a!, 100);
        model.Commit(b!, 101);
        Assert.Null(model.TryReserve(new SurfaceId(3)));

        model.Release(new SurfaceId(1));

        Assert.NotNull(model.TryReserve(new SurfaceId(3)));
    }

    [Fact]
    public void Release_of_an_uncommitted_reservation_frees_the_slot()
    {
        var model = new CapacityModel(ProviderWithSafeZone(400 * Mb)); // cap 2
        model.TryReserve(new SurfaceId(1));
        model.TryReserve(new SurfaceId(2));
        Assert.Null(model.TryReserve(new SurfaceId(3)));

        model.Release(new SurfaceId(2)); // e.g. factory threw

        Assert.NotNull(model.TryReserve(new SurfaceId(3)));
    }

    [Fact]
    public void Reserve_and_commit_are_idempotent_per_surface_id()
    {
        var model = new CapacityModel(ProviderWithSafeZone(400 * Mb)); // cap 2

        ReservationToken? first = model.TryReserve(new SurfaceId(1));
        ReservationToken? again = model.TryReserve(new SurfaceId(1)); // duplicate request
        model.Commit(first!, 100);
        model.Commit(first!, 100);                                    // duplicate commit
        ReservationToken? afterCommit = model.TryReserve(new SurfaceId(1));

        Assert.NotNull(first);
        Assert.NotNull(again);
        Assert.NotNull(afterCommit);                  // re-reserving a committed id is a no-op success
        Assert.Equal(1, model.State.Used + model.State.Reserved); // exactly one slot consumed
        Assert.NotNull(model.TryReserve(new SurfaceId(2)));        // second slot still free
    }

    // ---- Calibration -----------------------------------------------------------------------------

    [Fact]
    public void Three_measurements_above_seed_raise_budget_to_p75_and_drop_max()
    {
        var model = new CapacityModel(ProviderWithSafeZone(8 * Gb)); // max 40 at seed

        model.RecordMeasurement(new SurfaceId(1), 300 * Mb);
        model.RecordMeasurement(new SurfaceId(2), 300 * Mb);
        Assert.Equal(CapacityModel.SeedBudgetBytes, model.PerTerminalBudgetBytes); // < 3 samples
        model.RecordMeasurement(new SurfaceId(3), 300 * Mb);

        Assert.Equal(300 * Mb, model.PerTerminalBudgetBytes);
        Assert.Equal(27, model.MaxTerminals); // floor(8192/300)
    }

    [Fact]
    public void Calibration_never_drops_below_the_seed()
    {
        var model = new CapacityModel(ProviderWithSafeZone(8 * Gb));

        model.RecordMeasurement(new SurfaceId(1), 50 * Mb);
        model.RecordMeasurement(new SurfaceId(2), 50 * Mb);
        model.RecordMeasurement(new SurfaceId(3), 50 * Mb);

        Assert.Equal(CapacityModel.SeedBudgetBytes, model.PerTerminalBudgetBytes); // max(seed, P75)
        Assert.Equal(40, model.MaxTerminals);
    }

    [Fact]
    public void Remeasuring_the_same_surface_replaces_its_sample_instead_of_adding_one()
    {
        var model = new CapacityModel(ProviderWithSafeZone(8 * Gb));

        model.RecordMeasurement(new SurfaceId(1), 300 * Mb);
        model.RecordMeasurement(new SurfaceId(1), 320 * Mb);
        model.RecordMeasurement(new SurfaceId(1), 340 * Mb);

        Assert.Equal(CapacityModel.SeedBudgetBytes, model.PerTerminalBudgetBytes); // still 1 sample
    }

    [Fact]
    public void Calibration_round_trips_through_the_store_with_matching_fingerprint()
    {
        var store = new FakeCalibrationStore();
        var provider = ProviderWithSafeZone(8 * Gb);
        provider.TotalPhysBytes = 16 * Gb;
        var model = new CapacityModel(provider, store);
        model.RecordMeasurement(new SurfaceId(1), 300 * Mb);
        model.RecordMeasurement(new SurfaceId(2), 300 * Mb);
        model.RecordMeasurement(new SurfaceId(3), 300 * Mb);

        model.SaveCalibration();

        Assert.NotNull(store.Stored);
        Assert.Equal(300 * Mb, store.Stored!.BudgetBytes);
        Assert.Equal(16, store.Stored.HardwareFingerprintGb);

        var fresh = new CapacityModel(provider, store);
        fresh.LoadCalibration();
        Assert.Equal(300 * Mb, fresh.PerTerminalBudgetBytes);
        Assert.Equal(27, fresh.MaxTerminals);
    }

    [Fact]
    public void Calibration_from_a_different_machine_is_ignored()
    {
        var store = new FakeCalibrationStore
        {
            Stored = new CapacityCalibration(300 * Mb, HardwareFingerprintGb: 32), // 32 GB box
        };
        var provider = ProviderWithSafeZone(8 * Gb);
        provider.TotalPhysBytes = 16 * Gb; // this is a 16 GB box

        var model = new CapacityModel(provider, store);
        model.LoadCalibration();

        Assert.Equal(CapacityModel.SeedBudgetBytes, model.PerTerminalBudgetBytes);
        Assert.Equal(40, model.MaxTerminals);
    }

    // ---- Pressure + hysteresis -------------------------------------------------------------------

    [Fact]
    public void Low_memory_tick_drops_max_but_keeps_committed_terminals()
    {
        var provider = ProviderWithSafeZone(8 * Gb);
        var model = new CapacityModel(provider); // max 40
        for (int i = 1; i <= 6; i++)
        {
            ReservationToken? t = model.TryReserve(new SurfaceId(i));
            model.Commit(t!, 100 + i);
        }

        provider.AvailablePhysBytes = 3 * Gb;  // safe zone → 1 GB
        provider.CommitHeadroomBytes = 3 * Gb;
        provider.RaiseLowMemory();
        model.OnTick();

        Assert.Equal(5, model.MaxTerminals);              // floor(1024/200)
        Assert.Equal(6, model.State.Used);                // live terminals never reaped
        Assert.Equal(CapacityLevel.Cap, model.State.Level);
        Assert.Null(model.TryReserve(new SurfaceId(7)));  // but no new spawns
    }

    [Fact]
    public void One_recovered_tick_does_not_relax_the_cap_two_consecutive_do()
    {
        var provider = ProviderWithSafeZone(8 * Gb);
        var model = new CapacityModel(provider); // max 40

        provider.AvailablePhysBytes = 3 * Gb;
        provider.CommitHeadroomBytes = 3 * Gb;
        provider.RaiseLowMemory();
        model.OnTick();
        Assert.Equal(5, model.MaxTerminals);

        provider.IsLowMemorySignaled = false;
        provider.AvailablePhysBytes = 10 * Gb;   // ≥ 1.25 × 1 GB safe zone
        provider.CommitHeadroomBytes = 10 * Gb;
        model.OnTick();
        Assert.Equal(5, model.MaxTerminals);     // first recovered tick: hold

        model.OnTick();
        Assert.Equal(40, model.MaxTerminals);    // second consecutive: relax
        Assert.Equal(8 * Gb, model.SafeZoneBytes);
    }

    [Fact]
    public void Recovery_streak_resets_if_a_tick_is_not_recovered()
    {
        var provider = ProviderWithSafeZone(8 * Gb);
        var model = new CapacityModel(provider);

        provider.AvailablePhysBytes = 3 * Gb;
        provider.CommitHeadroomBytes = 3 * Gb;
        provider.RaiseLowMemory();
        model.OnTick();
        Assert.Equal(5, model.MaxTerminals);

        provider.IsLowMemorySignaled = false;
        provider.AvailablePhysBytes = 10 * Gb;
        provider.CommitHeadroomBytes = 10 * Gb;
        model.OnTick();                          // recovered tick #1

        provider.IsLowMemorySignaled = true;     // pressure returns
        provider.AvailablePhysBytes = 3 * Gb;
        provider.CommitHeadroomBytes = 3 * Gb;
        model.OnTick();                          // streak broken
        Assert.Equal(5, model.MaxTerminals);

        provider.IsLowMemorySignaled = false;
        provider.AvailablePhysBytes = 10 * Gb;
        provider.CommitHeadroomBytes = 10 * Gb;
        model.OnTick();                          // recovered tick #1 again
        Assert.Equal(5, model.MaxTerminals);     // still held — streak restarted
        model.OnTick();
        Assert.Equal(40, model.MaxTerminals);
    }

    [Fact]
    public void Low_memory_signal_alone_blocks_recovery_even_with_high_avail()
    {
        var provider = ProviderWithSafeZone(8 * Gb);
        var model = new CapacityModel(provider);

        provider.AvailablePhysBytes = 3 * Gb;
        provider.CommitHeadroomBytes = 3 * Gb;
        provider.RaiseLowMemory();
        model.OnTick();
        Assert.Equal(5, model.MaxTerminals);

        provider.AvailablePhysBytes = 10 * Gb;   // numbers look fine…
        provider.CommitHeadroomBytes = 10 * Gb;  // …but the OS signal is still raised
        model.OnTick();
        model.OnTick();
        Assert.Equal(5, model.MaxTerminals);
    }

    // ---- Level mapping + state event -------------------------------------------------------------

    [Fact]
    public void Level_is_calm_below_75_percent_warn_at_75_cap_at_max()
    {
        var model = new CapacityModel(ProviderWithSafeZone(800 * Mb)); // cap 4

        Assert.Equal(CapacityLevel.Calm, model.State.Level); // 0/4

        model.Commit(model.TryReserve(new SurfaceId(1))!, 101);
        model.Commit(model.TryReserve(new SurfaceId(2))!, 102);
        Assert.Equal(CapacityLevel.Calm, model.State.Level); // 2/4 = 50%

        model.TryReserve(new SurfaceId(3));                  // reserved counts toward level
        Assert.Equal(CapacityLevel.Warn, model.State.Level); // 3/4 = 75%

        model.Commit(model.TryReserve(new SurfaceId(3))!, 103);
        model.Commit(model.TryReserve(new SurfaceId(4))!, 104);
        Assert.Equal(CapacityLevel.Cap, model.State.Level);  // 4/4
    }

    [Fact]
    public void State_changed_fires_on_ledger_and_tick_transitions()
    {
        var provider = ProviderWithSafeZone(8 * Gb);
        var model = new CapacityModel(provider);
        var seen = new List<CapacityState>();
        model.StateChanged += s => seen.Add(s);

        ReservationToken? t = model.TryReserve(new SurfaceId(1));
        model.Commit(t!, 100);
        model.Release(new SurfaceId(1));
        provider.AvailablePhysBytes = 3 * Gb;
        provider.CommitHeadroomBytes = 3 * Gb;
        model.OnTick();

        Assert.Equal(4, seen.Count);
        Assert.Equal(new CapacityState(0, 1, 40, CapacityLevel.Calm), seen[0]);
        Assert.Equal(new CapacityState(1, 0, 40, CapacityLevel.Calm), seen[1]);
        Assert.Equal(new CapacityState(0, 0, 40, CapacityLevel.Calm), seen[2]);
        Assert.Equal(new CapacityState(0, 0, 5, CapacityLevel.Calm), seen[3]);
    }

    [Fact]
    public void Unchanged_tick_does_not_fire_state_changed()
    {
        var model = new CapacityModel(ProviderWithSafeZone(8 * Gb));
        int fired = 0;
        model.StateChanged += _ => fired++;

        model.OnTick(); // same provider numbers → same state

        Assert.Equal(0, fired);
    }
}
