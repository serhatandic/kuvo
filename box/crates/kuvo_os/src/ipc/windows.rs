use super::*;
use std::{
    ffi::c_void,
    mem::{MaybeUninit, size_of},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::null_mut,
};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
use windows_sys::Win32::{
    Foundation::{HANDLE, LocalFree},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            GetSecurityInfo, SDDL_REVISION_1, SE_KERNEL_OBJECT,
        },
        GetTokenInformation, OWNER_SECURITY_INFORMATION, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY,
        TOKEN_USER, TokenUser,
    },
    Storage::FileSystem::SECURITY_IDENTIFICATION,
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

struct LocalAllocation(*mut c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: the pointer is from a Win32 LocalAlloc-based conversion.
        unsafe { LocalFree(self.0) };
    }
}

pub(super) fn user_sid() -> io::Result<String> {
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
unsafe fn sid_text(sid: PSID) -> io::Result<String> {
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

fn owner_sid(pipe: HANDLE) -> io::Result<String> {
    let mut owner = null_mut();
    let mut descriptor = null_mut();
    // SAFETY: the pipe is open and both outputs are writable.
    let result = unsafe {
        GetSecurityInfo(
            pipe,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    let _allocation = LocalAllocation(descriptor);
    if owner.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "local pipe has no owner SID",
        ));
    }
    // SAFETY: owner points into the live security descriptor returned from GetSecurityInfo.
    unsafe { sid_text(owner) }
}

fn server(endpoint: &Endpoint, first: bool) -> io::Result<NamedPipeServer> {
    let sid = &endpoint.sid;
    let sddl: Vec<_> = format!("O:{sid}D:P(A;;GA;;;{sid})")
        .encode_utf16()
        .chain([0])
        .collect();
    let mut descriptor = null_mut();
    // SAFETY: the SDDL is NUL-terminated and descriptor is a writable output pointer.
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
    let _allocation = LocalAllocation(descriptor);
    let mut security = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    // SAFETY: the descriptor and SECURITY_ATTRIBUTES live through CreateNamedPipeW.
    unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(
                &endpoint.path,
                std::ptr::from_mut(&mut security).cast(),
            )
    }
}

pub(super) struct Listener {
    endpoint: Endpoint,
    next: NamedPipeServer,
}

pub(super) fn bind(endpoint: &Endpoint) -> io::Result<Listener> {
    Ok(Listener {
        endpoint: endpoint.clone(),
        next: server(endpoint, true)?,
    })
}

pub(super) async fn connect(endpoint: &Endpoint) -> io::Result<LocalStream> {
    // One instance is always available until it accepts. The caller can retry a busy listener.
    let pipe = ClientOptions::new()
        .security_qos_flags(SECURITY_IDENTIFICATION)
        .open(&endpoint.path)?;
    if owner_sid(pipe.as_raw_handle())? != endpoint.sid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "local pipe belongs to a different account SID",
        ));
    }
    Ok(Box::new(pipe))
}

impl Listener {
    pub(super) async fn accept(&mut self) -> io::Result<LocalStream> {
        self.next.connect().await?;
        match server(&self.endpoint, false) {
            Ok(next) => Ok(Box::new(std::mem::replace(&mut self.next, next))),
            Err(error) => {
                let _ = self.next.disconnect();
                Err(error)
            }
        }
    }
}
