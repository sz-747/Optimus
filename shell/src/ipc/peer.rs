//! Windows peer-identity + current-user SID helpers for the named-pipe server. Port of
//! `app/Ipc/PeerIdentity.cs`. Enforces the OptimusOnly/Password/Automation peer-SID check
//! (AllowAll skips it): a caller is authorized iff its process-token user SID matches ours.

use std::ffi::c_void;

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TokenUser, PSID, TOKEN_QUERY, TOKEN_USER};
use windows::Win32::System::Pipes::GetNamedPipeClientProcessId;
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// SDDL string SID of the current process's user (e.g. `S-1-5-21-…`), or `None` if unresolvable.
pub fn current_user_sid() -> Option<String> {
    // SAFETY: GetCurrentProcess returns a pseudo-handle (needs no close); the token is closed inside.
    unsafe { sid_of_process(GetCurrentProcess()) }
}

/// SDDL string SID of the pipe client's process user, or `None` if it can't be resolved.
pub fn resolve_client_sid(pipe: HANDLE) -> Option<String> {
    let mut pid: u32 = 0;
    // SAFETY: `pipe` is a connected named-pipe server handle.
    unsafe { GetNamedPipeClientProcessId(pipe, &mut pid).ok()? };
    if pid == 0 {
        return None;
    }
    // SAFETY: the opened process handle is closed before returning.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let sid = sid_of_process(process);
        let _ = CloseHandle(process);
        sid
    }
}

/// User SID string of a process from its handle. Does not close `process`.
unsafe fn sid_of_process(process: HANDLE) -> Option<String> {
    let mut token = HANDLE::default();
    OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
    let sid = token_user_sid(token);
    let _ = CloseHandle(token);
    sid
}

unsafe fn token_user_sid(token: HANDLE) -> Option<String> {
    // First call sizes the buffer (returns ERROR_INSUFFICIENT_BUFFER — ignored).
    let mut needed: u32 = 0;
    let _ = GetTokenInformation(token, TokenUser, None, 0, &mut needed);
    if needed == 0 {
        return None;
    }
    let mut buf = vec![0u8; needed as usize];
    GetTokenInformation(
        token,
        TokenUser,
        Some(buf.as_mut_ptr() as *mut c_void),
        needed,
        &mut needed,
    )
    .ok()?;
    let user = &*(buf.as_ptr() as *const TOKEN_USER);
    sid_to_string(user.User.Sid)
}

unsafe fn sid_to_string(sid: PSID) -> Option<String> {
    let mut wide = PWSTR::null();
    ConvertSidToStringSidW(sid, &mut wide).ok()?;
    if wide.is_null() {
        return None;
    }
    let s = wide.to_string().ok();
    let _ = LocalFree(Some(HLOCAL(wide.0 as *mut c_void)));
    s
}
