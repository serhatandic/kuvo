//! Artifacts record: versions, content-addressed copies, diffs, turn-end discovery,
//! attachment path relocation, and routes (`kuvo/artifacts…`, `kuvo/setup/outputs…`).

pub mod attachments;
pub mod budget;
pub mod changes;
pub mod diff;
pub mod folders;
pub mod index;
pub mod kinds;
pub mod paths;
pub mod policy;
pub mod prepare;
pub mod recorder;
pub mod routes;
pub mod store;
pub mod turnfiles;

pub use routes::{
    ArtifactHandle, FEATURES, OutputPeerCopy, OutputsFuture, OutputsPeers, OutputsSnapshot,
    PeerOutputLatest, PeerOutputRow, PendingComputer, local_output_copy, register,
};
pub use store::{ArtifactStore, StoreError, StoreOptions};
