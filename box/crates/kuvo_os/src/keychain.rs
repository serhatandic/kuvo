//! Cache a Keychain item's modification date, never its password.
//! The search list and store metadata are the invalidation boundary: a
//! different account may edit the same store, so invalidate conservatively.
use std::time::SystemTime;

#[derive(Default)]
pub struct CredentialDate {
    #[cfg(any(target_os = "macos", test))]
    kept: std::sync::Mutex<Option<Kept>>,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, PartialEq, Eq)]
struct Store {
    path: std::path::PathBuf,
    status: u32,
    files: Vec<Option<FileStamp>>,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed: (i64, i64),
}

#[cfg(any(target_os = "macos", test))]
impl Store {
    fn new(path: std::path::PathBuf, status: u32) -> Self {
        use std::fs;
        let files = ["", "-wal", "-shm", "-journal"]
            .into_iter()
            .map(|suffix| {
                let mut file = path.as_os_str().to_owned();
                file.push(suffix);
                let meta = fs::metadata(file).ok()?;
                #[cfg(unix)]
                use std::os::unix::fs::MetadataExt;
                Some(FileStamp {
                    len: meta.len(),
                    modified: meta.modified().ok(),
                    created: meta.created().ok(),
                    #[cfg(unix)]
                    inode: meta.ino(),
                    #[cfg(unix)]
                    changed: (meta.ctime(), meta.ctime_nsec()),
                })
            })
            .collect();
        Self {
            path,
            status,
            files,
        }
    }
}

#[cfg(any(target_os = "macos", test))]
struct Kept {
    service: String,
    stores: Vec<Store>,
    date: Option<SystemTime>,
}

impl CredentialDate {
    /// Explicit login, logout or observed revocation must bypass the retained date.
    pub fn invalidate(&self) {
        #[cfg(any(target_os = "macos", test))]
        {
            *self.kept.lock().unwrap_or_else(|p| p.into_inner()) = None;
        }
    }

    pub fn written(&self, service: &str) -> Option<SystemTime> {
        #[cfg(target_os = "macos")]
        {
            // Scratch boxes and this crate’s tests must never consult the
            // actual login Keychain, even if a caller forgot its own guard.
            if cfg!(test)
                || std::env::var_os("KUVO_TEST_PATH_ONLY").as_deref()
                    == Some(std::ffi::OsStr::new("1"))
            {
                return None;
            }
            self.read(service, mac::stores, mac::written)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = service;
            None
        }
    }

    #[cfg(any(target_os = "macos", test))]
    fn read(
        &self,
        service: &str,
        stores: impl Fn() -> Option<Vec<Store>>,
        lookup: impl FnOnce(&str) -> std::io::Result<Option<SystemTime>>,
    ) -> Option<SystemTime> {
        let mut kept = self.kept.lock().unwrap_or_else(|p| p.into_inner());
        let before = stores();
        if let (Some(previous), Some(current)) = (kept.as_ref(), before.as_ref())
            && previous.service == service
            && &previous.stores == current
        {
            return previous.date;
        }
        // A failed search-list query or transient item lookup must not turn
        // into a durable missing sign-in. Nor cache a read racing a store edit.
        *kept = None;
        let date = lookup(service).ok()?;
        if let Some(before) = before
            && stores().as_ref() == Some(&before)
        {
            *kept = Some(Kept {
                service: service.into(),
                stores: before,
                date,
            });
        }
        date
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::Store;
    use std::{
        ffi::{OsString, c_char, c_void},
        os::unix::ffi::OsStringExt,
        path::PathBuf,
        ptr,
        time::SystemTime,
    };

    type Ref = *const c_void;

    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SecKeychainCopySearchList(list: *mut Ref) -> i32;
        fn SecKeychainGetPath(keychain: Ref, length: *mut u32, path: *mut c_char) -> i32;
        fn SecKeychainGetStatus(keychain: Ref, status: *mut u32) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFArrayGetCount(array: Ref) -> isize;
        fn CFArrayGetValueAtIndex(array: Ref, index: isize) -> Ref;
        fn CFRelease(object: Ref);
    }

    struct SearchList(Ref);
    impl Drop for SearchList {
        fn drop(&mut self) {
            // SAFETY: successful CopySearchList returned our retained CFArray.
            unsafe { CFRelease(self.0) };
        }
    }

