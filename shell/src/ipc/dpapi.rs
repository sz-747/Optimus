//! Concrete Win32 DPAPI protector for the socket password store (plan §3.1). Port of the C#
//! `ProtectedData`-backed `SecretProtector`: `CryptProtectData`/`CryptUnprotectData` at the
//! default current-user scope, so the ciphertext is bound to the logged-in user and unreadable
//! by anyone else. `PasswordStore` (unit 3) was written protector-agnostic awaiting this.

use std::ffi::c_void;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

use crate::ipc::password::SecretProtector;

/// DPAPI-backed protector (current-user scope). `entropy`, when present, is additional secret
/// mixed into the key — protect and unprotect must use the same value.
pub struct Win32SecretProtector;

impl SecretProtector for Win32SecretProtector {
    fn protect(&self, plaintext: &[u8], entropy: Option<&[u8]>) -> Vec<u8> {
        // On failure, empty: the store treats an empty/undecryptable file as "no password".
        crypt(plaintext, entropy, Op::Protect).unwrap_or_default()
    }

    fn unprotect(&self, encrypted: &[u8], entropy: Option<&[u8]>) -> Result<Vec<u8>, ()> {
        crypt(encrypted, entropy, Op::Unprotect).ok_or(())
    }
}

enum Op {
    Protect,
    Unprotect,
}

fn crypt(input: &[u8], entropy: Option<&[u8]>, op: Op) -> Option<Vec<u8>> {
    let in_blob = blob(input);
    let mut ent_owned = entropy.map(|e| e.to_vec());
    let ent_blob = ent_owned.as_mut().map(|e| blob(e));
    let ent_ptr = ent_blob.as_ref().map(|b| b as *const CRYPT_INTEGER_BLOB);
    let mut out = CRYPT_INTEGER_BLOB::default();

    // SAFETY: all pointers are to live locals; `out.pbData` is a LocalAlloc buffer freed below.
    let result = unsafe {
        match op {
            Op::Protect => CryptProtectData(
                &in_blob,
                PCWSTR::null(),
                ent_ptr,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            ),
            Op::Unprotect => CryptUnprotectData(
                &in_blob,
                None,
                ent_ptr,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            ),
        }
    };
    result.ok()?;

    let bytes = if out.cbData == 0 || out.pbData.is_null() {
        Vec::new()
    } else {
        // SAFETY: DPAPI wrote `cbData` bytes at `pbData`.
        unsafe { std::slice::from_raw_parts(out.pbData as *const u8, out.cbData as usize).to_vec() }
    };
    // SAFETY: `pbData` came from LocalAlloc inside CryptProtect/UnprotectData.
    unsafe { let _ = LocalFree(Some(HLOCAL(out.pbData as *mut c_void))); }
    Some(bytes)
}

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_dpapi() {
        let p = Win32SecretProtector;
        let secret = b"hunter2 correct horse".to_vec();

        let sealed = p.protect(&secret, None);
        assert!(!sealed.is_empty());
        assert_ne!(sealed, secret, "ciphertext must differ from plaintext");

        assert_eq!(p.unprotect(&sealed, None).expect("unprotect"), secret);
        // Foreign/garbage ciphertext fails closed.
        assert!(p.unprotect(b"not dpapi data", None).is_err());
    }

    #[test]
    fn entropy_must_match() {
        let p = Win32SecretProtector;
        let sealed = p.protect(b"s3cr3t", Some(b"salt"));
        assert!(p.unprotect(&sealed, Some(b"salt")).is_ok());
        assert!(p.unprotect(&sealed, Some(b"wrong")).is_err());
    }
}
