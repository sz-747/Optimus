using System;
using System.Collections.Generic;
using System.ComponentModel;
using Optimus.Core;
using Xunit;

namespace Optimus.Core.Tests;

/// <summary>
/// Coverage for <see cref="CapacityIndicatorViewModel"/> (RAM safe-zone plan U6): label
/// formatting (including the null-model dash placeholder), fraction clamping, level passthrough
/// (the VM trusts <see cref="CapacityState.Level"/> — never recomputes), the cap hint, marshalling
/// of <see cref="CapacityModel.StateChanged"/> through the injected dispatcher delegate, and that
/// dispose unsubscribes. Dispatcher-agnostic by design — no WinUI required.
/// </summary>
public sealed class CapacityIndicatorViewModelTests
{
    private const ulong Gb = 1024UL * 1024 * 1024;

    private sealed class FakeCapacityProvider : ICapacityProvider
    {
        public ulong TotalPhysBytes { get; set; } = 16 * Gb;
        public ulong AvailablePhysBytes { get; set; } = 10 * Gb;
        public ulong CommitHeadroomBytes { get; set; } = 10 * Gb;
        public bool IsLowMemorySignaled { get; set; }

#pragma warning disable CS0067 // never raised in these tests
        public event Action? LowMemorySignal;
#pragma warning restore CS0067

        public ulong? MeasureProcessPrivateBytes(int pid) => null;
    }

    /// <summary>Avail/commit such that the safe zone yields exactly <paramref name="maxTerminals"/> slots.</summary>
    private static CapacityModel ModelWithMax(int maxTerminals)
    {
        ulong safeZone = (ulong)maxTerminals * CapacityModel.SeedBudgetBytes;
        var model = new CapacityModel(new FakeCapacityProvider
        {
            AvailablePhysBytes = safeZone + CapacityModel.OsReserveBytes,
            CommitHeadroomBytes = safeZone + CapacityModel.OsReserveBytes,
        });
        Assert.Equal(maxTerminals, model.MaxTerminals); // guard the fixture
        return model;
    }

    /// <summary>Records dispatched actions; runs them only when told to (proves marshalling).</summary>
    private sealed class FakeDispatcher
    {
        public List<Action> Pending { get; } = new();

        public void Dispatch(Action action) => Pending.Add(action);

        public void RunAll()
        {
            foreach (Action action in Pending)
            {
                action();
            }
            Pending.Clear();
        }
    }

    private static readonly Action<Action> Inline = action => action();

    // ---- Null model ------------------------------------------------------------------------------

    [Fact]
    public void Null_model_shows_dash_placeholder_and_never_caps()
    {
        using var vm = new CapacityIndicatorViewModel(null, Inline);

        Assert.Equal("— / — terminals", vm.LabelText);
        Assert.Equal(0.0, vm.FractionUsed);
        Assert.Equal(CapacityLevel.Calm, vm.Level);
        Assert.False(vm.IsAtCap);
        Assert.Null(vm.HintText);
    }

    [Fact]
    public void Null_dispatcher_throws()
    {
        Assert.Throws<ArgumentNullException>(() => new CapacityIndicatorViewModel(null, null!));
    }

    // ---- Label / fraction / level from a live model ------------------------------------------------

    [Fact]
    public void Label_counts_used_plus_reserved_over_max()
    {
        CapacityModel model = ModelWithMax(10);
        ReservationToken committed = model.TryReserve(new SurfaceId(1))!;
        model.Commit(committed, pid: 100);
        model.TryReserve(new SurfaceId(2)); // stays reserved

        using var vm = new CapacityIndicatorViewModel(model, Inline);

        Assert.Equal("2 / 10 terminals", vm.LabelText);
        Assert.Equal(0.2, vm.FractionUsed, precision: 10);
        Assert.Equal(CapacityLevel.Calm, vm.Level);
        Assert.False(vm.IsAtCap);
    }

    [Fact]
    public void Level_is_passed_through_from_state_not_recomputed()
    {
        CapacityModel model = ModelWithMax(4);
        for (int i = 1; i <= 3; i++) // 3/4 = 75% → Warn per the model
        {
            model.TryReserve(new SurfaceId(i));
        }

        using var vm = new CapacityIndicatorViewModel(model, Inline);

        Assert.Equal(model.State.Level, vm.Level);
        Assert.Equal(CapacityLevel.Warn, vm.Level);
        Assert.False(vm.IsAtCap);
        Assert.Null(vm.HintText);
    }

