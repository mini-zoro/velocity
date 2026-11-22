//! Instanced editor primitives, paged glyph caches and a Metal/DX12 wgpu renderer.
//! Geometry, font shaping and cache logic work without a GPU.
pub mod atlas;
pub mod cache;
pub mod fold;
#[cfg(any(target_os = "macos", windows))]
pub mod font;
#[cfg(any(target_os = "macos", windows))]
mod gpu;
pub mod guides;
pub mod invisibles;
#[cfg(any(target_os = "macos", windows))]
mod native;
pub mod nine_slice;
pub mod scene;
pub mod selection;
#[cfg(any(target_os = "macos", windows))]
pub mod snapshot;
#[cfg(any(target_os = "macos", windows))]
mod timing;
pub mod typography;
#[cfg(any(target_os = "macos", windows))]
pub use gpu::{FrameTimes, Renderer};
#[cfg(not(any(target_os = "macos", windows)))]
pub mod font {
    /// Portable glyph layout record for headless cache tests.
    #[derive(Clone, Copy, Debug)]
    pub struct ShapedGlyph {
        /// Face index.
        pub face: u16,
        /// Glyph id.
        pub id: u16,
        /// Horizontal offset in pixels.
        pub x: f32,
        /// Vertical offset in pixels.
        pub y: f32,
        /// UTF-8 byte span of the shaped cluster.
        pub source: [usize; 2],
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn shader_validates_without_a_device() {
        let module = naga::front::wgsl::parse_str(include_str!("quads.wgsl")).expect("parse WGSL");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("validate WGSL");
    }
}
