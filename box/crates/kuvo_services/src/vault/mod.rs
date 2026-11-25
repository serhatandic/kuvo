//! Vault metadata and the separate platform authority for its values.
mod activity;
mod ask_routes;
mod asks;
mod broker;
mod client;
mod command;
mod crypto;
mod encrypted;
mod fill;
mod helper;
mod holder;
mod identity;
mod items;
#[cfg(target_os = "macos")]
mod mac_wipe;
mod mask;
mod replay;
mod routes;
mod session_asks;
mod signed;
#[cfg(unix)]
mod ssh_agent;
#[cfg(unix)]
mod ssh_keys;
mod store;
mod tools;

pub use holder::Holder;
#[cfg(target_os = "macos")]
pub use mac_wipe::clear_mac_app_link_keys;

pub use mask::{Hidden, mask};
pub use routes::Resident;
pub use session_asks::{
    VaultAsk, VaultAskCaller, VaultAskReply, VaultCard, VaultDecision, VaultWaiting,
};

pub use crypto::{Secret, Values};
pub use encrypted::Encrypted;
pub use helper::Launch;
pub use items::{Item, Kind, site_of};
pub use signed::{File, Generation, Signed};
pub use store::{Grant, How, State, Vault, Who};

use std::{borrow::Cow, fmt, io};

pub const LOCKED: &str = "This computer's vault is locked since it started again. Open Kuvo Code on your Mac or iPhone to unlock it.";
pub const TAMPERED: &str = "The vault's files were changed outside Kuvo Code, so it uses none of them. Add its items again.";

#[derive(Debug)]
pub struct Error {
    pub status: u16,
    pub message: Cow<'static, str>,
    source: Option<io::Error>,
}

impl Error {
    pub(crate) fn refused(status: u16, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            status,
            message: message.into(),
            source: None,
        }
    }

    pub(crate) fn disk(message: &'static str, source: io::Error) -> Self {
        Self {
            status: 503,
            message: message.into(),
            source: Some(source),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_ref().map(|error| error as _)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn check_id(id: &str) -> Result<()> {
    if id.strip_prefix("v_").is_some_and(|suffix| {
        suffix.len() == 10
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    }) {
        Ok(())
    } else {
        Err(Error::refused(400, "That isn't an item."))
    }
}

pub(crate) use command::command_environment;
