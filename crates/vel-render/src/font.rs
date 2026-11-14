//! Adapted from Velocity’s MIT OR Apache-2.0 P0 prototype.
//! Menlo's ASCII fast path, lazy system fallbacks, swash shaping and rasterization.
use crate::native;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use swash::{
    CacheKey, FontDataRef, FontRef, StringId,
    scale::{
        Render, ScaleContext, Source, StrikeWith,
        image::{Content, Image},
    },
    shape::ShapeContext,
    text::Codepoint,
    zeno::{Format, Vector},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Debug)]
struct Face {
    bytes: memmap2::Mmap,
    offset: u32,
    key: CacheKey,
    path: PathBuf,
    name: String,
    native: Option<native::NativeFont>,
}
impl Face {
    fn load(path: &Path, name: &str, native: Option<native::NativeFont>) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        // SAFETY: only macOS system font files on the sealed/read-only system volume are
        // mapped; their bytes cannot be truncated or changed during this Font lifetime.
        let bytes = unsafe { memmap2::MmapOptions::new().map(&file) }.map_err(|e| e.to_string())?;
        let collection = FontDataRef::new(&bytes)
            .ok_or_else(|| format!("{} is not a supported font", path.display()))?;
        let font = collection
            .fonts()
            .find(|f| {
                f.localized_strings()
                    .any(|s| s.id() == StringId::PostScript && s.to_string() == name)
            })
            .or_else(|| collection.get(0))
            .ok_or("empty font collection")?;
        let actual = font
            .localized_strings()
            .find(|s| s.id() == StringId::PostScript)
            .map(|s| s.to_string())
            .unwrap_or_else(|| name.into());
        let (offset, key) = (font.offset, font.key);
        Ok(Self {
            bytes,
            offset,
            key,
            path: path.into(),
            name: actual,
            native,
        })
    }
    fn reference(&self) -> FontRef<'_> {
        FontRef {
            data: &self.bytes,
            offset: self.offset,
            key: self.key,
        }
    }
}
#[derive(Clone, Copy, Debug)]
/// Face-local glyph positioned relative to a run baseline, in physical pixels.
pub struct ShapedGlyph {
    /// Font face index.
    pub face: u16,
    /// Face-local glyph id.
    pub id: u16,
    /// Horizontal baseline offset.
    pub x: f32,
    /// Vertical baseline offset.
    pub y: f32,
}

