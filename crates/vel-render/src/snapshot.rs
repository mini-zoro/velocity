//! Device-free text geometry snapshots. Flat fills and glyphs only: not a
//! substitute for the Metal renderer when reviewing theme textures or effects.
use crate::{
    atlas::subpixel_x,
    font::{Fonts, ShapedGlyph},
    scene::{Color, Layer, Scene, Surface, TextRun},
};
use std::time::{Duration, Instant};
use swash::scale::image::Content;
use vel_platform::Rect;

/// Composites editor text, carets, guides, selection fills and straight underlines for visual QA
/// on hosts where no Metal adapter is available.
pub fn text_rgba(
    scene: &Scene,
    fonts: &mut Fonts,
    width: u32,
    height: u32,
    scale: f32,
) -> Result<Vec<u8>, String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let shape = |fonts: &mut Fonts, layer: &Layer, index: usize, run: &TextRun| {
        let px = (run.size * scale * 64.).round() / 64.;
        if let Some(family) = &run.family {
            fonts.shape_ui(&run.text, family, px, run.style).glyphs
        } else {
            fonts.shape_editor(
                &run.text,
                px,
                run.style,
                layer.wide_run_advances(index, false),
            )
        }
    };
    let layouts: Vec<Vec<Vec<ShapedGlyph>>> = loop {
        let shaped = scene
            .layers
            .iter()
            .map(|layer| {
                fonts.set_options(layer.font_options);
                layer
                    .text
                    .iter()
                    .enumerate()
                    .map(|(i, run)| shape(fonts, layer, i, run))
                    .collect()
            })
            .collect();
        if !fonts.fallback_pending() {
            break shaped;
        }
        while fonts.fallback_pending() {
            if Instant::now() > deadline {
                return Err("snapshot font fallback did not settle".into());
            }
            if fonts.poll_fallbacks() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    };

    let mut image = vec![scene.background; (width * height) as usize];
    let scale64 = f64::from(scale);
    let clip = |rect: Rect| {
        let x = (rect.x * scale64).floor().clamp(0., f64::from(width)) as i32;
        let y = (rect.y * scale64).floor().clamp(0., f64::from(height)) as i32;
        let right = ((rect.x + rect.width) * scale64)
            .ceil()
            .clamp(0., f64::from(width)) as i32;
        let bottom = ((rect.y + rect.height) * scale64)
            .ceil()
            .clamp(0., f64::from(height)) as i32;
        (x..right, y..bottom)
    };
    let linear = |n: u8| {
        let x = f32::from(n) / 255.;
        if x <= 0.04045 {
            x / 12.92
        } else {
            ((x + 0.055) / 1.055).powf(2.4)
        }
    };
    let index = |x: i32, y: i32| y as usize * width as usize + x as usize;
    for (layer, shapes) in scene.layers.iter().zip(layouts) {
        {
            let mut fill = |rect: Rect, scissor: Rect, color: Color, radius: f32| {
                let (x, y) = clip(rect);
                let (sx, sy) = clip(scissor);
                for row in y.start.max(sy.start)..y.end.min(sy.end) {
                    for col in x.start.max(sx.start)..x.end.min(sx.end) {
                        if (radius == -1. || radius == -3.) && row % 4 >= 2 {
                            continue;
                        }
                        if radius == -2.
                            && col > x.start
                            && col + 1 < x.end
                            && row > y.start
                            && row + 1 < y.end
                        {
                            continue;
                        }
                        over(&mut image[index(col, row)], color);
                    }
                }
            };
            for surface in &layer.surfaces {
                if let Surface::Fill(r) = surface {
                    fill(r.rect, r.clip, r.color, r.radius);
                }
            }
            for r in &layer.rects {
                fill(r.rect, r.clip, r.color, r.radius);
            }
        }
        for contour in &layer.contours {
            let (xs, ys) = clip(contour.clip);
            let points = &contour.points;
            for y in ys {
                let py = (f64::from(y) + 0.5) / scale64 - contour.offset.y;
                let mut crossings = Vec::new();
                for i in 0..points.len() {
                    let a = points[i];
                    let b = points[(i + 1) % points.len()];
                    if (a.y > py) != (b.y > py) {
                        crossings.push(a.x + (b.x - a.x) * (py - a.y) / (b.y - a.y));
                    }
                }
                crossings.sort_by(f64::total_cmp);
                for pair in crossings.as_chunks::<2>().0 {
                    let pixel = |x: f64| ((x + contour.offset.x) * scale64 - 0.5).ceil() as i32;
                    for x in pixel(pair[0]).max(xs.start)..pixel(pair[1]).min(xs.end) {
                        over(&mut image[index(x, y)], contour.fill);
                    }
                }
            }
        }
        for (run, glyphs) in layer.text.iter().zip(shapes) {
            let (xs, ys) = clip(run.clip);
            let px = (run.size * scale * 64.).round() / 64.;
            for glyph in glyphs {
                let (x, bucket) = subpixel_x(run.x * scale + glyph.x);
                let Some(raster) = fonts.rasterize(glyph.face, glyph.id, px, bucket) else {
                    continue;
                };
                let p = raster.placement;
                let left = x as i32 + p.left;
                let top = ((run.baseline * scale).round() - glyph.y).round() as i32 - p.top;
                let color = run
                    .highlight
                    .as_ref()
                    .and_then(|h| h.color_at(glyph.source))
                    .unwrap_or(run.color);
                for y in top.max(ys.start)..(top + p.height as i32).min(ys.end) {
                    for x in left.max(xs.start)..(left + p.width as i32).min(xs.end) {
                        let i = (y - top) as usize * p.width as usize + (x - left) as usize;
                        let mut c = color;
                        match raster.content {
                            Content::Color => {
                                let p = &raster.data[i * 4..][..4];
                                c = [
                                    linear(p[0]),
                                    linear(p[1]),
                                    linear(p[2]),
                                    c[3] * f32::from(p[3]) / 255.,
                                ];
                            }
                            Content::Mask => c[3] *= f32::from(raster.data[i]) / 255.,
                            Content::SubpixelMask => {
                                c[3] *= f32::from(
                                    raster.data[i * 4..][..3].iter().copied().max().unwrap_or(0),
                                ) / 255.
                            }
                        }
                        over(&mut image[index(x, y)], c);
                    }
                }
            }
        }
        for underline in layer
            .underlines
            .iter()
            .filter(|u| !u.squiggle && !u.stippled)
        {
            let (x, y) = clip(underline.rect);
            let (sx, sy) = clip(underline.clip);
            for row in y.start.max(sy.start)..y.end.min(sy.end) {
                for col in x.start.max(sx.start)..x.end.min(sx.end) {
                    over(&mut image[index(col, row)], underline.color);
                }
            }
        }
    }
    let srgb = |x: f32| {
        let x = x.clamp(0., 1.);
        ((if x <= 0.0031308 {
            x * 12.92
        } else {
            x.powf(1. / 2.4) * 1.055 - 0.055
        }) * 255.)
            .round() as u8
    };
    Ok(image
        .into_iter()
        .flat_map(|[r, g, b, _]| [srgb(r), srgb(g), srgb(b), 255])
        .collect())
}

fn over(dst: &mut Color, src: Color) {
    for i in 0..3 {
        dst[i] = src[i] * src[3] + dst[i] * (1. - src[3]);
    }
}
