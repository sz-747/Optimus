//! Windows ConPTY (pseudoconsole) wrapper — plan §7.2 (spike 2) / §8 U5.
//!
//! Sequence (per Microsoft's ConPTY guide):
//!   1. Create two anonymous pipes: one for shell *input*, one for shell *output*.
//!   2. `CreatePseudoConsole` with the input-read + output-write ends.
//!   3. Build a `STARTUPINFOEXW` whose attribute list carries
//!      `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE` → the HPCON.
//!   4. `CreateProcessW` with `EXTENDED_STARTUPINFO_PRESENT`.
//!   5. Close the pseudoconsole's copies of the child-side handles in the parent.
//!   6. Read VT bytes from the output-read end on a dedicated thread; write
//!      keystrokes to the input-write end.
//!
//! Shutdown ordering matters: `ClosePseudoConsole` signals the child, then the
//! reader must drain `output_read` to EOF before handles are closed — not draining
//! can deadlock (plan §7.2).

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{Result, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_BROKEN_PIPE, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows::Win32::System::Console::{
    ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
    InitializeProcThreadAttributeList, UpdateProcThreadAttribute, EXTENDED_STARTUPINFO_PRESENT,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

/// `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE`. Defined locally (value is stable across SDKs)
/// so we don't depend on whether the `windows` crate surfaces this particular constant.
const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x0002_0016;

/// A live pseudoconsole + the spawned shell process.
///
/// Owns every handle it creates and releases them in `Drop`. The output-read end is
/// handed to a reader thread via [`ConPty::output_reader`]; that reader borrows the
/// handle (does not own/close it), so the reader thread must be joined before the
/// `ConPty` is dropped.
pub struct ConPty {
    hpc: HPCON,
    /// We write shell input here.
    input_write: HANDLE,
    /// We read shell (VT) output here.
    output_read: HANDLE,
    proc_info: PROCESS_INFORMATION,
    /// Backing storage for the proc-thread attribute list; must outlive `CreateProcessW`
    /// and stay alive until `DeleteProcThreadAttributeList`.
    attr_list: Vec<u8>,
    /// Ensures `ClosePseudoConsole` runs exactly once (from `shutdown` or `Drop`).
    pseudoconsole_closed: AtomicBool,
}

// The handles are owned exclusively by this struct on a single logical PTY; the only
// cross-thread sharing is the read end via the explicit `OutputReader` Send wrapper.
unsafe impl Send for ConPty {}

impl ConPty {
    /// Create a pseudoconsole of `cols`×`rows` and spawn `cmdline` inside it.
    ///
    /// `cwd` is the working directory (None → inherit the parent's).
    pub fn spawn(cmdline: &str, cwd: Option<&str>, cols: u16, rows: u16) -> Result<Self> {
        unsafe {
            // 1. Pipes. CreatePipe gives (read, write). Defaults are non-inheritable,
            //    which is what ConPTY wants (handles flow via the pseudoconsole attribute).
            let mut input_read = HANDLE::default();
            let mut input_write = HANDLE::default();
            CreatePipe(&mut input_read, &mut input_write, None, 0)?;

            let mut output_read = HANDLE::default();
            let mut output_write = HANDLE::default();
            CreatePipe(&mut output_read, &mut output_write, None, 0)?;

            // 2. Pseudoconsole over the child-facing ends.
            let size = COORD {
                X: cols as i16,
                Y: rows as i16,
            };
            let hpc = CreatePseudoConsole(size, input_read, output_write, 0)?;

            // NB: do NOT close `input_read` / `output_write` yet. conhost is connected to the
            // pseudoconsole when `CreateProcessW` consumes the PSEUDOCONSOLE attribute, and it
            // duplicates the child-facing handles at that point. Closing them earlier leaves the
            // output pipe with no write end → the reader sees EOF immediately. We close our copies
            // *after* CreateProcessW (below).

            // 3. Attribute list carrying the HPCON.
            let mut attr_size: usize = 0;
            // First call returns ERROR_INSUFFICIENT_BUFFER and fills `attr_size`; ignore the Err.
            let _ = InitializeProcThreadAttributeList(None, 1, None, &mut attr_size);
            let mut attr_list = vec![0u8; attr_size];
            let attr_ptr = LPPROC_THREAD_ATTRIBUTE_LIST(attr_list.as_mut_ptr() as *mut c_void);
            InitializeProcThreadAttributeList(Some(attr_ptr), 1, None, &mut attr_size)?;

            // For PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, `lpValue` is the HPCON *value* itself
            // (not the address of an HPCON variable). Microsoft's EchoCon sample passes `hpc`
            // directly with `sizeof(hpc)`. Passing `&hpc` instead makes the kernel treat our
            // stack address as the pseudoconsole, so the child's console init fails with
            // STATUS_DLL_INIT_FAILED (0xC0000142).
            UpdateProcThreadAttribute(
                attr_ptr,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                Some(hpc.0 as *const c_void),
                size_of::<HPCON>(),
                None,
                None,
            )?;

            // 4. STARTUPINFOEXW + CreateProcessW.
            let mut si = STARTUPINFOEXW::default();
            si.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
            si.lpAttributeList = attr_ptr;
            // Detach the child's stdio from the parent's console. When the host process itself
            // owns a console (e.g. a `cargo test` runner), a console child would otherwise attach
            // to *that* console instead of our pseudoconsole — its output would bypass our pipe.
            // Setting STARTF_USESTDHANDLES with INVALID handles forces it onto the pseudoconsole.
            // (Real GUI hosts have no console, but this keeps the engine correct either way.)
            si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            si.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
            si.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
            si.StartupInfo.hStdError = INVALID_HANDLE_VALUE;

            let mut cmdline_w = to_wide_null(cmdline);
            let cwd_w = cwd.map(to_wide_null);
            let cwd_pcwstr = match &cwd_w {
                Some(v) => PCWSTR(v.as_ptr()),
                None => PCWSTR::null(),
            };

            let mut proc_info = PROCESS_INFORMATION::default();
            CreateProcessW(
                PCWSTR::null(),
                Some(PWSTR(cmdline_w.as_mut_ptr())),
                None,
                None,
                false,
                EXTENDED_STARTUPINFO_PRESENT,
                None,
                cwd_pcwstr,
                &si.StartupInfo,
                &mut proc_info,
            )?;

            // conhost is now connected and owns its duplicates of the child-facing ends.
            // Release our copies so the only remaining output-write handle belongs to conhost
            // (required for the read end to reach EOF when the pseudoconsole is closed).
            let _ = CloseHandle(input_read);
            let _ = CloseHandle(output_write);

            Ok(ConPty {
                hpc,
                input_write,
                output_read,
                proc_info,
                attr_list,
                pseudoconsole_closed: AtomicBool::new(false),
            })
        }
    }

    /// Write bytes to the shell's input (keystrokes / paste). Loops until all written.
    pub fn write(&self, mut data: &[u8]) -> Result<()> {
        while !data.is_empty() {
            let mut written: u32 = 0;
            unsafe { WriteFile(self.input_write, Some(data), Some(&mut written), None)? };
            data = &data[written as usize..];
        }
        Ok(())
    }

    /// The Windows process id of the spawned shell (the ConPTY child). Used by the host to
    /// enroll the child in a per-terminal Job Object (RAM safe-zone plan U4).
    pub fn child_pid(&self) -> u32 {
        self.proc_info.dwProcessId
    }

    /// Whether the spawned shell process is still running. The engine uses this to detect
    /// the shell exiting (so it can close the pane); `STILL_ACTIVE` (259) means running.
    pub fn child_alive(&self) -> bool {
        const STILL_ACTIVE: u32 = 259;
        let mut code: u32 = 0;
        unsafe {
            GetExitCodeProcess(self.proc_info.hProcess, &mut code).is_ok() && code == STILL_ACTIVE
        }
    }

    /// The shell's exit code, or `None` if it is still running (or unqueryable).
    pub fn exit_code(&self) -> Option<i32> {
        const STILL_ACTIVE: u32 = 259;
        let mut code: u32 = 0;
        unsafe { GetExitCodeProcess(self.proc_info.hProcess, &mut code).ok()? };
        if code == STILL_ACTIVE {
            None
        } else {
            Some(code as i32)
        }
    }

    /// Resize the pseudoconsole. Forwarded from the C# resize/DPI loop.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let size = COORD {
            X: cols as i16,
            Y: rows as i16,
        };
        unsafe { ResizePseudoConsole(self.hpc, size) }
    }

    /// A Send handle to the output-read end for the reader thread.
    ///
    /// The returned reader *borrows* the handle; it must stop (be dropped/joined)
    /// before this `ConPty` is dropped, which closes the underlying handle.
    pub fn output_reader(&self) -> OutputReader {
        OutputReader {
            handle: self.output_read,
        }
    }

    /// A `Write` adapter over the shell's input pipe, for handing to `wezterm-term`'s
    /// `Terminal` (so the VT core's own replies — DA/cursor reports — reach the shell).
    ///
    /// The adapter *borrows* the input handle (does not own/close it); it must be dropped
    /// before this `ConPty` is dropped. Both live on the render thread, so declaring the
    /// `Terminal` before the `ConPty` in the owning struct gives the correct drop order.
    pub fn input_writer(&self) -> PtyInput {
        PtyInput {
            handle: self.input_write,
        }
    }

    /// Begin teardown: close the pseudoconsole.
    ///
    /// This terminates the shell and closes conhost's output-pipe write end, which is
    /// what makes a blocked reader `ReadFile` return EOF. **Call this before joining the
    /// reader thread** — the reader cannot reach EOF until the pseudoconsole is closed,
    /// so joining first deadlocks (plan §7.2). Idempotent; `Drop` calls it again harmlessly.
    pub fn shutdown(&self) {
        if !self.pseudoconsole_closed.swap(true, Ordering::SeqCst) {
            unsafe { ClosePseudoConsole(self.hpc) };
        }
    }
}