/// Lazy system fonts and reusable swash shaping/raster contexts.
pub struct Fonts {
    variants: HashMap<(u16, crate::scene::FontStyle), u16>,
    faces: Vec<Face>,
    fallbacks_loaded: bool,
    /// Missing-face/glyph diagnostics.
    pub warnings: Vec<String>,
    coretext_rasters: usize,
    coretext_faces: std::collections::HashSet<u16>,
    scaler: ScaleContext,
    shaper: ShapeContext,
    shaped: HashMap<(String, u32, crate::scene::FontStyle), Vec<ShapedGlyph>>,
    ascii: [u16; 128],
    /// Primary font advance at 13 points.
    pub cell: f32,
    /// Primary ascent at 13 points.
    pub ascent: f32,
}
impl Fonts {
    /// Loads only the primary monospace face; system fallbacks are lazy.
    pub fn load() -> Result<Self, String> {
        Self::load_named("Menlo")
    }
    /// Loads a requested primary face through CoreText, retaining lazy fallbacks.
    pub fn load_named(requested: &str) -> Result<Self, String> {
        let primary = Path::new("/System/Library/Fonts/Menlo.ttc");
        let face = if requested == "Menlo" && primary.exists() {
            Face::load(primary, "Menlo-Regular", None)?
        } else {
            let (path, name, font) = native::font_location(requested)?;
            Face::load(&path, &name, Some(font))?
        };
        let font = face.reference();
        let metrics = font.metrics(&[]).scale(13.);
        let ascii = std::array::from_fn(|i| font.charmap().map(char::from(i as u8)));
        let cell = font
            .glyph_metrics(&[])
            .scale(13.)
            .advance_width(ascii[b'M' as usize]);
        Ok(Self {
            variants: HashMap::new(),
            faces: vec![face],
            fallbacks_loaded: false,
            warnings: Vec::new(),
            coretext_rasters: 0,
            coretext_faces: std::collections::HashSet::new(),
            scaler: ScaleContext::new(),
            shaper: ShapeContext::new(),
            shaped: HashMap::new(),
            ascii,
            cell,
            ascent: metrics.ascent,
        })
    }
    fn load_fallbacks(&mut self) {
        if self.fallbacks_loaded {
            return;
        }
        self.fallbacks_loaded = true;
        for requested in ["PingFangSC-Regular", "AppleColorEmoji"] {
            match native::font_location(requested).and_then(|(p, n, f)| Face::load(&p, &n, Some(f)))
            {
                Ok(face) => self.faces.push(face),
                Err(e) => self.warnings.push(e),
            }
        }
    }
    /// Font identities, raster fallback counts and diagnostics.
    pub fn metadata(&self) -> Value {
        json!({"rasterizer":"swash with CoreText fallback when a glyph has no swash image","coretext_fallback_raster_count":self.coretext_rasters,"point_size":13,"cell_points":self.cell,"ascent_points":self.ascent,"fallbacks_loaded_lazily":true,"faces":self.faces.iter().map(|f|json!({"path":f.path,"postscript_name":f.name,"mapped_font_file_bytes":f.bytes.len()})).collect::<Vec<_>>(),"warnings":self.warnings})
    }
    /// ASCII skips shaping; non-ASCII graphemes use a cached swash shaping path.
    pub fn shape(&mut self, text: &str, px: f32) -> Vec<ShapedGlyph> {
        self.shape_styled(text, px, crate::scene::FontStyle::default())
    }
    fn variant(&mut self, base: u16, style: crate::scene::FontStyle) -> u16 {
        if style == crate::scene::FontStyle::default() {
            return base;
        }
        if let Some(face) = self.variants.get(&(base, style)) {
            return *face;
        }
        let name = self.faces[base as usize].name.clone();
        let face = match native::font_variant(&name, style)
            .and_then(|(p, n, f)| Face::load(&p, &n, Some(f)))
        {
            Ok(face) => {
                let index = self.faces.len() as u16;
                self.faces.push(face);
                index
            }
            Err(e) => {
                self.warnings.push(format!("{name}: {e}"));
                base
            }
        };
        self.variants.insert((base, style), face);
        face
    }
    /// Shapes using actual face variants, including each non-ASCII fallback family.
    pub fn shape_styled(
        &mut self,
        text: &str,
        px: f32,
        style: crate::scene::FontStyle,
    ) -> Vec<ShapedGlyph> {
        if text.is_ascii() {
            let face = self.variant(0, style);
            return text
                .bytes()
                .enumerate()
                .map(|(i, b)| ShapedGlyph {
                    face,
                    id: if face == 0 {
                        self.ascii[b as usize]
                    } else {
                        self.faces[face as usize]
                            .reference()
                            .charmap()
                            .map(char::from(b))
                    },
                    x: i as f32 * self.cell * px / 13.,
                    y: 0.,
                })
                .collect();
        }
        let key = (text.to_owned(), (px * 64.).round() as u32, style);
        if let Some(g) = self.shaped.get(&key) {
            return g.clone();
        }
        self.load_fallbacks();
        // Keep each grapheme intact during fallback, then shape adjacent
        // graphemes of the same face as a run (Kana/Arabic/Indic contextual forms).
        let mut runs: Vec<(usize, usize, u16)> = Vec::new();
        for (start, cluster) in text.grapheme_indices(true) {
            let covers = |face: &Face| {
                cluster
                    .chars()
                    .filter(|c| !matches!(c, '\u{fe0e}' | '\u{fe0f}' | '\u{200d}'))
                    .all(|c| face.reference().charmap().map(c) != 0)
            };
            let emoji = cluster
                .chars()
                .any(|c| c as u32 >= 0x1f000 || c == '\u{fe0f}');
            let face = if emoji {
                self.faces
                    .iter()
                    .enumerate()
                    .find(|(_, f)| f.name.contains("Emoji") && covers(f))
                    .map(|(i, _)| i)
            } else {
                None
            }
            .or_else(|| self.faces.iter().position(covers))
            .unwrap_or(0) as u16;
            let face = self.variant(face, style);
            let end = start + cluster.len();
            if let Some(previous) = runs.last_mut()
                && previous.2 == face
            {
                previous.1 = end;
            } else {
                runs.push((start, end, face));
            }
        }
        let cell = self.cell * px / 13.;
        let columns = |s: &str| s.graphemes(true).map(|g| g.width().max(1)).sum::<usize>() as f32;
        let mut glyphs = Vec::new();
        for (start, end, face) in runs {
            let run = &text[start..end];
            let reference = self.faces[face as usize].reference();
            let mut builder = self.shaper.builder(reference).size(px);
            if let Some(c) = run
                .chars()
                .find(|c| !c.is_ascii_punctuation() && !c.is_whitespace())
            {
                builder = builder.script(c.script());
            }
            let mut shaper = builder.build();
            shaper.add_str(run);
            let run_x = columns(&text[..start]) * cell;
            shaper.shape_with(|cluster| {
                let from = cluster.source.start as usize;
                let to = cluster.source.end as usize;
                let x = run_x + columns(&run[..from]) * cell;
                let advance = cluster.advance();
                let target = columns(&run[from..to]) * cell;
                let factor = if advance > 0. { target / advance } else { 1. };
                let mut pen = 0.;
                for g in cluster.glyphs {
                    glyphs.push(ShapedGlyph {
                        face,
                        id: g.id,
                        x: x + pen * factor + g.x,
                        y: g.y,
                    });
                    pen += g.advance;
                }
            });
        }
        if glyphs.iter().any(|g| g.id == 0) && self.warnings.len() < 32 {
            self.warnings
                .push(format!("missing glyph in {text:?}; rendered .notdef"));
        }
        if self.shaped.len() >= 1024 {
            self.shaped.clear();
        }
        self.shaped.insert(key, glyphs.clone());
        glyphs
    }
    /// Rasterizes one glyph at a quarter-pixel physical phase.
    pub fn rasterize(&mut self, face: u16, id: u16, px: f32, bucket: u8) -> Option<Image> {
        let font = self.faces[face as usize].reference();
        if id != 0 && id == font.charmap().map(' ') {
            return None;
        }
        let mut scaler = self.scaler.builder(font).size(px).hint(true).build();
        let image = Render::new(&[
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
        ])
        .format(Format::Alpha)
        .offset(Vector::new(bucket as f32 * 0.25, 0.))
        .render(&mut scaler, id);
        if let Some(image) = image.filter(|i| i.placement.width > 0 && i.placement.height > 0) {
            return Some(image);
        }
        let native_font = match &self.faces[face as usize].native {
            Some(f) => f.clone(),
            None => match native::font_location(&self.faces[face as usize].name) {
                Ok((_, _, f)) => {
                    self.faces[face as usize].native = Some(f.clone());
                    f
                }
                Err(e) => {
                    self.warnings.push(e);
                    return None;
                }
            },
        };
        match native::raster_glyph(&native_font, id, px, bucket) {
            Ok(image) => {
                self.coretext_rasters += 1;
                if self.coretext_faces.insert(face) {
                    self.warnings.push(format!(
                        "swash image unavailable for {}; using CoreText raster fallback",
                        self.faces[face as usize].name
                    ));
                }
                Some(image)
            }
            Err(e) => {
                if self.warnings.len() < 32 {
                    self.warnings.push(e);
                }
                None
            }
        }
    }
}
/// Use straight-alpha RGBA for masks. Swash color bitmaps are retained as delivered.
pub fn rgba(image: &Image) -> Vec<u8> {
    match image.content {
        Content::Mask => image
            .data
            .iter()
            .flat_map(|a| [255, 255, 255, *a])
            .collect(),
        Content::Color => image.data.clone(),
        Content::SubpixelMask => image
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [255, 255, 255, p[0].max(p[1]).max(p[2])])
            .collect(),
    }
}

