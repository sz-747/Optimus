using System;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Security.Principal;
using Microsoft.Win32.SafeHandles;

namespace Optimus.Ipc;

/// <summary>
/// Windows peer identity helpers for named-pipe callers.
/// </summary>
internal static class PeerIdentity
{
    private const uint TokenQuery = 0x0008;
    private const uint ProcessQueryLimitedInformation = 0x1000;

    internal static string? GetCurrentUserSid() => WindowsIdentity.GetCurrent().User?.Value;

    internal static string? ResolveClientSid(NamedPipeServerStream serverStream)
    {
        if (!GetNamedPipeClientProcessId(serverStream.SafePipeHandle, out uint pid))
        {
            return null;
        }

        return ResolveProcessSid(pid);
    }

    private static string? ResolveProcessSid(uint pid)
    {
        IntPtr processHandle = OpenProcess(ProcessQueryLimitedInformation, false, pid);
        if (processHandle == IntPtr.Zero)
        {
            return null;
        }

        try
        {
            if (!OpenProcessToken(processHandle, TokenQuery, out IntPtr tokenHandle))
            {
                return null;
            }

            try
            {
                using WindowsIdentity identity = new(tokenHandle);
                return identity.User?.Value;
            }
            finally
            {
                _ = CloseHandle(tokenHandle);
            }
        }
        finally
        {
            _ = CloseHandle(processHandle);
        }
    }

    [DllImport("kernel32", SetLastError = true)]
    private static extern bool GetNamedPipeClientProcessId(SafePipeHandle pipe, out uint clientProcessId);

    [DllImport("kernel32", SetLastError = true)]
    private static extern IntPtr OpenProcess(uint desiredAccess, bool inheritHandle, uint processId);

    [DllImport("advapi32", SetLastError = true)]
    private static extern bool OpenProcessToken(IntPtr hProcess, uint desiredAccess, out IntPtr hToken);

    [DllImport("kernel32", SetLastError = true)]
    private static extern bool CloseHandle(IntPtr handle);
}
