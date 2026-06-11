using System;
using System.ComponentModel;

namespace Optimus.Interop;

/// <summary>
/// Tier-2 hard enforcement for the RAM safe-zone governor (plan U4): a per-terminal Job
/// Object configured with <c>JOB_OBJECT_LIMIT_PROCESS_MEMORY</c> (a runaway inside the
/// terminal gets allocation failures instead of exhausting the machine) and
/// <c>JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE</c> (disposing the job reaps the enrolled tree).
///
/// Enforcement is a best-effort backstop: every failure path returns <c>null</c>/<c>false</c>
/// and logs — it never throws — so a terminal still spawns normally when job creation or
/// assignment fails (e.g. the child is already in an unbreakable job).
/// </summary>
internal sealed class TerminalJobObject : IDisposable
{
    private readonly SafeJobHandle _job;

    private TerminalJobObject(SafeJobHandle job)
    {
        _job = job;
    }

    /// <summary>
    /// Create an anonymous Job Object limiting each enrolled process to
    /// <paramref name="processMemoryLimitBytes"/> committed bytes, with kill-on-close set.
    /// Returns <c>null</c> (logged) on any failure.
    /// </summary>
    public static TerminalJobObject? TryCreate(ulong processMemoryLimitBytes)
    {
        if (processMemoryLimitBytes == 0)
        {
            return null;
        }

        SafeJobHandle? job = null;
        try
        {
            job = JobObjectNativeMethods.CreateJobObjectW(IntPtr.Zero, null);
            if (job.IsInvalid)
            {
                App.LogError("TerminalJobObject.TryCreate", new Win32Exception());
                job.Dispose();
                return null;
            }

            var info = new JOBOBJECT_EXTENDED_LIMIT_INFORMATION();
            info.BasicLimitInformation.LimitFlags =
                JobObjectNativeMethods.JOB_OBJECT_LIMIT_PROCESS_MEMORY
                | JobObjectNativeMethods.JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            info.ProcessMemoryLimit = (nuint)processMemoryLimitBytes;

            bool ok = JobObjectNativeMethods.SetInformationJobObject(
                job,
                JobObjectNativeMethods.JobObjectExtendedLimitInformation,
                ref info,
                (uint)System.Runtime.InteropServices.Marshal.SizeOf<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>());
            if (!ok)
            {
                App.LogError("TerminalJobObject.TryCreate(SetInformationJobObject)", new Win32Exception());
                job.Dispose();
                return null;
            }

            return new TerminalJobObject(job);
        }
        catch (Exception ex)
        {
            App.LogError("TerminalJobObject.TryCreate", ex);
            job?.Dispose();
            return null;
        }
    }

    /// <summary>
    /// Enroll the process <paramref name="pid"/> (the ConPTY child) in this job.
    /// Returns <c>false</c> (logged) on any failure — the terminal then runs un-backstopped.
    /// </summary>
    public bool TryAssign(int pid)
    {
        if (pid <= 0 || _job.IsClosed || _job.IsInvalid)
        {
            return false;
        }

        IntPtr process = IntPtr.Zero;
        try
        {
            // AssignProcessToJobObject requires PROCESS_SET_QUOTA | PROCESS_TERMINATE.
            process = MemoryNativeMethods.OpenProcess(
                MemoryNativeMethods.PROCESS_SET_QUOTA | MemoryNativeMethods.PROCESS_TERMINATE,
                bInheritHandle: false,
                (uint)pid);
            if (process == IntPtr.Zero)
            {
                App.LogError("TerminalJobObject.TryAssign(OpenProcess)", new Win32Exception());
                return false;
            }

            if (!JobObjectNativeMethods.AssignProcessToJobObject(_job, process))
            {
                App.LogError("TerminalJobObject.TryAssign(AssignProcessToJobObject)", new Win32Exception());
                return false;
            }

            return true;
        }
        catch (Exception ex)
        {
            App.LogError("TerminalJobObject.TryAssign", ex);
            return false;
        }
        finally
        {
            if (process != IntPtr.Zero)
            {
                MemoryNativeMethods.CloseHandle(process);
            }
        }
    }

    /// <summary>Close the job handle; KILL_ON_JOB_CLOSE reaps any still-enrolled processes.</summary>
    public void Dispose() => _job.Dispose();
}
