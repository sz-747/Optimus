using System;
using System.Collections.Generic;
using System.Linq;

namespace Optimus.Core;

/// <summary>
/// Two-phase ticket handed out by <see cref="CapacityModel.TryReserve"/> and redeemed by
/// <see cref="CapacityModel.Commit"/>. Holding a token means a safe-zone slot is held for
/// <see cref="Id"/> — parallel spawns cannot race past the cap (TOCTOU guard).
/// </summary>
public sealed record ReservationToken(SurfaceId Id);

/// <summary>
/// Tier-1 soft admission governor (RAM safe-zone plan U2): owns the safe-zone math, the
/// per-terminal budget calibration, and the reserve-then-commit ledger that
/// <c>SurfaceManager.CreateSurface</c> gates on (U5). Pure Core — all memory facts come from an
/// injected <see cref="ICapacityProvider"/>, all persistence from an injected
/// <see cref="ICalibrationStore"/>, so the whole policy is unit-testable without Win32.
/// Thread-safe: parallel spawns hit <see cref="TryReserve"/> concurrently.
///
/// Cap policy: <see cref="MaxTerminals"/> is monotonically non-increasing under pressure within
/// a session (we tighten, never silently relax). Relaxation requires the OS low-memory signal to
/// be clear AND available RAM ≥ <see cref="RecoveryHeadroomFactor"/> × the tightened safe zone
/// for <see cref="RecoveryTicksRequired"/> consecutive ticks. Live terminals are never reaped:
/// a shrink can leave <c>Used &gt; Max</c>, which only blocks new spawns.
/// </summary>
public sealed class CapacityModel
{
    /// <summary>Headroom always left for the OS — never handed to terminals.</summary>
    public const ulong OsReserveBytes = 2UL * 1024 * 1024 * 1024;

    /// <summary>Conservative first-launch per-terminal budget; calibration can only raise it.</summary>
    public const ulong SeedBudgetBytes = 200UL * 1024 * 1024;

    /// <summary>Samples required before measured budgets replace the seed.</summary>
    public const int CalibrationMinSamples = 3;

    /// <summary>Available RAM must reach this multiple of the safe zone before the cap relaxes.</summary>
    public const double RecoveryHeadroomFactor = 1.25;

    /// <summary>Consecutive recovered ticks required before the cap relaxes.</summary>
    public const int RecoveryTicksRequired = 2;

    private readonly object _gate = new();
    private readonly ICapacityProvider _provider;
    private readonly ICalibrationStore? _store;
    private readonly Dictionary<SurfaceId, int> _committed = new(); // surface → pid
    private readonly HashSet<SurfaceId> _reserved = new();
    private readonly Dictionary<SurfaceId, ulong> _samples = new(); // latest measurement per surface

    private ulong _safeZoneBytes;
    private ulong _budgetBytes = SeedBudgetBytes;
    private int _maxTerminals;
    private int _recoveryStreak;
    private bool _lowSignalPending;

    public CapacityModel(ICapacityProvider provider, ICalibrationStore? store = null)
    {
        _provider = provider ?? throw new ArgumentNullException(nameof(provider));
        _store = store;
        _provider.LowMemorySignal += OnLowMemorySignal;
        _safeZoneBytes = ComputeSafeZone(provider.AvailablePhysBytes, provider.CommitHeadroomBytes);
        _maxTerminals = FloorTerminals(_safeZoneBytes, _budgetBytes);
    }

    public ulong SafeZoneBytes { get { lock (_gate) { return _safeZoneBytes; } } }

    public ulong PerTerminalBudgetBytes { get { lock (_gate) { return _budgetBytes; } } }

    public int MaxTerminals { get { lock (_gate) { return _maxTerminals; } } }

    /// <summary>Snapshot for the chrome indicator.</summary>
    public CapacityState State { get { lock (_gate) { return Snapshot(); } } }

