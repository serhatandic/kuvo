//! A client of a Kuvo box (protocol/PROTOCOL.md), for Kuvo's GPUI app and any other Rust
//! client: finding a box and reaching it ([`BoxClient::this_mac`], [`BoxClient::machine`]), its
//! update gate ([`gate`]), its session lists ([`sessions`]), a conversation's messages
//! ([`messages`]) and what a typed text means ([`typed`]), followed live ([`follow`]).
//!
//! The box decides; this only reads its facts, changes them through its routes, and orders what's
//! drawn the way every client does (held to `protocol/vectors`). No GPUI here.
//!
//! Nothing here logs. A box's password is read only from its own record (`run/serve.json`), sent
//! only to the box's server once the process that record names is that server, and never shown
//! (`Debug` leaves it out).

pub mod accounts;
pub mod approval;
pub mod areas;
pub mod asks;
pub mod artifacts;
pub mod background;
pub mod bots;
pub mod browser_view;
pub mod boxes;
pub mod client;
pub mod connections;
pub mod error;
pub mod find;
pub mod folders;
pub mod follow;
pub mod gate;
pub use kuvo_protocol::FEATURES;
pub mod git;
pub mod groups;
pub mod ids;
pub mod library;
pub mod marketplace;
pub mod image_names;
pub mod info;
pub mod install;
pub mod line;
pub mod lines;
pub mod link;
pub mod mentions;
pub mod local;
pub mod messages;
pub mod models;
pub mod moves;
pub mod notices;
pub mod parts;
pub mod paths;
pub mod permissions;
pub mod pictures;
pub mod place;
pub mod printed;
pub mod pty;
pub mod queue;
pub mod quoting;
pub mod search;
pub mod session_actions;
pub mod session_context;
pub mod sessions;
pub mod subagents;
pub mod settings;
pub mod settings_pages;
pub mod setup;
pub mod shown_files;
pub mod side_chats;
mod socket;
pub mod ssh;
pub mod transport;
pub mod typed;
pub mod work_queue;

pub use client::{BoxClient, Draft, Sent, WorkCounts};
pub use parts::{Delivery, Parts, Sending};
pub use error::{Error, Refusal, Result};
pub use follow::{ConversationUpdate, Follow, ListUpdate, Timing};
pub use gate::Gate;
pub use info::BoxInfo;
pub use messages::{Conversation, Message, Where};
pub use sessions::{Listing, Sidebar};
pub use ssh::SshBox;

pub mod wakes;
