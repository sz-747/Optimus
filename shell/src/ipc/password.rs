//! Port of core/Ipc/PasswordStore.cs — the socket password source + verifier for Password mode.
//! Source precedence: (1) `OPTIMUS_SOCKET_PASSWORD` env var, (2) a DPAPI-protected file under
//! `%LOCALAPPDATA%\optimus\`. The verify path hashes both sides with SHA-256 and compares in
//! constant time (no length/timing oracle), matching the C# `FixedTimeEquals` discipline.
//!
//! The concrete DPAPI protector (CryptProtectData/CryptUnprotectData) lands with the pipe server
//! (P3 unit 5) once the `windows` crate is a dependency; the store is protector-agnostic so it and
//! its tests stand alone here.

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::ipc::access::PASSWORD_ENV;

const PASSWORD_FILE_NAME: &str = "optimus-socket-password.bin";

/// Pluggable secret protect/deprotect so the store is testable without DPAPI. `unprotect` returns
/// `Err` on any failure (tampered/foreign ciphertext); the store treats that as "no password".
pub trait SecretProtector {
    fn protect(&self, plaintext: &[u8], entropy: Option<&[u8]>) -> Vec<u8>;
    // The failure is deliberately opaque: any bad ciphertext (tampered, foreign, wrong entropy)
    // means "no usable password" — the caller never branches on why, so a custom error would be
    // ceremony. ponytail: promote to a real error type if a caller ever needs the reason.
    #[allow(clippy::result_unit_err)]
    fn unprotect(&self, encrypted: &[u8], entropy: Option<&[u8]>) -> Result<Vec<u8>, ()>;
}

/// Identity protector — the injectable default for tests (the C# `NoopSecretProtector`).
pub struct NoopSecretProtector;

impl SecretProtector for NoopSecretProtector {
    fn protect(&self, plaintext: &[u8], _entropy: Option<&[u8]>) -> Vec<u8> {
        plaintext.to_vec()
    }
    fn unprotect(&self, encrypted: &[u8], _entropy: Option<&[u8]>) -> Result<Vec<u8>, ()> {
        Ok(encrypted.to_vec())
    }
}

/// Password source + verifier for the named-pipe socket. All I/O is injected (env, local-app-data
/// path, existence probe, reader) so behavior is deterministic under test.
// The injected-I/O closures are a test seam, not a public type; aliasing each `Box<dyn Fn…>`
// would add names no caller uses. The one constructor already carries the same allow.
#[allow(clippy::type_complexity)]
pub struct PasswordStore {
    protector: Box<dyn SecretProtector>,
    get_env: Box<dyn Fn(&str) -> Option<String>>,
    get_local_app_data: Box<dyn Fn() -> String>,
    file_exists: Box<dyn Fn(&str) -> bool>,
    read_file: Box<dyn Fn(&str) -> std::io::Result<Vec<u8>>>,
}

impl PasswordStore {
    #[allow(clippy::type_complexity)]
    pub fn new(
        protector: Box<dyn SecretProtector>,
        get_env: Box<dyn Fn(&str) -> Option<String>>,
        get_local_app_data: Box<dyn Fn() -> String>,
        file_exists: Box<dyn Fn(&str) -> bool>,
        read_file: Box<dyn Fn(&str) -> std::io::Result<Vec<u8>>>,
    ) -> Self {
        Self {
            protector,
            get_env,
            get_local_app_data,
            file_exists,
            read_file,
        }
    }

    /// True when `credential` matches the stored password (constant-time over SHA-256 digests).
    pub fn verify(&self, credential: &str) -> bool {
        match self.read_stored_password() {
            Some(expected) if !expected.is_empty() => constant_time_equals(credential, &expected),
            _ => false,
        }
    }

    /// The active password: env var (verbatim, untrimmed) if set non-whitespace, else the
    /// DPAPI file's decrypted+trimmed contents, or `None`.
    pub fn read_stored_password(&self) -> Option<String> {
        if let Some(env_password) = (self.get_env)(PASSWORD_ENV).filter(|v| !v.trim().is_empty()) {
            return Some(env_password);
        }

        let file = self.password_file_path();
        if !(self.file_exists)(&file) {
            return None;
        }

        // Any failure across read → unprotect → decode collapses to "no password" (C# catch-all).
        let encrypted = (self.read_file)(&file).ok()?;
        let plain = self.protector.unprotect(&encrypted, None).ok()?;
        let text = String::from_utf8(plain).ok()?;
        Some(text.trim().to_string())
    }

    fn password_file_path(&self) -> String {
        let base = (self.get_local_app_data)();
        std::path::Path::new(&base)
            .join("optimus")
            .join(PASSWORD_FILE_NAME)
            .to_string_lossy()
            .into_owned()
    }
}

/// Compare two secrets in constant time: SHA-256 each, then fixed-time-compare the 32-byte digests
/// (independent of input length; equal only when the digests match).
fn constant_time_equals(left: &str, right: &str) -> bool {
    let left_hash = Sha256::digest(left.as_bytes());
    let right_hash = Sha256::digest(right.as_bytes());
    left_hash.ct_eq(&right_hash).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    /// Tracks whether `unprotect` ran, so tests can assert the env fast-path skips the file.
    struct TrackingSecretProtector {
        unprotect_called: Rc<Cell<bool>>,
    }

    impl SecretProtector for TrackingSecretProtector {
        fn protect(&self, plaintext: &[u8], _entropy: Option<&[u8]>) -> Vec<u8> {
            plaintext.to_vec()
        }
        fn unprotect(&self, encrypted: &[u8], _entropy: Option<&[u8]>) -> Result<Vec<u8>, ()> {
            self.unprotect_called.set(true);
            Ok(encrypted.to_vec())
        }
    }

    #[test]
    fn environment_password_takes_precedence_over_file() {
        let called = Rc::new(Cell::new(false));
        let store = PasswordStore::new(
            Box::new(TrackingSecretProtector {
                unprotect_called: called.clone(),
            }),
            Box::new(|_| Some(" env-pass ".to_string())),
            Box::new(|| r"C:\users\app\Local".to_string()),
            Box::new(|_| true),
            Box::new(|_| Ok(Vec::new())),
        );

        assert!(store.verify(" env-pass "));
        assert!(!called.get());
    }

    #[test]
    fn stored_file_password_is_verified_and_used() {
        let called = Rc::new(Cell::new(false));
        let expected = "from-file";
        let store = PasswordStore::new(
            Box::new(TrackingSecretProtector {
                unprotect_called: called.clone(),
            }),
            Box::new(|_| None),
            Box::new(|| r"C:\users\app\Local".to_string()),
            Box::new(|path| path == r"C:\users\app\Local\optimus\optimus-socket-password.bin"),
            Box::new(move |_| Ok(expected.as_bytes().to_vec())),
        );

        assert!(store.verify(expected));
        assert!(!store.verify("bad"));
        assert!(called.get());
    }

    #[test]
    fn verify_is_false_when_no_password_is_available() {
        let called = Rc::new(Cell::new(false));
        let store = PasswordStore::new(
            Box::new(TrackingSecretProtector {
                unprotect_called: called,
            }),
            Box::new(|_| None),
            Box::new(|| r"C:\users\app\Local".to_string()),
            Box::new(|_| false),
            Box::new(|_| Ok(Vec::new())),
        );

        assert!(!store.verify("anything"));
    }

    #[test]
    fn constant_time_equals_matches_only_identical_secrets() {
        assert!(constant_time_equals("hunter2", "hunter2"));
        assert!(!constant_time_equals("hunter2", "hunter3"));
        assert!(!constant_time_equals("", "x"));
    }
}