    [Fact]
    public void At_cap_sets_IsAtCap_and_hint()
    {
        CapacityModel model = ModelWithMax(2);
        model.TryReserve(new SurfaceId(1));
        model.TryReserve(new SurfaceId(2));

        using var vm = new CapacityIndicatorViewModel(model, Inline);

        Assert.True(vm.IsAtCap);
        Assert.Equal(CapacityIndicatorViewModel.CapHint, vm.HintText);
        Assert.Equal(1.0, vm.FractionUsed);
    }

    [Fact]
    public void Fraction_is_zero_when_max_is_zero()
    {
        // Starved machine: safe zone 0 → Max 0 (and Level == Cap, 0 ≥ 0).
        var model = new CapacityModel(new FakeCapacityProvider
        {
            AvailablePhysBytes = CapacityModel.OsReserveBytes, // safe zone exactly 0
            CommitHeadroomBytes = CapacityModel.OsReserveBytes,
        });
        Assert.Equal(0, model.MaxTerminals);

        using var vm = new CapacityIndicatorViewModel(model, Inline);

        Assert.Equal(0.0, vm.FractionUsed);
        Assert.Equal("0 / 0 terminals", vm.LabelText);
        Assert.True(vm.IsAtCap);
    }

    // ---- Marshalling -----------------------------------------------------------------------------

    [Fact]
    public void State_changes_marshal_through_the_injected_dispatcher()
    {
        CapacityModel model = ModelWithMax(10);
        var dispatcher = new FakeDispatcher();
        using var vm = new CapacityIndicatorViewModel(model, dispatcher.Dispatch);
        var raised = new List<string?>();
        vm.PropertyChanged += (_, e) => raised.Add(e.PropertyName);

        model.TryReserve(new SurfaceId(1));

        // Until the dispatcher runs, the VM still shows the snapshot it was built with.
        Assert.Single(dispatcher.Pending);
        Assert.Equal("0 / 10 terminals", vm.LabelText);
        Assert.Empty(raised);

        dispatcher.RunAll();

        Assert.Equal("1 / 10 terminals", vm.LabelText);
        Assert.Contains(nameof(CapacityIndicatorViewModel.LabelText), raised);
        Assert.Contains(nameof(CapacityIndicatorViewModel.FractionUsed), raised);
        Assert.Contains(nameof(CapacityIndicatorViewModel.Level), raised);
        Assert.Contains(nameof(CapacityIndicatorViewModel.IsAtCap), raised);
        Assert.Contains(nameof(CapacityIndicatorViewModel.HintText), raised);
    }

    [Fact]
    public void Reaching_cap_via_state_change_flips_IsAtCap_and_hint()
    {
        CapacityModel model = ModelWithMax(2);
        using var vm = new CapacityIndicatorViewModel(model, Inline);
        Assert.False(vm.IsAtCap);

        model.TryReserve(new SurfaceId(1));
        model.TryReserve(new SurfaceId(2));

        Assert.True(vm.IsAtCap);
        Assert.Equal(CapacityIndicatorViewModel.CapHint, vm.HintText);

        model.Release(new SurfaceId(2));

        Assert.False(vm.IsAtCap);
        Assert.Null(vm.HintText);
    }

    // ---- Dispose ---------------------------------------------------------------------------------

    [Fact]
    public void Dispose_unsubscribes_from_the_model()
    {
        CapacityModel model = ModelWithMax(10);
        var dispatcher = new FakeDispatcher();
        var vm = new CapacityIndicatorViewModel(model, dispatcher.Dispatch);

        vm.Dispose();
        model.TryReserve(new SurfaceId(1));

        Assert.Empty(dispatcher.Pending);
        Assert.Equal("0 / 10 terminals", vm.LabelText); // frozen at the pre-dispose snapshot
    }

    [Fact]
    public void Dispose_is_idempotent_and_drops_late_dispatched_updates()
    {
        CapacityModel model = ModelWithMax(10);
        var dispatcher = new FakeDispatcher();
        var vm = new CapacityIndicatorViewModel(model, dispatcher.Dispatch);

        model.TryReserve(new SurfaceId(1)); // queued on the fake dispatcher
        vm.Dispose();
        vm.Dispose();
        dispatcher.RunAll(); // the queued update arrives after dispose → ignored

        Assert.Equal("0 / 10 terminals", vm.LabelText);
    }
}
