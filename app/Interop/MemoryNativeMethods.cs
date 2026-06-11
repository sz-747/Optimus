using System;
using System.Runtime.InteropServices;

namespace Optimus.Interop;

/// <summary>
/// Win32 memory-measurement P/Invokes for the RAM safe-zone capacity governor (plan U1).
/// System-wide numbers come from <c>GlobalMemoryStatusEx</c> / <c>GetPerformanceInfo</c>,
/// per-terminal private bytes from <c>GetProcessMemoryInfo</c>, and event-driven pressure
/// from the memory-resource-notification pair. Prefer the <c>TryGet*</c> wrappers — they
/// set the size fields (<c>dwLength</c> / <c>cb</c>) the raw APIs silently fail without.
/// </summary>
internal static partial class MemoryNativeMethods
{
    /// <summary>Access right for <see cref="OpenProcess"/>: enough to query memory counters.</summary>
    internal const uint PROCESS_QUERY_LIMITED_INFORMATION = 0x1000;

    /// <summary>Access right for <see cref="OpenProcess"/>: required by AssignProcessToJobObject.</summary>
    internal const uint PROCESS_TERMINATE = 0x0001;

    /// <summary>Access right for <see cref="OpenProcess"/>: required by AssignProcessToJobObject.</summary>
    internal const uint PROCESS_SET_QUOTA = 0x0100;

    /// <summary>MEMORY_RESOURCE_NOTIFICATION_TYPE: signal when system memory is low.</summary>
    internal const int LowMemoryResourceNotification = 0;

    [LibraryImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool GlobalMemoryStatusEx(ref MEMORYSTATUSEX lpBuffer);

    [LibraryImport("psapi.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool GetPerformanceInfo(ref PERFORMANCE_INFORMATION pPerformanceInformation, uint cb);

    [LibraryImport("psapi.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool GetProcessMemoryInfo(IntPtr hProcess, ref PROCESS_MEMORY_COUNTERS_EX ppsmemCounters, uint cb);

    [LibraryImport("kernel32.dll", SetLastError = true)]
    internal static partial IntPtr OpenProcess(uint dwDesiredAccess, [MarshalAs(UnmanagedType.Bool)] bool bInheritHandle, uint dwProcessId);

    [LibraryImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static partial bool CloseHandle(IntPtr hObject);

    [LibraryImport("kernel32.dll", SetLastError = true)]
    internal static partial IntPtr CreateMemoryResourceNotification(int notificationType);

    [LibraryImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static partial bool QueryMemoryResourceNotification(IntPtr resourceNotificationHandle, [MarshalAs(UnmanagedType.Bool)] out bool resourceState);

    /// <summary>
    /// <c>GlobalMemoryStatusEx</c> with <c>dwLength</c> pre-set — the raw API fails with
    /// ERROR_INVALID_PARAMETER if the caller forgets it (the #1 P/Invoke failure mode here).
    /// </summary>
    internal static bool TryGetMemoryStatus(out MEMORYSTATUSEX status)
    {
        status = default;
        status.dwLength = (uint)Marshal.SizeOf<MEMORYSTATUSEX>();
        return GlobalMemoryStatusEx(ref status);
    }

    /// <summary><c>GetPerformanceInfo</c> with <c>cb</c> pre-set (same trap as dwLength).</summary>
    internal static bool TryGetPerformanceInfo(out PERFORMANCE_INFORMATION info)
    {
        info = default;
        uint cb = (uint)Marshal.SizeOf<PERFORMANCE_INFORMATION>();
        info.cb = cb;
        return GetPerformanceInfo(ref info, cb);
    }

    /// <summary><c>GetProcessMemoryInfo</c> with <c>cb</c> pre-set. PrivateUsage is the per-terminal feed.</summary>
    internal static bool TryGetProcessMemoryInfo(IntPtr hProcess, out PROCESS_MEMORY_COUNTERS_EX counters)
    {
        counters = default;
        uint cb = (uint)Marshal.SizeOf<PROCESS_MEMORY_COUNTERS_EX>();
        counters.cb = cb;
        return GetProcessMemoryInfo(hProcess, ref counters, cb);
    }
}

[StructLayout(LayoutKind.Sequential)]
internal struct MEMORYSTATUSEX
{
    /// <summary>Must equal sizeof(MEMORYSTATUSEX) before the call; see TryGetMemoryStatus.</summary>
    public uint dwLength;
    public uint dwMemoryLoad;
    public ulong ullTotalPhys;
    public ulong ullAvailPhys;
    public ulong ullTotalPageFile;
    public ulong ullAvailPageFile;
    public ulong ullTotalVirtual;
    public ulong ullAvailVirtual;
    public ulong ullAvailExtendedVirtual;
}

[StructLayout(LayoutKind.Sequential)]
internal struct PERFORMANCE_INFORMATION
{
    /// <summary>Must equal sizeof(PERFORMANCE_INFORMATION) before the call; see TryGetPerformanceInfo.</summary>
    public uint cb;
    // SIZE_T fields are in PAGES, not bytes — multiply by PageSize.
    public nuint CommitTotal;
    public nuint CommitLimit;
    public nuint CommitPeak;
    public nuint PhysicalTotal;
    public nuint PhysicalAvailable;
    public nuint SystemCache;
    public nuint KernelTotal;
    public nuint KernelPaged;
    public nuint KernelNonpaged;
    public nuint PageSize;
    public uint HandleCount;
    public uint ProcessCount;
    public uint ThreadCount;
}

[StructLayout(LayoutKind.Sequential)]
internal struct PROCESS_MEMORY_COUNTERS_EX
{
    /// <summary>Must equal sizeof(PROCESS_MEMORY_COUNTERS_EX) before the call; see TryGetProcessMemoryInfo.</summary>
    public uint cb;
    public uint PageFaultCount;
    public nuint PeakWorkingSetSize;
    public nuint WorkingSetSize;
    public nuint QuotaPeakPagedPoolUsage;
    public nuint QuotaPagedPoolUsage;
    public nuint QuotaPeakNonPagedPoolUsage;
    public nuint QuotaNonPagedPoolUsage;
    public nuint PagefileUsage;
    public nuint PeakPagefileUsage;
    /// <summary>Private (non-shared) committed bytes — the per-terminal calibration feed.</summary>
    public nuint PrivateUsage;
}
