//! Windows account security and the private NTFS installation folder.

use std::{
    ffi::c_void,
    fs::{File, OpenOptions},
    io,
    mem::{MaybeUninit, size_of},
    os::windows::{
        ffi::OsStrExt as _,
        fs::{MetadataExt as _, OpenOptionsExt as _},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    ptr::null_mut,
};
use windows_sys::Win32::{
    Foundation::{ERROR_ALREADY_EXISTS, LocalFree},
    Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER,
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT, SetSecurityInfo,
        },
        CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, GetAce, GetSecurityDescriptorControl,
        GetSecurityDescriptorDacl, GetTokenInformation, OBJECT_INHERIT_ACE,
        OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSID, SE_DACL_PROTECTED,
        SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    Storage::FileSystem::{
        CreateDirectoryW, FILE_ALL_ACCESS, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        GetDriveTypeW, GetVolumeInformationByHandleW, READ_CONTROL, WRITE_DAC,
    },
    System::{
        SystemServices::{ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE},
        Threading::{GetCurrentProcess, OpenProcessToken},
        WindowsProgramming::DRIVE_FIXED,
    },
};

pub(crate) struct LocalAllocation(pub(crate) *mut c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: the pointer is from a Win32 LocalAlloc-based conversion.
        unsafe { LocalFree(self.0) };
    }
}

pub fn user_sid() -> io::Result<String> {
    let mut token = null_mut();
    // SAFETY: token is a writable handle output, and the process pseudohandle is valid.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful OpenProcessToken returned a new owned handle.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut size = 0;
    // SAFETY: no buffer supplied, asking only for its size.
    unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut size) };
    if size < size_of::<TOKEN_USER>() as u32 {
        return Err(io::Error::last_os_error());
    }
    let mut buffer =
        vec![MaybeUninit::<usize>::uninit(); (size as usize).div_ceil(size_of::<usize>())];
    // SAFETY: aligned buffer has at least size bytes of writable storage.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            size,
            &mut size,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: success initialized TOKEN_USER with a valid SID within the live buffer.
    unsafe { sid_text((*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid) }
}

/// The caller owns the buffer that contains this SID for the duration of the call.
pub(crate) unsafe fn sid_text(sid: PSID) -> io::Result<String> {
    let mut text = null_mut();
    // SAFETY: the caller supplies a valid SID and text is a writable output pointer.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _allocation = LocalAllocation(text.cast());
    // SAFETY: the conversion returned a NUL-terminated string in a live allocation.
    unsafe {
        let length = (0..).find(|i| text.add(*i).read() == 0).unwrap();
        String::from_utf16(std::slice::from_raw_parts(text, length)).map_err(io::Error::other)
    }
}

pub(crate) struct SecurityDescriptor(LocalAllocation);

impl SecurityDescriptor {
    pub(crate) fn parse(sddl: &str) -> io::Result<Self> {
        let sddl: Vec<_> = sddl.encode_utf16().chain([0]).collect();
        let mut descriptor = null_mut();
        // SAFETY: the SDDL is terminated and the output pointer is writable.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(LocalAllocation(descriptor)))
    }

    pub(crate) fn as_ptr(&self) -> *mut c_void {
        self.0.0
    }
}

/// Create the home with its final ACL in one operation. Existing homes must
/// already be private. Repairing one could hide prior exposure of its keys.
pub fn private_home(path: &Path, create: bool) -> io::Result<()> {
    check_home(path, create).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "Kuvo needs a private folder on a local NTFS drive at {}: {error}",
                path.display()
            ),
        )
    })
}

fn check_home(path: &Path, create: bool) -> io::Result<()> {
    let invalid = || io::Error::new(io::ErrorKind::PermissionDenied, "expected a user-only ACL");
    let parent = path.parent().ok_or_else(invalid)?.canonicalize()?;
    let root = parent.ancestors().last().ok_or_else(invalid)?;
    let drive = crate::platform::windows_conventional_path(root).ok_or_else(invalid)?;
    let drive = drive
        .to_str()
        .filter(|drive| drive.as_bytes().get(1) == Some(&b':'));
    let drive: Vec<_> = drive
        .ok_or_else(invalid)?
        .encode_utf16()
        .chain([0])
        .collect();
    // SAFETY: drive is an absolute, NUL-terminated root.
    if unsafe { GetDriveTypeW(drive.as_ptr()) } != DRIVE_FIXED {
        return Err(invalid());
    }
    let directory = |path: &Path, access| -> io::Result<File> {
        let file = OpenOptions::new()
            .access_mode(access | FILE_READ_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(invalid());
        }
        Ok(file)
    };
    let volume = directory(&parent, 0)?;
    let mut filesystem = [0u16; 32];
    // SAFETY: volume is open, the output array is writable, and optional outputs are omitted.
    if unsafe {
        GetVolumeInformationByHandleW(
            volume.as_raw_handle(),
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            null_mut(),
            filesystem.as_mut_ptr(),
            filesystem.len() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if String::from_utf16_lossy(&filesystem[..4]) != "NTFS" {
        return Err(invalid());
    }
    let sid = user_sid()?;
    let path = parent.join(path.file_name().ok_or_else(invalid)?);
    if create {
        let descriptor = SecurityDescriptor::parse(&format!("O:{sid}D:P(A;OICI;FA;;;{sid})"))?;
        let security = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.as_ptr(),
            bInheritHandle: 0,
        };
        let wide: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
        if wide[..wide.len() - 1].contains(&0) {
            return Err(invalid());
        }
        // SAFETY: both inputs are live, with the extended path terminated by NUL.
        if unsafe { CreateDirectoryW(wide.as_ptr(), &security) } == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_ALREADY_EXISTS as i32) {
                return Err(error);
            }
        }
    }
    let folder = directory(&path, READ_CONTROL)?;
    verify_acl(&folder, &sid).and_then(|private| if private { Ok(()) } else { Err(invalid()) })
}

