//! A device's keys on the link (PROTOCOL.md, "Keys and addresses"): its static X25519 key, which
//! the boxes it's paired with know it by, and its relay secret, which only the relay sees.
//!
//! Nothing here prints a key or a secret: `Debug` shows only the public half.

use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

/// Base64url without padding, as keys, addresses and tokens are written in text.
pub fn encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Text back to bytes; none for what isn't base64url.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(text.trim_end_matches('=')).ok()
}

/// Exactly `N` bytes from text, else none.
pub fn decode_exact<const N: usize>(text: &str) -> Option<[u8; N]> {
    decode(text)?.try_into().ok()
}

/// Where a device is on the relay: the first 16 bytes of `SHA-256("kuvo-link address v1" || secret)`.
pub type Address = [u8; 16];

pub fn address_of(secret: &[u8; 32]) -> Address {
    let mut hash = Sha256::new();
    hash.update(b"kuvo-link address v1");
    hash.update(secret);
    let digest = hash.finalize();
    let mut address = [0u8; 16];
    address.copy_from_slice(&digest[..16]);
    address
}

/// A box's public facts on the link: its static key and where it is on the relay.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Host {
    pub key: [u8; 32],
    pub address: Address,
}

impl Host {
    /// From the text a box's `kuvo/computers` (or a pairing link) gives.
    pub fn from_text(key: &str, address: &str) -> Option<Host> {
        Some(Host { key: decode_exact(key)?, address: decode_exact(address)? })
    }
}

/// This device on the link: its static key and its relay secret, made once and kept by the app in
/// the system's keychain ([`Keeper`]). The same key for every computer; a secret of its own on
/// each relay, so two relays can't tell it's the same device.
#[derive(Clone)]
pub struct Identity {
    key: [u8; 32],
    secret: [u8; 32],
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never its key or secret.
        f.debug_struct("Identity").field("public", &encode(&self.public())).finish_non_exhaustive()
    }
}

impl Identity {
    /// A new one, from the system's randomness.
    pub fn new() -> Identity {
        let (mut key, mut secret) = ([0u8; 32], [0u8; 32]);
        getrandom::fill(&mut key).expect("the system gives random bytes");
        getrandom::fill(&mut secret).expect("the system gives random bytes");
        Identity { key, secret }
    }

    pub fn from_bytes(key: [u8; 32], secret: [u8; 32]) -> Identity {
        Identity { key, secret }
    }

    /// As it's kept: `<key>.<secret>`, base64url. Only for a [`Keeper`].
    pub fn kept(&self) -> String {
        format!("{}.{}", encode(&self.key), encode(&self.secret))
    }

    /// One read back from where it's kept; none for what isn't one.
    pub fn from_kept(text: &str) -> Option<Identity> {
        let (key, secret) = text.trim().split_once('.')?;
        Some(Identity { key: decode_exact(key)?, secret: decode_exact(secret)? })
    }

    /// The static key's public half, which a box keeps once this device is paired.
    pub fn public(&self) -> [u8; 32] {
        PublicKey::from(&StaticSecret::from(self.key)).to_bytes()
    }

    pub(crate) fn private(&self) -> [u8; 32] {
        self.key
    }

    /// Its secret on `relay` (HMAC of `kuvo-link relay v1 <url>` under the one secret), as a phone
    /// derives its own.
    pub fn secret_on(&self, relay: &str) -> [u8; 32] {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.secret).expect("any length is a key");
        mac.update(format!("kuvo-link relay v1 {relay}").as_bytes());
        mac.finalize().into_bytes().into()
    }

    /// Where it is on `relay`.
    pub fn address_on(&self, relay: &str) -> Address {
        address_of(&self.secret_on(relay))
    }
}

impl Default for Identity {
    fn default() -> Self {
        Identity::new()
    }
}

/// Where the app keeps this device's [`Identity`]: the system's keychain on a Mac, a file only you
/// can read in the app's own folder elsewhere, memory in tests. `kuvo_box` never writes it itself.
pub trait Keeper: Send + Sync {
    /// What's kept, as [`Identity::kept`] wrote it.
    fn read(&self) -> Kept;
    /// Keeps it; false when it couldn't be.
    fn write(&self, kept: &str) -> bool;
    /// Lets it go (this device was removed, and makes a new one on your word).
    fn forget(&self);
}

/// What a [`Keeper`] holds now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kept {
    Found(String),
    /// Nothing's kept: this device has no key yet.
    Nothing,
    /// It couldn't be read now (the keychain is locked, or refused): there may be a key, so none
    /// is made in its place.
    Unreadable,
}

/// The identity `keeper` holds, made and kept the first time (only when it says nothing's kept).
/// None when it can't be read or kept: a device that would be a new one at each start never
/// pairs, and one whose key can't be read now isn't replaced by another.
pub fn identity(keeper: &dyn Keeper) -> Option<Identity> {
    match keeper.read() {
        Kept::Found(text) => Identity::from_kept(&text),
        Kept::Unreadable => None,
        Kept::Nothing => {
            let made = Identity::new();
            keeper.write(&made.kept()).then_some(made)
        }
    }
}

/// A keeper in memory, for tests and for a run that keeps nothing.
#[derive(Default)]
pub struct MemoryKeeper(std::sync::Mutex<Option<String>>);

impl Keeper for MemoryKeeper {
    fn read(&self) -> Kept {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone().map_or(Kept::Nothing, Kept::Found)
    }

    fn write(&self, kept: &str) -> bool {
        *self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(kept.to_owned());
        true
    }

    fn forget(&self) {
        *self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}
