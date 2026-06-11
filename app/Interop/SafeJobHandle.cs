using Microsoft.Win32.SafeHandles;

namespace Optimus.Interop;

/// <summary>
/// Owning wrapper over a Job Object handle. With JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE set,
/// closing the handle kills every enrolled terminal — so the handle MUST be a SafeHandle
/// rooted by the owning pane, never a raw IntPtr a GC race could finalize early.
/// </summary>
internal sealed class SafeJobHandle : SafeHandleZeroOrMinusOneIsInvalid
{
    /// <summary>Required by LibraryImport SafeHandle marshalling.</summary>
    public SafeJobHandle() : base(ownsHandle: true)
    {
    }

    protected override bool ReleaseHandle() => MemoryNativeMethods.CloseHandle(handle);
}
