//! Pseudo-terminal backend.
//!
//! Phase 1 ships the Windows ConPTY implementation (`conpty`). The engine owns the
//! ConPTY (plan §2.1): it creates the pseudoconsole, spawns the shell, runs the
//! reader loop, and feeds bytes to the VT parser. C# only forwards input + resize.

pub mod conpty;

pub use conpty::ConPty;

/// Resolve the default shell command line (plan §8 U5): prefer PowerShell 7+ (`pwsh.exe`),
/// fall back to Windows PowerShell (`powershell.exe`), then `cmd.exe`.
///
/// We search `PATH` ourselves (rather than relying on `CreateProcessW`'s implicit search)
/// so the fallback is decided *before* spawning — a failed `CreateProcessW` would otherwise
/// leak the already-created pseudoconsole/pipes on each retry.
pub fn default_shell() -> String {
    find_on_path("pwsh.exe")
        .or_else(|| find_on_path("powershell.exe"))
        // powershell.exe ships in System32 (always on PATH), so cmd is just a safety net.
        .unwrap_or_else(|| std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string()))
}

/// Find `exe` in one of the `PATH` directories, returning its full path if present.
fn find_on_path(exe: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(exe))
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().into_owned())
}
