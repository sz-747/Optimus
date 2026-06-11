using System;

namespace Optimus.Core;

/// <summary>
/// Memory facts the capacity governor needs, abstracted so <see cref="CapacityModel"/> stays
/// pure Core (RAM safe-zone plan U2). The production binding is the Win32 provider in the app
/// layer (U3: <c>GlobalMemoryStatusEx</c> / <c>GetPerformanceInfo</c> /
/// <c>QueryMemoryResourceNotification</c>); tests use a deterministic fake.
/// </summary>
public interface ICapacityProvider
{
    /// <summary>Total installed physical RAM. Feeds the calibration hardware fingerprint.</summary>
    ulong TotalPhysBytes { get; }

    /// <summary>Physical RAM currently available (<c>ullAvailPhys</c>).</summary>
    ulong AvailablePhysBytes { get; }

    /// <summary>Commit headroom: <c>CommitLimit − CommitTotal</c>.</summary>
    ulong CommitHeadroomBytes { get; }

    /// <summary>
    /// Whether the OS low-memory resource notification is currently signaled. Polled at tick
    /// time; while true, the cap may tighten but never relax.
    /// </summary>
    bool IsLowMemorySignaled { get; }

    /// <summary>Fires when the OS raises the low-memory resource notification.</summary>
    event Action? LowMemorySignal;

    /// <summary>
    /// Private bytes (<c>PrivateUsage</c>) of the process with <paramref name="pid"/>, or
    /// <c>null</c> if it cannot be measured (exited, access denied).
    /// </summary>
    ulong? MeasureProcessPrivateBytes(int pid);
}
