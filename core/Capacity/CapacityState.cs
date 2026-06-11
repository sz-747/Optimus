namespace Optimus.Core;

/// <summary>Escalation level of the capacity indicator (DESIGN.md: calm → amber → red).</summary>
public enum CapacityLevel
{
    /// <summary>Below 75% of the safe-zone cap.</summary>
    Calm,

    /// <summary>At or above 75% of the cap.</summary>
    Warn,

    /// <summary>At (or, after a pressure-tightening, above) the cap. No new spawns.</summary>
    Cap,
}

/// <summary>
/// Immutable snapshot of the capacity ledger that the chrome indicator (U6) binds to.
/// <paramref name="Used"/> is committed terminals, <paramref name="Reserved"/> is in-flight
/// reservations; the level is computed on the <c>Used + Reserved</c> basis.
/// </summary>
public readonly record struct CapacityState(int Used, int Reserved, int Max, CapacityLevel Level);