impl std::fmt::Debug for Fonts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fonts")
            .field("faces", &self.faces)
            .field("cell", &self.cell)
            .field("warnings", &self.warnings)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn bold_italic_use_distinct_real_faces_without_changing_grid_advance() {
        let mut fonts = super::Fonts::load().unwrap();
        let mut faces = std::collections::BTreeSet::new();
        for (bold, italic) in [(false, false), (true, false), (false, true), (true, true)] {
            let glyphs = fonts.shape_styled("Mono", 13., crate::scene::FontStyle { bold, italic });
            assert_eq!(glyphs.len(), 4);
            assert!((glyphs[3].x - fonts.cell * 3.).abs() < 0.001);
            faces.insert(glyphs[0].face);
            assert!(
                fonts
                    .rasterize(glyphs[0].face, glyphs[0].id, 13., 0)
                    .is_some()
            );
        }
        assert_eq!(faces.len(), 4);
    }
    use super::*;
    #[test]
    fn ascii_fast_path_mixed_fallback_and_coretext_bitmap_are_headless() {
        let mut fonts = Fonts::load().unwrap();
        let ascii = fonts.shape("ABC", 26.);
        assert_eq!(ascii.len(), 3);
        assert!((ascii[1].x - fonts.cell * 2.).abs() < 0.001);
        let mixed = fonts.shape("a你好🦀é", 26.);
        assert!(mixed.iter().all(|g| g.id != 0));
        assert!(mixed.iter().any(|g| g.face != 0));
        for text in ["你", "😀"] {
            for glyph in fonts.shape(text, 26.) {
                assert!(
                    fonts.rasterize(glyph.face, glyph.id, 26., 0).is_some(),
                    "missing fallback bitmap for {text}"
                );
            }
        }
        let f = fonts.shape("F", 40.)[0];
        let (_, _, native) = native::font_location("Menlo-Regular").unwrap();
        let image = native::raster_glyph(&native, f.id, 40., 0).unwrap();
        let width = image.placement.width as usize;
        let height = image.placement.height as usize;
        let third = (height / 3).max(1);
        let upper: u64 = image.data[..third * width]
            .iter()
            .map(|v| u64::from(*v))
            .sum();
        let lower: u64 = image.data[(height - third) * width..]
            .iter()
            .map(|v| u64::from(*v))
            .sum();
        assert!(upper > lower, "CoreText F must be top-down");
        assert_eq!(rgba(&image).len(), width * height * 4);
    }
}
