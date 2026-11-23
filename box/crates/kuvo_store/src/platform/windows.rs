use std::{
    fs::{self, File, OpenOptions},
    io,
    mem::{offset_of, size_of},
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
    ptr, thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_DIR_NOT_EMPTY, ERROR_LOCK_VIOLATION,
        ERROR_SHARING_VIOLATION,
    },
    Storage::FileSystem::{
        DELETE, FILE_RENAME_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FileRenameInfoEx, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        SetFileInformationByHandle,
    },
    System::WindowsProgramming::{
        FILE_RENAME_FLAG_POSIX_SEMANTICS, FILE_RENAME_FLAG_REPLACE_IF_EXISTS,
    },
};

pub fn private_dirs(folder: &Path) -> io::Result<()> {
    fs::create_dir_all(folder)
}

// The account's Kuvo home supplies the inheritable ACL on Windows.
pub fn private_file(_: &File) -> io::Result<()> {
    Ok(())
}
pub fn restrict_folder(_: &Path) -> io::Result<()> {
    Ok(())
}

// Windows exposes no equivalent of fsync on a directory.
pub fn sync_dir(_: &Path) -> io::Result<()> {
    Ok(())
}

fn wide(path: &Path) -> io::Result<Vec<u16>> {
    // Canonicalize the parent only. The leaf may be new or a reparse point to replace.
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing file name"))?;
    let parent = super::super::files::parent(path)?;
    let mut wide: Vec<_> = parent
        .canonicalize()?
        .join(name)
        .as_os_str()
        .encode_wide()
        .collect();
    if wide.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "file name contains NUL",
        ));
    }
    wide.push(0);
    Ok(wide)
}

fn retry(mut operation: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        match operation() {
            Err(error)
                if matches!(
                    error.raw_os_error().map(|code| code as u32),
                    Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION | ERROR_ACCESS_DENIED)
                ) && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(10))
            }
            result => return result,
        }
    }
}

pub fn replace(from: &Path, to: &Path) -> io::Result<()> {
    let source = wide(from)?;
    let target = wide(to)?;
    retry(
        || match move_file(&source, &target, MOVEFILE_REPLACE_EXISTING) {
            Err(error)
                if matches!(
                    error.raw_os_error().map(|code| code as u32),
                    Some(ERROR_SHARING_VIOLATION | ERROR_ACCESS_DENIED)
                ) =>
            {
                // Phase 0.4 established that a reader holding FILE_SHARE_DELETE
                // can stay on the old file using the POSIX rename flags.
                rename_open(from, to, &target)
            }
            result => result,
        },
    )
}

fn rename_open(from: &Path, to: &Path, target: &[u16]) -> io::Result<()> {
    // Keep delete access to the old file until the rename. This respects a
    // reader that denied delete sharing, even on filesystems supporting POSIX rename.
    let _previous = OpenOptions::new()
        .access_mode(DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(to)?;
    let file = OpenOptions::new()
        .access_mode(DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(from)?;
    let name = &target[..target.len() - 1];
    let name_bytes = size_of_val(name);
    let bytes =
        size_of::<FILE_RENAME_INFO>().max(offset_of!(FILE_RENAME_INFO, FileName) + name_bytes);
    let mut buffer = vec![0usize; bytes.div_ceil(size_of::<usize>())];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: the zeroed buffer has pointer alignment and room for the struct
    // and all UTF-16 filename bytes. The file and buffer live through the call.
    unsafe {
        (*info).Anonymous.Flags =
            FILE_RENAME_FLAG_REPLACE_IF_EXISTS | FILE_RENAME_FLAG_POSIX_SEMANTICS;
        (*info).FileNameLength = name_bytes as u32;
        ptr::copy_nonoverlapping(
            name.as_ptr(),
            info.cast::<u8>()
                .add(offset_of!(FILE_RENAME_INFO, FileName))
                .cast::<u16>(),
            name.len(),
        );
        if SetFileInformationByHandle(
            file.as_raw_handle(),
            FileRenameInfoEx,
            info.cast(),
            bytes as u32,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn move_file(from: &[u16], to: &[u16], options: u32) -> io::Result<()> {
    // SAFETY: both inputs came from wide(), which returns NUL-terminated paths.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), options | MOVEFILE_WRITE_THROUGH) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn publish_new(from: &Path, to: &Path) -> io::Result<()> {
    let from = wide(from)?;
    let to = wide(to)?;
    retry(|| move_file(&from, &to, 0))
}

pub fn launcher_runs(pid: u32) -> io::Result<bool> {
    // Legacy shell owners never ran natively. Leave any live PID alone.
    Ok(kuvo_os::process::ProcessIdentity::read(pid)?.is_some())
}

pub fn directory_conflict(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error().map(|code| code as u32),
        Some(ERROR_ALREADY_EXISTS | ERROR_DIR_NOT_EMPTY | ERROR_ACCESS_DENIED)
    )
}

pub fn open_for_sync(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).open(path)
}

pub fn directory_id(path: &Path) -> Option<u64> {
    use std::time::UNIX_EPOCH;
    u64::try_from(
        fs::metadata(path)
            .ok()?
            .created()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos(),
    )
    .ok()
}

pub fn file_stamp(path: &Path) -> io::Result<crate::files::FileStamp> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_BASIC_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_INFO, FILE_READ_ATTRIBUTES,
        FileBasicInfo, FileIdInfo, GetFileInformationByHandleEx,
    };

    let file = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    let mut times = FILE_BASIC_INFO::default();
    let mut id = FILE_ID_INFO::default();
    // SAFETY: file is live and each buffer is valid for the requested information class.
    unsafe {
        if GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileBasicInfo,
            (&raw mut times).cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        ) == 0
            || GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileIdInfo,
                (&raw mut id).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            ) == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    // FILETIME counts 100ns intervals since 1601.
    let nanos = |ticks| (i128::from(ticks) - 116_444_736_000_000_000) * 100;
    Ok(crate::files::FileStamp {
        device: id.VolumeSerialNumber,
        file: u128::from_le_bytes(id.FileId.Identifier),
        created_ns: Some(nanos(times.CreationTime)),
        modified_ns: nanos(times.LastWriteTime),
        changed_ns: nanos(times.ChangeTime),
        size: file.metadata()?.len(),
    })
}
