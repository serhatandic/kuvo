use super::*;
pub(super) use crate::windows::user_sid;
use crate::windows::{LocalAllocation, SecurityDescriptor, sid_text};
use std::{mem::size_of, os::windows::io::AsRawHandle, ptr::null_mut};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
use windows_sys::Win32::{
    Foundation::HANDLE,
    Security::{
        Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT},
        OWNER_SECURITY_INFORMATION, SECURITY_ATTRIBUTES,
    },
    Storage::FileSystem::SECURITY_IDENTIFICATION,
};

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
    let descriptor = SecurityDescriptor::parse(&format!("O:{sid}D:P(A;;GA;;;{sid})"))?;
    let mut security = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.as_ptr(),
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
