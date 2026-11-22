//! Native Windows path-component comparison, kept in a leaf crate so that low-level
//! crates (vel-config) need neither `unsafe` nor a dependency on the platform layer.
#![cfg_attr(not(windows), forbid(unsafe_code))]
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::windows_path_component_eq;
