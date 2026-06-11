using System;
using System.Runtime.InteropServices;

namespace Optimus.Interop;

/// <summary>
/// Job Object P/Invokes for tier-2 hard memory enforcement (plan U1/U4): each terminal's
/// ConPTY child is enrolled in a job with JOB_OBJECT_LIMIT_PROCESS_MEMORY so a runaway
/// allocation fails inside the terminal instead of taking down the machine.
/// CloseHandle lives on <see cref="MemoryNativeMethods"/> (shared).
/// </summary>
internal static partial class JobObjectNativeMethods
{
    // JOBOBJECT_BASIC_LIMIT_INFORMATION.LimitFlags
    internal const uint JOB_OBJECT_LIMIT_PROCESS_MEMORY = 0x00000100;
    internal const uint JOB_OBJECT_LIMIT_JOB_MEMORY = 0x00000200;
    internal const uint JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000;

    // JOBOBJECTINFOCLASS
    internal const int JobObjectExtendedLimitInformation = 9;

    [LibraryImport("kernel32.dll", SetLastError = true, StringMarshalling = StringMarshalling.Utf16)]
    internal static partial SafeJobHandle CreateJobObjectW(IntPtr lpJobAttributes, string? lpName);

    [LibraryImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static partial bool AssignProcessToJobObject(SafeJobHandle hJob, IntPtr hProcess);

    [LibraryImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static partial bool SetInformationJobObject(
        SafeJobHandle hJob,
        int jobObjectInformationClass,
        ref JOBOBJECT_EXTENDED_LIMIT_INFORMATION lpJobObjectInformation,
        uint cbJobObjectInformationLength);

    // hJob is IntPtr (not SafeJobHandle) so callers can pass NULL = "is the process in ANY job".
    [LibraryImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static partial bool IsProcessInJob(IntPtr processHandle, IntPtr jobHandle, [MarshalAs(UnmanagedType.Bool)] out bool result);
}

[StructLayout(LayoutKind.Sequential)]
internal struct IO_COUNTERS
{
    public ulong ReadOperationCount;
    public ulong WriteOperationCount;
    public ulong OtherOperationCount;
    public ulong ReadTransferCount;
    public ulong WriteTransferCount;
    public ulong OtherTransferCount;
}

[StructLayout(LayoutKind.Sequential)]
internal struct JOBOBJECT_BASIC_LIMIT_INFORMATION
{
    public long PerProcessUserTimeLimit;
    public long PerJobUserTimeLimit;
    public uint LimitFlags;
    public nuint MinimumWorkingSetSize;
    public nuint MaximumWorkingSetSize;
    public uint ActiveProcessLimit;
    public nuint Affinity;
    public uint PriorityClass;
    public uint SchedulingClass;
}

[StructLayout(LayoutKind.Sequential)]
internal struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION
{
    public JOBOBJECT_BASIC_LIMIT_INFORMATION BasicLimitInformation;
    public IO_COUNTERS IoInfo;
    public nuint ProcessMemoryLimit;
    public nuint JobMemoryLimit;
    public nuint PeakProcessMemoryUsed;
    public nuint PeakJobMemoryUsed;
}
