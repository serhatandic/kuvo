//! Shared routing, the resumable line, and local resident transports.

mod dispatch;
mod line;
mod local;
mod path;
mod resident;
pub mod socket;
pub mod ssh;

pub use dispatch::{
    Access, Body, Caller, DispatchRequest, Dispatcher, JsonLine, LocalOrigin, Pairing, Refusal,
    Registry, Reply, RouteCall, RouteReply, Transport,
};
pub use line::LineTiming;
pub use resident::{Resident, ServeRecord};
