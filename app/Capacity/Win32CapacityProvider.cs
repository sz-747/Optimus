using System;
using Optimus.Core;
using Optimus.Interop;

namespace Optimus.Capacity;

/// <summary>
/// Production <see cref="ICapacityProvider"/> binding (RAM safe-zone plan U3): feeds the Tier-1
/// <see cref="CapacityModel"/> real numbers from the U1 P/Invokes. System-wide facts come from
/// <c>GlobalMemoryStatusEx</c> / <c>GetPerformanceInfo</c>, per-terminal private bytes from
/// <c>GetProcessMemoryInfo</c>, and pressure from a lazily created
/// <c>LowMemoryResourceNotification</c> object. All reads fail conservative: a Win32 failure
/// reports zero headroom (the model tightens, never crashes).
/// </summary>
internal sealed class Win32CapacityProvider : ICapacityProvider, IDisposable
{
    private readonly object _gate = new();
    private IntPtr _lowMemoryNotification;
    private bool _disposed;

    /// <inheritdoc/>
    public ulong TotalPhysBytes =>
        MemoryNativeMethods.TryGetMemoryStatus(out MEMORYSTATUSEX status) ? status.ullTotalPhys : 0;

    /// <inheritdoc/>
    public ulong AvailablePhysBytes =>
        MemoryNativeMethods.TryGetMemoryStatus(out MEMORYSTATUSEX status) ? status.ullAvailPhys : 0;

    /// <inheritdoc/>
    public ulong CommitHeadroomBytes
    {
        get
        {
            if (!MemoryNativeMethods.TryGetPerformanceInfo(out PERFORMANCE_INFORMATION info))
            {
                return 0;
            }
            // CommitLimit / CommitTotal are in PAGES (see U1 struct comment) — convert to bytes.
            ulong limit = info.CommitLimit;
            ulong total = info.CommitTotal;
            return limit > total ? (limit - total) * info.PageSize : 0;
        }
    }

    /// <inheritdoc/>
    public bool IsLowMemorySignaled
    {
        get
        {
            IntPtr handle = NotificationHandle;
            return handle != IntPtr.Zero
                && MemoryNativeMethods.QueryMemoryResourceNotification(handle, out bool low)
                && low;
        }
    }

    /// <inheritdoc/>
    public event Action? LowMemorySignal;

    /// <summary>
    /// The low-memory resource-notification handle, lazily created. The ticker (U3) registers a
    /// thread-pool wait on it; <see cref="IsLowMemorySignaled"/> polls it. Zero when creation
    /// failed or the provider is disposed. Owned by this provider — do not close it elsewhere.
    /// </summary>
    internal IntPtr NotificationHandle
    {
        get
        {
            lock (_gate)
            {
                if (_disposed)
                {
                    return IntPtr.Zero;
                }
                if (_lowMemoryNotification == IntPtr.Zero)
                {
                    _lowMemoryNotification = MemoryNativeMethods.CreateMemoryResourceNotification(
                        MemoryNativeMethods.LowMemoryResourceNotification);
                }
                return _lowMemoryNotification;
            }
        }
    }

    /// <inheritdoc/>
    public ulong? MeasureProcessPrivateBytes(int pid)
    {
        IntPtr process = MemoryNativeMethods.OpenProcess(
            MemoryNativeMethods.PROCESS_QUERY_LIMITED_INFORMATION,
            bInheritHandle: false,
            (uint)pid);
        if (process == IntPtr.Zero)
        {
            return null;
        }

        try
        {
            return MemoryNativeMethods.TryGetProcessMemoryInfo(process, out PROCESS_MEMORY_COUNTERS_EX counters)
                ? counters.PrivateUsage
                : null;
        }
        finally
        {
            MemoryNativeMethods.CloseHandle(process);
        }
    }

    /// <summary>Raised by <see cref="CapacityTicker"/> when the OS notification fires.</summary>
    internal void RaiseLowMemorySignal() => LowMemorySignal?.Invoke();

    public void Dispose()
    {
        lock (_gate)
        {
            if (_disposed)
            {
                return;
            }
            _disposed = true;
            if (_lowMemoryNotification != IntPtr.Zero)
            {
                MemoryNativeMethods.CloseHandle(_lowMemoryNotification);
                _lowMemoryNotification = IntPtr.Zero;
            }
        }
    }
}