    /// <summary>Fires (outside the lock) whenever <see cref="State"/> changes.</summary>
    public event Action<CapacityState>? StateChanged;

    // ---- Reservation ledger --------------------------------------------------------------------

    /// <summary>
    /// Hold a safe-zone slot for <paramref name="id"/>, or <c>null</c> when committed + reserved
    /// already fills the cap. Idempotent per id (mirrors <see cref="SurfaceManager"/> R1): a
    /// duplicate request for an id that is already reserved or committed succeeds without
    /// consuming a second slot.
    /// </summary>
    public ReservationToken? TryReserve(SurfaceId id)
    {
        CapacityState? changed;
        ReservationToken? token;
        lock (_gate)
        {
            if (_committed.ContainsKey(id) || _reserved.Contains(id))
            {
                return new ReservationToken(id);
            }
            if (_committed.Count + _reserved.Count >= _maxTerminals)
            {
                return null;
            }
            _reserved.Add(id);
            token = new ReservationToken(id);
            changed = Snapshot();
        }
        Raise(changed);
        return token;
    }

    /// <summary>Convert a reservation into a committed terminal with <paramref name="pid"/>. Idempotent.</summary>
    public void Commit(ReservationToken token, int pid)
    {
        ArgumentNullException.ThrowIfNull(token);
        CapacityState? changed = null;
        lock (_gate)
        {
            if (!_committed.ContainsKey(token.Id))
            {
                _reserved.Remove(token.Id);
                _committed[token.Id] = pid;
                changed = Snapshot();
            }
        }
        Raise(changed);
    }

    /// <summary>Free the slot for <paramref name="id"/>, reserved or committed. No-op otherwise.</summary>
    public void Release(SurfaceId id)
    {
        CapacityState? changed = null;
        lock (_gate)
        {
            bool removed = _reserved.Remove(id) | _committed.Remove(id);
            _samples.Remove(id);
            if (removed)
            {
                changed = Snapshot();
            }
        }
        Raise(changed);
    }

    // ---- Calibration -----------------------------------------------------------------------------

    /// <summary>
    /// Record a measured private-bytes sample for a live terminal (latest sample per surface
    /// wins). Once ≥ <see cref="CalibrationMinSamples"/> surfaces have samples, the budget becomes
    /// <c>max(seed, P75(samples))</c>; a budget rise tightens <see cref="MaxTerminals"/>
    /// immediately, a fall only relaxes via the tick hysteresis.
    /// </summary>
    public void RecordMeasurement(SurfaceId id, ulong bytes)
    {
        CapacityState? changed = null;
        lock (_gate)
        {
            _samples[id] = bytes;
            if (_samples.Count >= CalibrationMinSamples)
            {
                changed = ApplyBudgetLocked(Math.Max(SeedBudgetBytes, Percentile75(_samples.Values)));
            }
        }
        Raise(changed);
    }

    /// <summary>Persist the current budget plus the hardware fingerprint of this machine.</summary>
    public void SaveCalibration()
    {
        if (_store is null)
        {
            return;
        }
        ulong budget;
        lock (_gate) { budget = _budgetBytes; }
        _store.Save(new CapacityCalibration(budget, FingerprintGb(_provider.TotalPhysBytes)));
    }

    /// <summary>
    /// Adopt a persisted budget so the next session starts already-calibrated. Ignored when no
    /// calibration exists or its hardware fingerprint does not match this machine.
    /// </summary>
    public void LoadCalibration()
    {
        CapacityCalibration? calibration = _store?.Load();
        if (calibration is null || calibration.HardwareFingerprintGb != FingerprintGb(_provider.TotalPhysBytes))
        {
            return;
        }
        CapacityState? changed;
        lock (_gate)
        {
            changed = ApplyBudgetLocked(Math.Max(SeedBudgetBytes, calibration.BudgetBytes));
        }
        Raise(changed);
    }

