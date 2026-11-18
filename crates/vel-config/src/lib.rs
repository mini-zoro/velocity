//! Clean-room Sublime resource formats, layered settings and indexed key resolution.
#![forbid(unsafe_code)]
pub mod json;
pub mod keymap;
pub mod resources;
pub mod settings;
mod settings_source;
pub mod ui;
pub use serde_json::{Value, json};
