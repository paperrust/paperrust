//! Secrets (device token) in the OS keychain (docs/SYNC.md §8, docs/REMOTE-APP.md §2).
//!
//! * Windows: Credential Manager, generic entry
//!   `PaperRust/<profile-id>`, `CRED_PERSIST_LOCAL_MACHINE` (this user only, not roaming).
//! * other systems (until keyring/Secret Service is integrated) and tests: [`MemoryStore`] –
//!   token only in memory, new login on each start (as provided in SYNC.md §8).
//!
//! The token is never written to disk, never logged and never given to the WebView.

use std::collections::HashMap;
use std::sync::Mutex;

use zeroize::Zeroizing;

use crate::error::{Error, Result};

/// Prefix of the keychain entry.
pub const SERVICE: &str = "PaperRust";

/// Name of the entry for a profile.
pub fn target_for(profile_id: &str) -> String {
    format!("{SERVICE}/{profile_id}")
}

/// Storage for secrets.
pub trait SecretStore: Send + Sync {
    fn get(&self, target: &str) -> Result<Option<Zeroizing<String>>>;
    fn set(&self, target: &str, secret: &str) -> Result<()>;
    /// Delete; a missing entry is not an error.
    fn delete(&self, target: &str) -> Result<()>;
    /// Permanent (keychain) or memory only.
    fn persistent(&self) -> bool;
}

/// Memory only.
#[derive(Default)]
pub struct MemoryStore {
    map: Mutex<HashMap<String, Zeroizing<String>>>,
}

impl SecretStore for MemoryStore {
    fn get(&self, target: &str) -> Result<Option<Zeroizing<String>>> {
        let m = self.map.lock().map_err(|_| Error::Secret("poisoned".into()))?;
        Ok(m.get(target).cloned())
    }

    fn set(&self, target: &str, secret: &str) -> Result<()> {
        let mut m = self.map.lock().map_err(|_| Error::Secret("poisoned".into()))?;
        m.insert(target.to_string(), Zeroizing::new(secret.to_string()));
        Ok(())
    }

    fn delete(&self, target: &str) -> Result<()> {
        let mut m = self.map.lock().map_err(|_| Error::Secret("poisoned".into()))?;
        m.remove(target);
        Ok(())
    }

    fn persistent(&self) -> bool {
        false
    }
}

/// Best available storage of this system.
pub fn os_store() -> Box<dyn SecretStore> {
    #[cfg(windows)]
    {
        Box::new(windows::CredentialStore)
    }
    #[cfg(not(windows))]
    {
        Box::new(MemoryStore::default())
    }
}

#[cfg(windows)]
pub mod windows {
    //! Windows Credential Manager via `advapi32` (CredWriteW/CredReadW/CredDeleteW).

    use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, FILETIME, GetLastError};
    use windows_sys::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW, CredWriteW,
    };
    use zeroize::{Zeroize, Zeroizing};

    use super::SecretStore;
    use crate::error::{Error, Result};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Generic entries of the logged-in Windows user.
    pub struct CredentialStore;

    impl SecretStore for CredentialStore {
        fn get(&self, target: &str) -> Result<Option<Zeroizing<String>>> {
            let t = wide(target);
            let mut p: *mut CREDENTIALW = std::ptr::null_mut();
            // SAFETY: valid, null-terminated name; `p` is only read on success and released with
            // CredFree.
            let ok = unsafe { CredReadW(t.as_ptr(), CRED_TYPE_GENERIC, 0, &mut p) };
            if ok == 0 {
                // SAFETY: simple call without arguments.
                let err = unsafe { GetLastError() };
                return if err == ERROR_NOT_FOUND { Ok(None) } else { Err(Error::Secret(format!("CredReadW failed ({err})"))) };
            }
            // SAFETY: CredReadW returned a valid pointer; the blob has CredentialBlobSize bytes.
            let bytes = unsafe {
                let c = &*p;
                let mut v = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec();
                // overwrite the blob in the system buffer before freeing
                std::ptr::write_bytes(c.CredentialBlob, 0, c.CredentialBlobSize as usize);
                CredFree(p as *const _);
                let s = String::from_utf8(v.clone());
                v.zeroize();
                s
            };
            match bytes {
                Ok(s) => Ok(Some(Zeroizing::new(s))),
                Err(_) => Err(Error::Secret("stored secret is not UTF-8".into())),
            }
        }

        fn set(&self, target: &str, secret: &str) -> Result<()> {
            let mut t = wide(target);
            let mut user = wide("paperrust");
            let mut blob = Zeroizing::new(secret.as_bytes().to_vec());
            let cred = CREDENTIALW {
                Flags: 0,
                Type: CRED_TYPE_GENERIC,
                TargetName: t.as_mut_ptr(),
                Comment: std::ptr::null_mut(),
                LastWritten: FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 },
                CredentialBlobSize: blob.len() as u32,
                CredentialBlob: blob.as_mut_ptr(),
                Persist: CRED_PERSIST_LOCAL_MACHINE,
                AttributeCount: 0,
                Attributes: std::ptr::null_mut(),
                TargetAlias: std::ptr::null_mut(),
                UserName: user.as_mut_ptr(),
            };
            // SAFETY: all pointers point to live buffers of this function.
            let ok = unsafe { CredWriteW(&cred, 0) };
            if ok == 0 {
                // SAFETY: simple call without arguments.
                let err = unsafe { GetLastError() };
                return Err(Error::Secret(format!("CredWriteW failed ({err})")));
            }
            Ok(())
        }

        fn delete(&self, target: &str) -> Result<()> {
            let t = wide(target);
            // SAFETY: valid, null-terminated name.
            let ok = unsafe { CredDeleteW(t.as_ptr(), CRED_TYPE_GENERIC, 0) };
            if ok == 0 {
                // SAFETY: simple call without arguments.
                let err = unsafe { GetLastError() };
                if err != ERROR_NOT_FOUND {
                    return Err(Error::Secret(format!("CredDeleteW failed ({err})")));
                }
            }
            Ok(())
        }

        fn persistent(&self) -> bool {
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_roundtrip() {
        let s = MemoryStore::default();
        assert!(s.get("a").unwrap().is_none());
        s.set("a", "prt_x").unwrap();
        assert_eq!(s.get("a").unwrap().unwrap().as_str(), "prt_x");
        s.delete("a").unwrap();
        s.delete("a").unwrap();
        assert!(s.get("a").unwrap().is_none());
    }

    /// Real Windows keychain with a throwaway entry (`PaperRust-test/<uuid>`) that is deleted at the
    /// end.
    #[cfg(windows)]
    #[test]
    fn windows_credential_roundtrip() {
        let s = windows::CredentialStore;
        let target = format!("PaperRust-test/{}", uuid::Uuid::new_v4());
        assert!(s.get(&target).unwrap().is_none());
        s.set(&target, "prt_testtoken_ÄÖ").unwrap();
        assert_eq!(s.get(&target).unwrap().unwrap().as_str(), "prt_testtoken_ÄÖ");
        s.set(&target, "prt_second").unwrap();
        assert_eq!(s.get(&target).unwrap().unwrap().as_str(), "prt_second");
        s.delete(&target).unwrap();
        assert!(s.get(&target).unwrap().is_none());
        s.delete(&target).unwrap();
    }
}