    // ---- Tick ------------------------------------------------------------------------------------

    /// <summary>
    /// Re-read the provider and update the safe zone per the cap policy. Called at 1 Hz and on
    /// the low-memory signal (U3).
    /// </summary>
    public void OnTick()
    {
        ulong avail = _provider.AvailablePhysBytes;
        ulong headroom = _provider.CommitHeadroomBytes;
        bool lowNow = _provider.IsLowMemorySignaled;

        CapacityState? changed = null;
        lock (_gate)
        {
            lowNow |= _lowSignalPending;
            _lowSignalPending = false;

            ulong candidateZone = ComputeSafeZone(avail, headroom);
            int candidateMax = FloorTerminals(candidateZone, _budgetBytes);
            CapacityState before = Snapshot();

            if (candidateMax < _maxTerminals)
            {
                _safeZoneBytes = candidateZone;
                _maxTerminals = candidateMax;
                _recoveryStreak = 0;
            }
            else if (candidateMax > _maxTerminals)
            {
                bool recovered = !lowNow && avail >= (ulong)(_safeZoneBytes * RecoveryHeadroomFactor);
                _recoveryStreak = recovered ? _recoveryStreak + 1 : 0;
                if (_recoveryStreak >= RecoveryTicksRequired)
                {
                    _safeZoneBytes = candidateZone;
                    _maxTerminals = candidateMax;
                    _recoveryStreak = 0;
                }
            }
            else
            {
                _safeZoneBytes = candidateZone;
                _recoveryStreak = 0;
            }

            CapacityState after = Snapshot();
            if (after != before)
            {
                changed = after;
            }
        }
        Raise(changed);
    }

    // ---- Internals -------------------------------------------------------------------------------

    private void OnLowMemorySignal()
    {
        lock (_gate) { _lowSignalPending = true; }
    }

    /// <summary>Apply a new budget; only ever tightens the cap (relaxation goes through OnTick).</summary>
    private CapacityState? ApplyBudgetLocked(ulong budget)
    {
        if (budget == _budgetBytes)
        {
            return null;
        }
        CapacityState before = Snapshot();
        _budgetBytes = budget;
        _maxTerminals = Math.Min(_maxTerminals, FloorTerminals(_safeZoneBytes, _budgetBytes));
        CapacityState after = Snapshot();
        return after != before ? after : null;
    }

    private CapacityState Snapshot()
    {
        int used = _committed.Count;
        int reserved = _reserved.Count;
        return new CapacityState(used, reserved, _maxTerminals, Level(used + reserved, _maxTerminals));
    }

    private static CapacityLevel Level(int total, int max)
    {
        if (total >= max)
        {
            return CapacityLevel.Cap;
        }
        return total * 4L >= max * 3L ? CapacityLevel.Warn : CapacityLevel.Calm;
    }

    private static ulong ComputeSafeZone(ulong availPhys, ulong commitHeadroom)
    {
        ulong tighter = Math.Min(availPhys, commitHeadroom);
        return tighter > OsReserveBytes ? tighter - OsReserveBytes : 0;
    }

    private static int FloorTerminals(ulong safeZone, ulong budget)
        => (int)Math.Min(int.MaxValue, safeZone / budget);

    /// <summary>Nearest-rank P75.</summary>
    private static ulong Percentile75(IEnumerable<ulong> samples)
    {
        ulong[] sorted = samples.OrderBy(s => s).ToArray();
        int rank = (int)Math.Ceiling(sorted.Length * 0.75) - 1;
        return sorted[rank];
    }

    private static int FingerprintGb(ulong totalPhysBytes)
    {
        const ulong gb = 1024UL * 1024 * 1024;
        return (int)((totalPhysBytes + gb / 2) / gb);
    }

    private void Raise(CapacityState? state)
    {
        if (state is CapacityState s)
        {
            StateChanged?.Invoke(s);
        }
    }
}
