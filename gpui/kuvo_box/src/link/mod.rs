//! The link (protocol/PROTOCOL.md, "Kuvo Link"): how this client reaches a box that isn't on its
//! own computer, through the relay, end to end encrypted, exactly as the phone does. The client is
//! a paired device with a key of its own ([`Identity`]); it opens one session per box
//! ([`LinkClient`]) and asks each the same routes a box on this computer answers ([`LinkBox`]).
//!
//! Held to `protocol/vectors` (`noise.json`, `link.json`), as KuvoLink and the box are.

pub mod client;
pub use kuvo_wire::frames;
pub mod keys;
pub mod linked;
pub mod log;
pub use kuvo_wire::noise;
pub mod relay;
#[cfg(feature = "stand-in")]
pub mod stand_in;

pub use client::{BoxHello, LinkClient, Timing, is_unpaired};
pub(crate) use client::words::{PAIRING_READD, PAIRING_WAIT};
pub use keys::{Host, Identity, Keeper, Kept, MemoryKeeper};
pub use linked::LinkBox;
pub use relay::{Dial, WebSocketDial};