impl Drop for ConPty {
    fn drop(&mut self) {
        // Ensure the pseudoconsole is closed (no-op if `shutdown` already ran). The reader
        // thread must already be joined by now — its borrowed output handle is closed below.
        self.shutdown();
        unsafe {
            let _ = CloseHandle(self.input_write);
            let _ = CloseHandle(self.output_read);
            if !self.proc_info.hProcess.is_invalid() {
                let _ = CloseHandle(self.proc_info.hProcess);
            }
            if !self.proc_info.hThread.is_invalid() {
                let _ = CloseHandle(self.proc_info.hThread);
            }
            if !self.attr_list.is_empty() {
                DeleteProcThreadAttributeList(LPPROC_THREAD_ATTRIBUTE_LIST(
                    self.attr_list.as_mut_ptr() as *mut c_void,
                ));
            }
        }
    }
}

/// A `Send` view of the PTY output-read handle for the reader thread.
///
/// Does not own the handle — the parent [`ConPty`] closes it on drop.
pub struct OutputReader {
    handle: HANDLE,
}

// The raw handle is just an integer; we move it to the reader thread deliberately and
// guarantee (by joining) the reader stops before the owning ConPty closes it.
unsafe impl Send for OutputReader {}

impl OutputReader {
    /// Blocking read of VT bytes. Returns `Ok(0)` at EOF (pseudoconsole closed / shell exited).
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let mut read: u32 = 0;
        match unsafe { ReadFile(self.handle, Some(buf), Some(&mut read), None) } {
            Ok(()) => Ok(read as usize),
            // A closed pseudoconsole surfaces as a broken pipe — that's our EOF.
            Err(e) if e.code() == ERROR_BROKEN_PIPE.to_hresult() => Ok(0),
            Err(e) => Err(e),
        }
    }
}

/// A `Send`, non-owning `std::io::Write` over the PTY input-write handle.
///
/// Handed to `wezterm-term`'s `Terminal` as its writer. Does not own the handle — the
/// parent [`ConPty`] closes it on drop.
pub struct PtyInput {
    handle: HANDLE,
}

// The raw handle is just an integer; writes are serialized on the render thread.
unsafe impl Send for PtyInput {}

impl std::io::Write for PtyInput {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut written: u32 = 0;
        unsafe { WriteFile(self.handle, Some(buf), Some(&mut written), None) }
            .map_err(std::io::Error::other)?;
        Ok(written as usize)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
