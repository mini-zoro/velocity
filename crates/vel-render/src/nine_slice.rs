//! Device-free nine-slice placement with density-correct corners and bounded tiling.
use crate::scene::{Color, Image, ImageQuad, Layer, Surface};
use std::sync::Arc;
use vel_platform::Rect;

/// Image slicing parameters in logical points, in left/top/right/bottom order.
#[derive(Clone, Copy, Debug)]
pub struct Slices {
    /// Source density divisor.
    pub density: f32,
    /// UI magnification of destination borders/tiles, independent of source backing density.
    pub scale: f32,
    /// Insets in logical points.
    pub margins: [f32; 4],
    /// Paint the center as well as edges/corners.
    pub center: bool,
    /// Tile edge/center patches, cropping the final tile.
    pub repeat: bool,
}
/// Appends atlas patches. Small destinations shrink opposing borders proportionally.
/// Tiling is clipped to the viewport and capped at 4096 patches per call.
pub fn paint(
    layer: &mut Layer,
    image: &Arc<Image>,
    rect: Rect,
    clip: Rect,
    color: Color,
    slices: Slices,
) {
    if rect.width <= 0. || rect.height <= 0. {
        return;
    }
    let d = slices.density.max(1.);
    let w = image.width as f32 / d;
    let h = image.height as f32 / d;
    let [l, t, r, b] = slices
        .margins
        .map(|n| if n.is_finite() { n.max(0.) } else { 0. });
    let fit = |a: f32, b: f32, size: f32| {
        let f = (size / (a + b).max(f32::EPSILON)).min(1.);
        (a * f, b * f)
    };
    let (l, r) = fit(l, r, w);
    let (t, b) = fit(t, b, h);
    let scale = if slices.scale.is_finite() {
        slices.scale.max(0.01)
    } else {
        1.
    };
    let (dl, dr) = fit(l * scale, r * scale, rect.width as f32);
    let (dt, db) = fit(t * scale, b * scale, rect.height as f32);
    let sx = [0., l * d, (w - r) * d, image.width as f32];
    let sy = [0., t * d, (h - b) * d, image.height as f32];
    let dx = [
        rect.x,
        rect.x + f64::from(dl),
        rect.x + rect.width - f64::from(dr),
        rect.x + rect.width,
    ];
    let dy = [
        rect.y,
        rect.y + f64::from(dt),
        rect.y + rect.height - f64::from(db),
        rect.y + rect.height,
    ];
    let mut count = 0;
    for row in 0..3 {
        for col in 0..3 {
            if !slices.center && row == 1 && col == 1 {
                continue;
            }
            let sw = sx[col + 1] - sx[col];
            let sh = sy[row + 1] - sy[row];
            let dw = dx[col + 1] - dx[col];
            let dh = dy[row + 1] - dy[row];
            if sw <= 0. || sh <= 0. || dw <= 0. || dh <= 0. {
                continue;
            }
            let tw = if slices.repeat && col == 1 {
                f64::from(sw / d * scale)
            } else {
                dw
            };
            let th = if slices.repeat && row == 1 {
                f64::from(sh / d * scale)
            } else {
                dh
            };
            let first_x = ((clip.x - dx[col]) / tw).floor().max(0.) as usize;
            let first_y = ((clip.y - dy[row]) / th).floor().max(0.) as usize;
            let nx = (dw / tw).ceil().min(4096.) as usize;
            let ny = (dh / th).ceil().min(4096.) as usize;
            for iy in first_y..ny {
                let y = dy[row] + iy as f64 * th;
                if y >= clip.y + clip.height {
                    break;
                }
                for ix in first_x..nx {
                    let x = dx[col] + ix as f64 * tw;
                    if x >= clip.x + clip.width {
                        break;
                    }
                    if count == 4096 {
                        return;
                    }
                    let width = tw.min(dx[col + 1] - x);
                    let height = th.min(dy[row + 1] - y);
                    layer.surfaces.push(Surface::Image(ImageQuad {
                        image: image.clone(),
                        source: [
                            sx[col],
                            sy[row],
                            sw * (width / tw) as f32,
                            sh * (height / th) as f32,
                        ],
                        rect: Rect {
                            x,
                            y,
                            width,
                            height,
                        },
                        clip,
                        color,
                    }));
                    count += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn density_corners_small_destinations_and_cropped_tiles() {
        let image = Arc::new(Image {
            logical_size: [12.; 2],
            id: 1,
            width: 24,
            height: 24,
            rgba: vec![255; 24 * 24 * 4].into(),
        });
        let mut layer = Layer::default();
        let rect = Rect {
            x: 0.,
            y: 0.,
            width: 25.,
            height: 25.,
        };
        paint(
            &mut layer,
            &image,
            rect,
            rect,
            [1.; 4],
            Slices {
                density: 2.,
                scale: 2.,
                margins: [2.; 4],
                center: true,
                repeat: true,
            },
        );
        let Surface::Image(first) = &layer.surfaces[0] else {
            panic!()
        };
        assert_eq!(first.source, [0., 0., 4., 4.]);
        assert_eq!(first.rect.width, 4.);
        assert!(layer.surfaces.iter().all(|s|matches!(s,Surface::Image(q) if q.rect.x+q.rect.width<=25. && q.rect.y+q.rect.height<=25.)));
        layer.surfaces.clear();
        let small = Rect {
            width: 2.,
            height: 2.,
            ..rect
        };
        paint(
            &mut layer,
            &image,
            small,
            small,
            [1.; 4],
            Slices {
                density: 2.,
                scale: 1.,
                margins: [4.; 4],
                center: false,
                repeat: false,
            },
        );
        assert_eq!(layer.surfaces.len(), 4);
    }
}
