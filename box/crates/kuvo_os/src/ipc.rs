//! Private local byte streams for SSH forwarding and holders.

use std::{
    io,
    path::{Path, PathBuf},
};
use tokio::io::{AsyncRead, AsyncWrite};

#[cfg(unix)]
#[path = "ipc/unix.rs"]
mod sys;
#[cfg(windows)]
#[path = "ipc/windows.rs"]
mod sys;

pub trait ByteStream: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> ByteStream for T {}
pub type LocalStream = Box<dyn ByteStream>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Channel {
    Ssh,
    Keeper,
    Holder(String),
}

impl Channel {
    fn label(&self) -> io::Result<String> {
        match self {
            Self::Ssh => Ok("ssh".into()),
            Self::Keeper => Ok("keeper-stop".into()),
            Self::Holder(id)
                if id.len() == 12
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) =>
            {
                Ok(format!("holder:{id}"))
            }
            Self::Holder(_) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid holder ID",
            )),
        }
    }
}

#[derive(Clone)]
pub struct Endpoint {
    path: PathBuf,
    #[cfg(unix)]
    channel: Channel,
    #[cfg(windows)]
    sid: String,
}

impl Endpoint {
    /// Use the same socket filename and key as the existing holder and SSH protocols.
    pub fn new(unix_path: &Path, home_key: &[u8; 32], channel: Channel) -> io::Result<Self> {
        let label = channel.label()?;
        #[cfg(unix)]
        {
            let _ = (home_key, label);
            if !unix_path.is_absolute() || unix_path.file_name().is_none() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "local socket requires an absolute filename",
                ));
            }
            Ok(Self {
                path: unix_path.to_owned(),
                channel,
            })
        }
        #[cfg(windows)]
        {
            let _ = unix_path;
            let sid = sys::user_sid()?;
            let path = PathBuf::from(windows_pipe_name(&sid, home_key, &label));
            Ok(Self { path, sid })
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run inside a Tokio runtime with I/O enabled.
    pub fn bind(&self) -> io::Result<Listener> {
        sys::bind(self).map(|inner| Listener { inner })
    }

    pub async fn connect(&self) -> io::Result<LocalStream> {
        sys::connect(self).await
    }
}

/// The TypeScript protocol's pipe spelling. The account SID is used, including in SSH logons
/// with a different logon SID. The key and channel separate listeners belonging to that account.
pub fn windows_pipe_path(sid: &str, home_key: &[u8; 32], channel: &Channel) -> io::Result<String> {
    let parts = sid
        .strip_prefix("S-1-")
        .map(|suffix| suffix.split('-').collect::<Vec<_>>());
    if !parts.is_some_and(|parts| {
        parts.len() >= 2
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    }) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid account SID",
        ));
    }
    Ok(windows_pipe_name(sid, home_key, &channel.label()?))
}

fn windows_pipe_name(sid: &str, home_key: &[u8; 32], label: &str) -> String {
    use hmac::{Hmac, Mac};
    let mut mac =
        Hmac::<sha2::Sha256>::new_from_slice(home_key).expect("HMAC accepts this key length");
    mac.update(format!("kuvo-local-ipc-v1\0{sid}\0{label}").as_bytes());
    format!(
        r"\\.\pipe\kuvo-{}",
        hex::encode(mac.finalize().into_bytes())
    )
}

pub struct Listener {
    inner: sys::Listener,
}

impl Listener {
    pub async fn accept(&mut self) -> io::Result<LocalStream> {
        self.inner.accept().await
    }
}
