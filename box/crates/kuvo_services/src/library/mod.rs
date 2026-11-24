//! The box's Library: what was handed over to you on this computer and gathered across your
//! computers (`kuvo/library…`, `kuvo/setup/library/events`).

pub mod peers;
pub mod routes;
pub mod service;
pub mod store;
pub mod thumbnails;

pub use peers::{
    Asked, LibraryPeers, PartState, PeerComputer, filter_peer_copy, is_part, keep_part, kept_part,
    library_for_peer, merged,
};
pub use routes::{FEATURES, LibraryHandle, filter_of, register};
pub use service::{LibraryOptions, LibraryService};
pub use store::{Filter, Handover, How, Item, LibraryKind, LibraryStore, LibraryStoreError};
pub use thumbnails::{Thumbnail, ThumbnailPlatform, Thumbnails};
