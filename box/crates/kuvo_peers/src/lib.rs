//! A computer's relay connection, paired sessions, peer mail and durable delivery.

pub mod activity;
mod attention;
pub mod calls;
pub mod conversation;
pub mod devices;
pub mod directory;
mod kept;
mod helper_menu;
mod letters;
pub mod lists;
pub mod manage;
pub mod memory_read;
pub mod moves;
pub mod notices;
pub mod outbox;
pub mod owner_read;
pub mod ownership;
mod parts;
pub mod reach;
pub mod relay;
mod relay_routes;
pub mod remote_work;
pub mod routes;
pub mod session;
pub mod setup_lists;
pub mod work_rows;

pub use parts::{work_queue_mails, SeatParts, usage_seat_mails};
pub const WORK_QUEUE_SNAPSHOTS: &str = "work-queue.elsewhere";
