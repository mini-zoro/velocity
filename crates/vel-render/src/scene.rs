//! Scene primitives in logical points, grouped by kind within explicit layers.
use std::sync::Arc;
use vel_platform::Rect;

/// Straight-alpha linear RGBA.
pub type Color = [f32; 4];
/// Face traits requested by a token. Glyph/layout caches include both flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FontStyle {
    /// Select a bold face through CoreText.
    pub bold: bool,
    /// Select an italic face through CoreText.
    pub italic: bool,
}
/// A solid or rounded rectangle.
#[derive(Clone, Debug)]
pub struct SolidRect {
    /// Bounds in logical points.
    pub rect: Rect,
    /// Corner radius in points; zero is solid. Reserved negative style tags are
    /// emitted only by [`SolidRect::stippled_guide`] and [`SolidRect::outline`].
    pub radius: f32,
    /// Fill color.
    pub color: Color,
    /// Scissor bounds, in logical points.
    pub clip: Rect,
}
impl SolidRect {
    /// One vertical stippled guide. The reserved negative radius is a compact
    /// style tag, so existing scene translation/clipping and rectangle batches
    /// carry the stroke without another primitive array or draw call.
    pub fn stippled_guide(rect: Rect, clip: Rect, color: Color) -> Self {
        Self {
            rect,
            clip,
            color,
            radius: -1.,
        }
    }
    /// A ruler sharing the existing stippled-guide primitive; no extra draw batch.
    pub fn patterned_ruler(rect: Rect, clip: Rect, color: Color, dashed: bool) -> Self {
        Self {
            rect,
            clip,
            color,
            radius: if dashed { -3. } else { -1. },
        }
    }
    /// A one-point outline carried by a single rectangle instance.
    pub fn outline(rect: Rect, clip: Rect, color: Color) -> Self {
        Self {
            rect,
            clip,
            color,
            radius: -2.,
        }
    }
    /// Whether this rectangle carries the reserved outline style tag.
    pub fn is_outline(&self) -> bool {
        self.radius == -2.
    }
    /// Whether this rectangle carries the reserved vertical-guide style tag.
    pub fn is_stippled_guide(&self) -> bool {
        self.radius == -1. || self.radius == -3.
    }
}
/// A single-baseline text run; tab expansion belongs to the UI layout.
#[derive(Clone, Debug)]
pub struct TextRun {
    /// CSS/theme family list. None retains the editor monospace grid.
    pub family: Option<Arc<str>>,
    /// Requested weight and slant.
    pub style: FontStyle,
    /// Text to shape, with no tabs or newlines.
    pub text: String,
    /// Left baseline origin in logical points.
    pub x: f32,
    /// Baseline in logical points.
    pub baseline: f32,
    /// Font size in points.
    pub size: f32,
    /// Foreground; color emoji retain their original pixels.
    pub color: Color,
    /// Scissor bounds.
    pub clip: Rect,
}
/// Straight underline or sinusoidal diagnostic squiggle.
#[derive(Clone, Debug)]
pub struct Underline {
    /// Dotted stroke. Takes precedence over a wave.
    pub stippled: bool,
    /// Bounds; height is amplitude for a squiggle.
    pub rect: Rect,
    /// Stroke thickness in points.
    pub thickness: f32,
    /// Whether to use a wave instead of a straight line.
    pub squiggle: bool,
    /// Stroke color.
    pub color: Color,
    /// Scissor bounds.
    pub clip: Rect,
}
/// A soft rounded box shadow.
#[derive(Clone, Debug)]
pub struct Shadow {
    /// Outer shadow bounds, including blur.
    pub rect: Rect,
    /// Corner radius in points.
    pub radius: f32,
    /// Blur width in points.
    pub blur: f32,
    /// Shadow color.
    pub color: Color,
    /// Scissor bounds.
    pub clip: Rect,
}
/// Back-to-front primitive groups: shadows, solids, glyphs, underlines.
/// Use separate layers when a later solid must cover earlier glyphs.
#[derive(Clone, Debug, Default)]
pub struct Layer {
    /// Ordered theme fills/textures, behind selections and text.
    pub surfaces: Vec<Surface>,
    /// Editor shaping/raster switches, absent for UI text.
    pub font_options: crate::typography::FontOptions,
    /// Text runs with glow, painted beneath normal glyphs only when present.
    pub glow: Vec<TextRun>,
    /// Connected selection fill and border; empty on the normal caret-only path.
    pub contours: Vec<crate::selection::Contour>,
    /// Marks the overview layer for isolated GPU timestamp measurements.
    pub minimap: bool,
    /// Shadows behind other primitives.
    pub shadows: Vec<Shadow>,
    /// Selection, gutter, caret and rounded panels.
    pub rects: Vec<SolidRect>,
    /// Baseline text runs.
    pub text: Vec<TextRun>,
    /// Preedit and diagnostic decorations.
    pub underlines: Vec<Underline>,
}
/// A complete, reusable frame description in logical coordinates.
#[derive(Clone, Debug)]
pub struct Scene {
    /// Clear color.
    pub background: Color,
    /// Layers in painter order.
    pub layers: Vec<Layer>,
}
impl Default for Scene {
    fn default() -> Self {
        Self {
            background: [0.075, 0.085, 0.105, 1.],
            layers: Vec::new(),
        }
    }
}
/// Primitive kinds in draw order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimitiveKind {
    /// Ordered fills and theme image patches.
    Surface,
    /// Soft shadow.
    Shadow,
    /// Solid/rounded rectangle.
    Rect,
    /// Textured glyph.
    Glyph,
    /// Straight/wavy underline.
    Underline,
}
/// One instanced draw range. Atlas pages share a texture array, so all glyphs
/// within one layer use one draw even across page/color boundaries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    /// Index of the scene layer.
    pub layer: usize,
    /// Shader branch.
    pub kind: PrimitiveKind,
    /// Half-open instance range.
    pub instances: std::ops::Range<u32>,
}

