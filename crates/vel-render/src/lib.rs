//! Instanced editor primitives, paged glyph caches and a Metal wgpu renderer.
//! Geometry and cache logic are usable without a GPU. Native rendering and
//! system fonts are gated to macOS; no renderer is instantiated by tests.
pub mod atlas;
pub mod cache;
#[cfg(target_os = "macos")]
pub mod font;
#[cfg(target_os = "macos")]
mod gpu;
#[cfg(target_os = "macos")]
mod native;
pub mod nine_slice;
pub mod scene;
#[cfg(target_os = "macos")]
mod timing;
#[cfg(target_os = "macos")]
pub use gpu::{FrameTimes, Renderer};
#[cfg(not(target_os = "macos"))]
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
