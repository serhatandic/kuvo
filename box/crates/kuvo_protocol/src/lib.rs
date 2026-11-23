//! The shared Rust contract for Kuvo routes. Transport and storage live outside this crate.

mod check;
pub mod gate;
#[doc(hidden)]
pub mod json;
pub mod shape;
pub mod version;

pub use check::{AnswerError, Route, ShapeError, check_answer, check_shape, route};
pub use routes::Refusal;

include!(concat!(env!("OUT_DIR"), "/features.rs"));

/// Generated request and answer types, grouped by method and route pattern.
pub mod routes {
    include!(concat!(env!("OUT_DIR"), "/routes.rs"));
}

#[cfg(test)]
mod tests;