/// A folder for secrets, where it really is: refused when another account owns it, and closed
/// to everyone but this account when it's open to others. The private home's inherited ACL
/// already closes the folders inside it.
pub fn private_folder(path: &Path) -> io::Result<()> {
    let folder = OpenOptions::new()
        .access_mode(READ_CONTROL | WRITE_DAC | FILE_READ_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    let sid = user_sid()?;
    let security = Security::of(&folder)?;
    if !security.owned_here(&sid) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the folder belongs to another account",
        ));
    }
    if security.closed_to_others(&sid) {
        return Ok(());
    }
    let descriptor = SecurityDescriptor::parse(&format!("D:P(A;OICI;FA;;;{sid})"))?;
    let (mut present, mut acl, mut defaulted) = (0, null_mut(), 0);
    // SAFETY: the descriptor is live and every output is writable.
    if unsafe {
        GetSecurityDescriptorDacl(descriptor.as_ptr(), &mut present, &mut acl, &mut defaulted)
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the folder is open with WRITE_DAC and the ACL lives in the descriptor.
    let error = unsafe {
        SetSecurityInfo(
            folder.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            acl,
            null_mut(),
        )
    };
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    Ok(())
}

/// Whether this file or folder itself (not a link's target) is owned here and open to this
/// account only, as `private_folder` leaves one.
pub fn is_private(path: &Path) -> io::Result<bool> {
    let file = OpenOptions::new()
        .access_mode(READ_CONTROL | FILE_READ_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let sid = user_sid()?;
    let security = Security::of(&file)?;
    Ok(security.owned_here(&sid) && security.closed_to_others(&sid))
}

struct Ace {
    kind: u32,
    flags: u32,
    mask: u32,
    sid: String,
}

/// A file's owner and DACL; no entries for a NULL DACL, which lets everyone in.
struct Security {
    owner: String,
    protected: bool,
    aces: Option<Vec<Ace>>,
}

impl Security {
    /// As root may on Unix, SYSTEM and Administrators can read anything here anyway.
    fn owned_here(&self, sid: &str) -> bool {
        [sid, "S-1-5-18", "S-1-5-32-544"].contains(&self.owner.as_str())
    }

    fn closed_to_others(&self, sid: &str) -> bool {
        self.aces.as_ref().is_some_and(|aces| {
            aces.iter().all(|ace| match ace.kind {
                ACCESS_ALLOWED_ACE_TYPE => ace.sid == sid,
                ACCESS_DENIED_ACE_TYPE => true,
                _ => false,
            })
        })
    }

    fn of(file: &File) -> io::Result<Self> {
        let (mut descriptor, mut owner, mut acl) = (null_mut(), null_mut(), null_mut());
        // SAFETY: the file is open with READ_CONTROL. All supplied outputs are writable.
        let error = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                &mut acl,
                null_mut(),
                &mut descriptor,
            )
        };
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        let _descriptor = LocalAllocation(descriptor);
        let invalid = || io::Error::new(io::ErrorKind::InvalidData, "an unreadable ACL");
        // SAFETY: all pointers below refer to the live descriptor returned by GetSecurityInfo.
        unsafe {
            if owner.is_null() {
                return Err(invalid());
            }
            let (mut control, mut revision) = (0, 0);
            if GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) == 0 {
                return Err(io::Error::last_os_error());
            }
            let aces = if acl.is_null() {
                None
            } else {
                let mut aces = Vec::new();
                for index in 0..u32::from((*acl).AceCount) {
                    let mut ace = null_mut();
                    if GetAce(acl, index, &mut ace) == 0 {
                        return Err(io::Error::last_os_error());
                    }
                    let head = &*ace.cast::<ACE_HEADER>();
                    let kind = u32::from(head.AceType);
                    // Allowed and denied entries share one layout; others keep no SID here.
                    let simple = matches!(kind, ACCESS_ALLOWED_ACE_TYPE | ACCESS_DENIED_ACE_TYPE);
                    if simple && usize::from(head.AceSize) < size_of::<ACCESS_ALLOWED_ACE>() {
                        return Err(invalid());
                    }
                    let ace = &*ace.cast::<ACCESS_ALLOWED_ACE>();
                    aces.push(Ace {
                        kind,
                        flags: u32::from(head.AceFlags),
                        mask: if simple { ace.Mask } else { 0 },
                        sid: if simple {
                            sid_text((&raw const ace.SidStart).cast_mut().cast())?
                        } else {
                            String::new()
                        },
                    });
                }
                Some(aces)
            };
            Ok(Self {
                owner: sid_text(owner)?,
                protected: control & SE_DACL_PROTECTED != 0,
                aces,
            })
        }
    }
}

fn verify_acl(folder: &File, sid: &str) -> io::Result<bool> {
    let security = Security::of(folder)?;
    Ok(security.owner == sid
        && security.protected
        && matches!(security.aces.as_deref(), Some([ace]) if ace.kind == ACCESS_ALLOWED_ACE_TYPE
            && ace.flags == OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
            && ace.mask == FILE_ALL_ACCESS
            && ace.sid == sid))
}

pub fn system_directory() -> io::Result<std::path::PathBuf> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt as _};
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut buffer = [0; 260];
    // SAFETY: the buffer is writable for its stated capacity.
    let size = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    if size == 0 || size >= buffer.len() {
        return Err(io::Error::last_os_error());
    }
    Ok(OsString::from_wide(&buffer[..size]).into())
}
