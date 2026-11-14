//! Scene primitives in logical points, grouped by kind within explicit layers.
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
    /// Corner radius in points; zero is a solid rectangle.
    pub radius: f32,
    /// Fill color.
    pub color: Color,
    /// Scissor bounds, in logical points.
    pub clip: Rect,
}
/// A single-baseline text run; tab expansion belongs to the UI layout.
#[derive(Clone, Debug)]
pub struct TextRun {
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
