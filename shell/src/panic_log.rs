//! Crash forensics: a process-wide panic hook that appends every panic to a single log file
//! under `%LOCALAPPDATA%\optimus\logs\panic.log` (C4, port of the C# `install_render_panic_logger`).
//!
//! The HARD requirement is "nothing crashes after migration" — when something does anyway, this is
//! the black box. The hook chains to the previously installed hook (so debug builds still print to
//! the console) and is strictly best-effort: a panic handler that itself panics would abort the
//! process, so every step here swallows its own errors.

use std::backtrace::Backtrace;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::panic;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// The log file: `%LOCALAPPDATA%\optimus\logs\panic.log`. `None` when `%LOCALAPPDATA%` is unset
/// (unreachable on a real Windows session) — the hook then just chains to the default.
fn log_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(
        PathBuf::from(base)
            .join("optimus")
            .join("logs")
            .join("panic.log"),
    )
}

/// Install the panic hook. Call once, first thing in `main`, before any thread is spawned.
pub fn install() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        write_record(info); // best-effort; never unwinds
        previous(info); // keep the default behaviour (console print / abort semantics)
    }));
}

/// Append one panic record. Every fallible step is swallowed — a failing panic logger must not
/// escalate a recoverable panic into an abort.
fn write_record(info: &panic::PanicHookInfo<'_>) {
    let Some(path) = log_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }

    let thread = std::thread::current();
    let name = thread.name().unwrap_or("<unnamed>");
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown>".into());
    let backtrace = Backtrace::force_capture();

    let record = format!(
        "\n===== panic @ {}s =====\nthread : {name}\nat     : {location}\npayload: {}\n{backtrace}\n",
        unix_seconds(),
        payload_str(info),
    );

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = file.write_all(record.as_bytes());
    }
}

/// The panic payload as text — panics carry `&str` or `String`; anything else is opaque.
fn payload_str(info: &panic::PanicHookInfo<'_>) -> String {
    let p = info.payload();
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".into()
    }
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
