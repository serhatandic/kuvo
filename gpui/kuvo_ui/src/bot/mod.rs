//! A bot, drawn: its look (Pebble), its motion in each state, and the painting of a pose.
//!
//! What a bot is doing is the box's (`bots.now.state`); this only draws it. The app keeps one
//! [`Motion`] per bot and steps it as its copies draw, so every copy of a bot moves as one creature.

pub mod look;
pub mod motion;
pub mod paint;

pub use look::{Shape, swatch};
pub use motion::{Glyph, Motion, Pose, State, seed_of};
pub use paint::paint_bot;