/// Device-free draw plan enforcing contiguous instances and one batch per kind
/// per layer. Texture-array pages do not introduce extra glyph batches.
#[derive(Clone, Debug, Default)]
pub struct BatchPlan {
    batches: Vec<Batch>,
}
impl BatchPlan {
    /// Drops ranges while retaining their allocation for the next scene.
    pub fn clear(&mut self) {
        self.batches.clear();
    }
    /// Adds a nonempty draw range. Malformed order is rejected before GPU encoding.
    pub fn push(
        &mut self,
        layer: usize,
        kind: PrimitiveKind,
        instances: std::ops::Range<u32>,
    ) -> Result<(), &'static str> {
        if instances.start > instances.end {
            return Err("reversed instance range");
        }
        if instances.is_empty() {
            return Ok(());
        }
        let expected = self.batches.last().map_or(0, |b| b.instances.end);
        if instances.start != expected {
            return Err("instance ranges must be contiguous");
        }
        if self.batches.last().is_some_and(|b| layer < b.layer) {
            return Err("layers must follow painter order");
        }
        if self
            .batches
            .iter()
            .rev()
            .take_while(|b| b.layer == layer)
            .any(|b| b.kind == kind)
        {
            return Err("a kind may only have one batch per layer");
        }
        self.batches.push(Batch {
            layer,
            kind,
            instances,
        });
        Ok(())
    }
    /// Final draw ranges, in painter order.
    pub fn as_slice(&self) -> &[Batch] {
        &self.batches
    }
    /// Number of actual draw calls.
    pub fn len(&self) -> usize {
        self.batches.len()
    }
    /// Whether this scene needs no draws.
    pub fn is_empty(&self) -> bool {
        self.batches.is_empty()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pages_share_one_glyph_draw_and_caret_keeps_painter_order() {
        let mut plan = BatchPlan::default();
        plan.push(0, PrimitiveKind::Shadow, 0..0).unwrap();
        plan.push(0, PrimitiveKind::Rect, 0..5).unwrap();
        // Fifty glyphs may span any number of array layers; their batch stays one.
        plan.push(0, PrimitiveKind::Glyph, 5..55).unwrap();
        plan.push(0, PrimitiveKind::Underline, 55..57).unwrap();
        plan.push(1, PrimitiveKind::Rect, 57..58).unwrap();
        assert_eq!(plan.len(), 4);
        assert_eq!(plan.as_slice()[3].layer, 1);
        assert_eq!(plan.as_slice()[3].instances, 57..58);
        assert!(plan.push(1, PrimitiveKind::Rect, 58..60).is_err());
        assert!(plan.push(0, PrimitiveKind::Glyph, 58..60).is_err());
        assert!(plan.push(2, PrimitiveKind::Glyph, 59..60).is_err());
        assert_eq!(plan.len(), 4);
        plan.clear();
        assert!(plan.is_empty());
    }
}

/// Immutable straight-alpha image shared by CPU scenes and the GPU atlas.
#[derive(Clone, Debug)]
pub struct Image {
    /// Intrinsic logical dimensions; a decoded HTML image may use a downsampled bitmap.
    pub logical_size: [f32; 2],
    /// Content identity; hosts must change this when pixels change.
    pub id: u64,
    /// Physical pixel width.
    pub width: u32,
    /// Physical pixel height.
    pub height: u32,
    /// Tightly packed RGBA8, top row first.
    pub rgba: Arc<[u8]>,
}
/// One clipped image patch with independently tinted pixels.
#[derive(Clone, Debug)]
pub struct ImageQuad {
    /// Shared atlas resource.
    pub image: Arc<Image>,
    /// Source pixels (left, top, width, height).
    pub source: [f32; 4],
    /// Logical destination.
    pub rect: Rect,
    /// Logical clip.
    pub clip: Rect,
    /// Multiplicative RGBA tint.
    pub color: Color,
}
/// Ordered surface primitive. Separate layers preserve overlap with text.
#[derive(Clone, Debug)]
pub enum Surface {
    /// CSS box, including corner-specific radii and border edges.
    CssBox(CssBox),
    /// Solid or rounded fill.
    Fill(SolidRect),
    /// Atlas image patch.
    Image(ImageQuad),
}
impl Surface {
    /// Mutable destination and clipping bounds for coordinate transforms.
    pub fn bounds_mut(&mut self) -> (&mut Rect, &mut Rect) {
        match self {
            Self::CssBox(r) => (&mut r.rect, &mut r.clip),
            Self::Fill(r) => (&mut r.rect, &mut r.clip),
            Self::Image(r) => (&mut r.rect, &mut r.clip),
        }
    }
}

/// One CSS border edge, ordered top/right/bottom/left.
#[derive(Clone, Copy, Debug, Default)]
pub struct BorderEdge {
    /// Width in logical points.
    pub width: f32,
    /// Straight-alpha edge color.
    pub color: Color,
}
/// A minihtml box with independently rounded corners and colored border edges.
#[derive(Clone, Debug)]
pub struct CssBox {
    /// Logical outer border box.
    pub rect: Rect,
    /// Ancestor clipping rectangle.
    pub clip: Rect,
    /// Background behind the content and border.
    pub background: Color,
    /// Top-left, top-right, bottom-right, bottom-left radii.
    pub radii: [f32; 4],
    /// Top, right, bottom, left border edges.
    pub borders: [BorderEdge; 4],
}
