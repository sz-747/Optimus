//! Tier-2 hard memory backstop (plan §3.3 / U4). Port of `app/Interop/JobObjectNativeMethods.cs`.
//!
//! Each ConPTY child is enrolled in an anonymous Job Object with `KILL_ON_JOB_CLOSE` — the
//! single-process-ownership win of the migration: the one Tauri process owns every job handle,
//! so its death (or an ordered teardown) makes the OS reap every shell. An optional per-process
//! memory cap (`JOB_OBJECT_LIMIT_PROCESS_MEMORY`) turns a runaway allocation into a failure
//! *inside* that terminal instead of an OOM that takes down the machine.
//!
//! Best-effort: any failure yields `None` and the terminal runs unbackstopped (today's behavior).
//! Disposal is straggler-kill, so the engine drops the job **after** `ClosePseudoConsole` — never
//! job-first, which would hard-kill a shell that was already exiting cleanly (plan §3 item 4).

use std::ffi::c_void;
use std::mem::size_of;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
    JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
};

/// An anonymous Job Object owning one ConPTY child. `Drop` closes the job handle; with
/// `KILL_ON_JOB_CLOSE` set that reaps any process still assigned.
pub struct JobObject {
    handle: HANDLE,
}

// The handle is owned exclusively by this struct on the engine worker thread.
unsafe impl Send for JobObject {}

impl JobObject {
    /// Create a job, apply `KILL_ON_JOB_CLOSE` (+ a per-process memory cap when
    /// `memory_limit_bytes > 0`), and assign `process`. Returns `None` on any failure so the
    /// caller runs unbackstopped rather than crashing (job creation is best-effort by design).
    pub fn create_for(process: HANDLE, memory_limit_bytes: usize) -> Option<Self> {
        unsafe {
            let handle = CreateJobObjectW(None, PCWSTR::null()).ok()?;

            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            let flags = if memory_limit_bytes > 0 {
                info.ProcessMemoryLimit = memory_limit_bytes;
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_PROCESS_MEMORY
            } else {
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            };
            info.BasicLimitInformation.LimitFlags = flags;

            let assigned = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const c_void,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .and_then(|()| AssignProcessToJobObject(handle, process));

            if assigned.is_err() {
                let _ = CloseHandle(handle);
                return None;
            }
            Some(Self { handle })
        }
    }
}

impl Drop for JobObject {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::io::AsRawHandle;
    use std::process::Command;
    use windows::Win32::System::JobObjects::IsProcessInJob;

    /// The child spawned into the job must report as a member. Exercises create + set-info +
    /// assign end to end; a broken FFI layout or a wrong info-class fails the assignment.
    #[test]
    fn assigns_child_into_job() {
        // A short-lived child we control. `ping -n` blocks long enough to enroll it, then exits
        // on its own (KILL_ON_JOB_CLOSE also reaps it when the job drops).
        let mut child = Command::new("cmd")
            .args(["/c", "ping", "-n", "10", "127.0.0.1"])
            .spawn()
            .expect("spawn test child");
        let process = HANDLE(child.as_raw_handle());

        let job = JobObject::create_for(process, 512 * 1024 * 1024).expect("job created");

        let mut in_job = windows::core::BOOL(0);
        // SAFETY: `process` is a live child handle; `in_job` is a valid out-param.
        unsafe { IsProcessInJob(process, Some(job.handle), &mut in_job).expect("query job") };
        assert!(in_job.as_bool(), "child should be a member of the job");

        drop(job);
        let _ = child.kill();
        let _ = child.wait();
    }
}
