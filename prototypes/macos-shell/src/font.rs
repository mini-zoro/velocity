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
pub struct ShapedGlyph {
    pub face: u16,
    pub id: u16,
    pub x: f32,
    pub y: f32,
}

pub struct Fonts {
    faces: Vec<Face>,
    fallbacks_loaded: bool,
    pub warnings: Vec<String>,
    coretext_rasters: usize,
    coretext_faces: std::collections::HashSet<u16>,
    scaler: ScaleContext,
    shaper: ShapeContext,
    shaped: HashMap<(String, u32), Vec<ShapedGlyph>>,
    ascii: [u16; 128],
    pub cell: f32,
    pub ascent: f32,
}
impl Fonts {
    pub fn load() -> Result<Self, String> {
        let primary = Path::new("/System/Library/Fonts/Menlo.ttc");
        let face = if primary.exists() {
            Face::load(primary, "Menlo-Regular", None)?
        } else {
            let (path, name, font) = native::font_location("SFMono-Regular")?;
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
    pub fn metadata(&self) -> Value {
        json!({"rasterizer":"swash with CoreText fallback when a glyph has no swash image","coretext_fallback_raster_count":self.coretext_rasters,"point_size":13,"cell_points":self.cell,"ascent_points":self.ascent,"fallbacks_loaded_lazily":true,"faces":self.faces.iter().map(|f|json!({"path":f.path,"postscript_name":f.name,"mapped_font_file_bytes":f.bytes.len()})).collect::<Vec<_>>(),"warnings":self.warnings})
    }
    /// ASCII skips shaping; non-ASCII graphemes use a cached swash shaping path.
    pub fn shape(&mut self, text: &str, px: f32) -> Vec<ShapedGlyph> {
        if text.len() == 1 && text.is_ascii() {
            return vec![ShapedGlyph {
                face: 0,
                id: self.ascii[text.as_bytes()[0] as usize],
                x: 0.,
                y: 0.,
            }];
        }
        let key = (text.to_owned(), (px * 64.).round() as u32);
        if let Some(g) = self.shaped.get(&key) {
            return g.clone();
        }
        let covers = |face: &Face| {
            text.chars()
                .filter(|c| !matches!(c, '\u{fe0e}' | '\u{fe0f}' | '\u{200d}'))
                .all(|c| face.reference().charmap().map(c) != 0)
        };
        let face = if covers(&self.faces[0]) {
            0
        } else {
            self.load_fallbacks();
            self.faces.iter().position(covers).unwrap_or(0)
        };
        let reference = self.faces[face].reference();
        let mut builder = self.shaper.builder(reference).size(px);
        if let Some(c) = text.chars().next() {
            builder = builder.script(c.script());
        }
        let mut shaper = builder.build();
        shaper.add_str(text);
        let mut glyphs = Vec::new();
        let mut pen = 0.;
        shaper.shape_with(|cluster| {
            for g in cluster.glyphs {
                glyphs.push(ShapedGlyph {
                    face: face as u16,
                    id: g.id,
                    x: pen + g.x,
                    y: g.y,
                });
                pen += g.advance;
            }
        });
        if glyphs.iter().any(|g| g.id == 0) && self.warnings.len() < 32 {
            self.warnings
                .push(format!("missing glyph in {text:?}; rendered .notdef"));
        }
        self.shaped.insert(key, glyphs.clone());
        glyphs
    }
    pub fn rasterize(&mut self, face: u16, id: u16, px: f32, bucket: u8) -> Option<Image> {
        let font = self.faces[face as usize].reference();
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fallback_charmap_matches_the_retained_coretext_face() {
        use std::ptr::NonNull;
        let mut fonts = Fonts::load().unwrap();
        for text in ["你", "好"] {
            let glyph = fonts.shape(text, 26.)[0];
            let font = fonts.faces[glyph.face as usize].native.as_ref().unwrap();
            let mut character = text.encode_utf16().next().unwrap();
            let mut native_glyph = 0u16;
            // SAFETY: both pointers address valid single-element buffers, count is exactly one.
            let mapped = unsafe {
                font.glyphs_for_characters(
                    NonNull::from(&mut character),
                    NonNull::from(&mut native_glyph),
                    1,
                )
            };
            assert!(mapped);
            assert_eq!(glyph.id, native_glyph);
        }
    }
    #[test]
    fn alpha_expansion_preserves_coverage() {
        let image = Image {
            content: Content::Mask,
            data: vec![0, 128, 255],
            ..Image::default()
        };
        assert_eq!(
            rgba(&image),
            [255, 255, 255, 0, 255, 255, 255, 128, 255, 255, 255, 255]
        );
    }
    #[test]
    fn menlo_ascii_metrics_and_rasterization_need_no_metal() {
        if !Path::new("/System/Library/Fonts/Menlo.ttc").exists() {
            return;
        }
        let mut fonts = Fonts::load().unwrap();
        assert!(fonts.cell > 6. && fonts.cell < 10.);
        let a = fonts.shape("A", 26.)[0];
        assert_ne!(a.id, 0);
        let image = fonts.rasterize(0, a.id, 26., 0).unwrap();
        assert!(image.placement.width > 0);
        assert_eq!(
            rgba(&image).len(),
            (image.placement.width * image.placement.height * 4) as usize
        );
    }
}
