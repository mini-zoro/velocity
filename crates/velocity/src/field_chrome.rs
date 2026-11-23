//! Rounded single-line field chrome shared by inline editors (the tab rename field) and the
//! Settings window: a text background with a low-contrast hairline, and a translucent accent
//! focus ring outside it, as on macOS and Windows 11 fields.
use vel_platform::Rect;
use vel_render::scene::{BorderEdge, Color, CssBox, Layer, Surface};
use vel_ui::theme::ThemeUi;

/// Corner radius of a field, in points.
pub(crate) const RADIUS: f32 = 5.;
/// Width of the focus ring outside a field, in points.
pub(crate) const RING: f64 = 3.;

fn mix(a: Color, b: Color, t: f32) -> Color {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// The accent for fills, selections, carets and focus rings: the system accent where the
/// platform has one, otherwise the theme's.
pub(crate) fn accent(theme: &ThemeUi) -> Color {
    vel_platform::system_accent_color().unwrap_or_else(|| theme.accent())
}

/// Colors of a field drawn over the given background.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FieldColors {
    pub fill: Color,
    pub border: Color,
    pub ring: Color,
    pub selection: Color,
    pub caret: Color,
}

impl FieldColors {
    /// A text background a little lighter (light) or deeper (dark) than `background`, a
    /// hairline from `foreground`, and selection, caret and ring from `accent`.
    pub(crate) fn new(background: Color, foreground: Color, accent: Color, dark: bool) -> Self {
        let fill = if dark {
            mix(background, [0., 0., 0., 1.], 0.32)
        } else {
            mix(background, [1., 1., 1., 1.], 0.88)
        };
        let mut border = foreground;
        border[3] = if dark { 0.24 } else { 0.2 };
        let mut ring = accent;
        ring[3] = 0.5;
        let mut selection = accent;
        selection[3] = if dark { 0.45 } else { 0.3 };
        let mut caret = accent;
        caret[3] = 1.;
        Self {
            fill: [fill[0], fill[1], fill[2], 1.],
            border,
            ring,
            selection,
            caret,
        }
    }
}

/// The field's face: rounded text background with a hairline border.
pub(crate) fn face(layer: &mut Layer, r: Rect, clip: Rect, colors: &FieldColors) {
    layer.surfaces.push(Surface::CssBox(CssBox {
        rect: r,
        clip,
        background: colors.fill,
        radii: [RADIUS; 4],
        borders: [BorderEdge {
            color: colors.border,
            width: 1.,
        }; 4],
    }));
}

/// The keyboard focus ring: a translucent accent band of `RING` points just outside `r`.
pub(crate) fn focus_ring(layer: &mut Layer, r: Rect, clip: Rect, color: Color, radius: f32) {
    layer.surfaces.push(Surface::CssBox(CssBox {
        rect: Rect {
            x: r.x - RING,
            y: r.y - RING,
            width: r.width + 2. * RING,
            height: r.height + 2. * RING,
        },
        clip,
        background: [0.; 4],
        radii: [radius + RING as f32; 4],
        borders: [BorderEdge {
            color,
            width: RING as f32,
        }; 4],
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_colors_follow_the_background_and_accent() {
        let accent = [0.2, 0.4, 0.9, 1.];
        let light = FieldColors::new([0.96, 0.95, 0.92, 1.], [0.1, 0.1, 0.1, 1.], accent, false);
        let dark = FieldColors::new([0.08, 0.1, 0.13, 1.], [0.9, 0.9, 0.9, 1.], accent, true);
        // A text background: lighter than a light strip, deeper than a dark one.
        assert!(light.fill[0] > 0.96 && light.fill[2] > 0.92);
        assert!(dark.fill[2] < 0.13);
        // Low-contrast hairlines; translucent accent selections and rings; opaque caret.
        for colors in [light, dark] {
            assert!(colors.border[3] < 0.3);
            assert_eq!(colors.selection[..3], accent[..3]);
            assert!(colors.selection[3] < 0.5 && colors.ring[3] < 1.);
            assert_eq!(colors.caret, accent);
        }
    }
}