    /// Read only the actual search list, paths, status and file metadata.
    /// Custom keychains and a search-list reorder are part of the cache key.
    pub(super) fn stores() -> Option<Vec<Store>> {
        let mut list = ptr::null();
        // SAFETY: the out pointer is valid and initialized to null.
        if unsafe { SecKeychainCopySearchList(&mut list) } != 0 || list.is_null() {
            return None;
        }
        let list = SearchList(list);
        // SAFETY: list is a retained, valid CFArray of SecKeychain references.
        let count = unsafe { CFArrayGetCount(list.0) };
        let mut out = Vec::new();
        for index in 0..count {
            // SAFETY: index lies inside the retained array.
            let keychain = unsafe { CFArrayGetValueAtIndex(list.0, index) };
            let mut path = vec![0u8; 4096];
            let mut len = path.len() as u32;
            let mut status = 0;
            // SAFETY: keychain is retained by list; both output buffers are valid
            // for their specified sizes.
            if unsafe {
                SecKeychainGetPath(keychain, &mut len, path.as_mut_ptr().cast()) != 0
                    || SecKeychainGetStatus(keychain, &mut status) != 0
            } || len as usize > path.len()
            {
                return None;
            }
            path.truncate(len as usize);
            let store = Store::new(PathBuf::from(OsString::from_vec(path)), status);
            // If metadata isn't visible, we cannot prove the date is still current.
            if store.files.first().is_none_or(Option::is_none) {
                return None;
            }
            out.push(store);
        }
        Some(out)
    }

    /// Keep the existing security command's metadata semantics, without
    /// launching it for an unchanged search list and store generation.
    pub(super) fn written(service: &str) -> std::io::Result<Option<SystemTime>> {
        let output = std::process::Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", service])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()?;
        if !output.status.success() {
            // errSecItemNotFound (-25300), truncated to Unix's exit byte.
            return if output.status.code() == Some(44) {
                Ok(None)
            } else {
                Err(std::io::Error::other("Keychain metadata lookup failed"))
            };
        }
        let invalid = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Keychain item has no readable modification date",
            )
        };
        let text = String::from_utf8_lossy(&output.stdout);
        let line = text
            .lines()
            .find(|line| line.contains("\"mdat\"<timedate>="))
            .ok_or_else(invalid)?;
        let stamp = line.split('"').nth(3).ok_or_else(invalid)?;
        let digits = stamp
            .strip_suffix("Z\\000")
            .or_else(|| stamp.strip_suffix('Z'))
            .and_then(|s| s.get(..14))
            .ok_or_else(invalid)?;
        let time =
            jiff::civil::DateTime::strptime("%Y%m%d%H%M%S", digits).map_err(|_| invalid())?;
        let at = time
            .to_zoned(jiff::tz::TimeZone::UTC)
            .map_err(|_| invalid())?
            .timestamp();
        Ok(Some(SystemTime::from(at)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, fs, time::Duration};

    #[test]
    fn unchanged_sign_in_metadata_is_reused_and_an_external_store_change_is_seen() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("fixture.keychain-db");
        fs::write(&path, b"first").unwrap();
        let date = Cell::new(Some(SystemTime::UNIX_EPOCH + Duration::from_secs(10)));
        let reads = Cell::new(0);
        let cache = CredentialDate::default();
        let read = || {
            cache.read(
                "fixture",
                || Some(vec![Store::new(path.clone(), 1)]),
                |_| {
                    reads.set(reads.get() + 1);
                    Ok(date.get())
                },
            )
        };
        assert_eq!(read(), date.get());
        for _ in 0..20 {
            assert_eq!(read(), date.get());
        }
        assert_eq!(reads.get(), 1);
        // A new Keychain item (or removing one) changes the store, not
        // necessarily Claude's retained account record.
        fs::write(&path, b"signed out, no key").unwrap();
        date.set(None);
        assert_eq!(read(), None);
        assert_eq!(read(), None);
        assert_eq!(reads.get(), 2);
        fs::write(&path, b"renewed").unwrap();
        date.set(Some(SystemTime::UNIX_EPOCH + Duration::from_secs(20)));
        assert_eq!(read(), date.get());
        assert_eq!(reads.get(), 3);
        cache.invalidate();
        assert_eq!(read(), date.get());
        assert_eq!(reads.get(), 4);
    }
}
