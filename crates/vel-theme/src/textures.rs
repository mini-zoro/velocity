//! PNG decoding, density selection, nine-slice metadata and file-icon mappings.
use crate::{Error, Insets};
use std::{
    collections::BTreeMap,
    io::Cursor,
    path::{Component, Path},
};

/// Decoded straight-alpha RGBA8 texture ready for a renderer upload.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedTexture {
    /// Chosen package resource, including the density suffix.
    pub resource: String,
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
    /// Density divisor (1, 2 or 3).
    pub scale: u8,
    /// Tightly packed RGBA8, top row first.
    pub rgba: Vec<u8>,
    /// Logical DIP slice insets; the renderer scales these for source-image slicing.
    pub inner_margin: Insets,
    /// Whether the central patch should be drawn.
    pub draw_center: bool,
    /// Whether the patches should tile instead of stretch.
    pub repeat: bool,
}
/// One nine-slice source rectangle, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Patch {
    /// Left, top, width, height in source-image pixels.
    pub source: [f32; 4],
    /// Grid column (0..3).
    pub column: u8,
    /// Grid row (0..3).
    pub row: u8,
}
impl DecodedTexture {
    /// Logical size in DIPs.
    pub fn logical_size(&self) -> [f32; 2] {
        [
            self.width as f32 / f32::from(self.scale),
            self.height as f32 / f32::from(self.scale),
        ]
    }
    /// Iterates the eight or nine source patches without allocation; overlapping insets clamp.
    pub fn patches(&self) -> impl Iterator<Item = Patch> + '_ {
        let s = f32::from(self.scale);
        let w = self.width as f32;
        let h = self.height as f32;
        let l = (self.inner_margin.left * s).clamp(0., w);
        let r = (w - self.inner_margin.right * s).clamp(l, w);
        let t = (self.inner_margin.top * s).clamp(0., h);
        let b = (h - self.inner_margin.bottom * s).clamp(t, h);
        let x = [0., l, r, w];
        let y = [0., t, b, h];
        (0u8..9)
            .filter(move |i| self.draw_center || *i != 4)
            .map(move |i| {
                let col = usize::from(i % 3);
                let row = usize::from(i / 3);
                Patch {
                    source: [x[col], y[row], x[col + 1] - x[col], y[row + 1] - y[row]],
                    column: i % 3,
                    row: i / 3,
                }
            })
    }
}
/// Loads the nearest available PNG density using the caller's package/archive provider.
/// Missing density variants fall back to other densities; malformed selected PNGs are errors.
pub fn load_texture(
    resource: &str,
    density: f32,
    inner_margin: Insets,
    draw_center: bool,
    repeat: bool,
    mut read: impl FnMut(&str) -> Result<Option<Vec<u8>>, Error>,
) -> Result<DecodedTexture, Error> {
    let resource = resource.strip_prefix("Packages/").unwrap_or(resource);
    if resource.contains('\\')
        || Path::new(resource)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || !resource.ends_with(".png")
    {
        return Err(Error::Format(
            "texture must be a package-relative PNG path".into(),
        ));
    }
    if [
        inner_margin.left,
        inner_margin.top,
        inner_margin.right,
        inner_margin.bottom,
    ]
    .iter()
    .any(|n| !n.is_finite() || *n < 0.)
    {
        return Err(Error::Format("invalid slice margin".into()));
    }
    let requested = if density.is_finite() {
        density.ceil().clamp(1., 3.) as u8
    } else {
        1
    };
    let mut scales = [1u8, 2, 3];
    scales.sort_by_key(|s| (s.abs_diff(requested), *s < requested));
    for scale in scales {
        let name = if scale == 1 {
            resource.into()
        } else {
            format!("{}@{scale}x.png", resource.trim_end_matches(".png"))
        };
        let Some(bytes) = read(&name)? else {
            continue;
        };
        let mut decoder = png::Decoder::new(Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        decoder.set_limits(png::Limits {
            bytes: 256 * 1024 * 1024,
        });
        let mut reader = decoder.read_info().map_err(|e| Error::Png(e.to_string()))?;
        let (width, height) = (reader.info().width, reader.info().height);
        if u64::from(width) * u64::from(height) > 64 * 1024 * 1024 {
            return Err(Error::Png("texture exceeds 64 megapixels".into()));
        }
        let size = reader
            .output_buffer_size()
            .ok_or_else(|| Error::Png("PNG buffer overflow".into()))?;
        let mut pixels = vec![0; size];
        let info = reader
            .next_frame(&mut pixels)
            .map_err(|e| Error::Png(e.to_string()))?;
        let pixels = &pixels[..info.buffer_size()];
        let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
        match info.color_type {
            png::ColorType::Rgba => rgba.extend_from_slice(pixels),
            png::ColorType::Rgb => {
                for p in pixels.as_chunks::<3>().0 {
                    rgba.extend_from_slice(&[p[0], p[1], p[2], 255]);
                }
            }
            png::ColorType::Grayscale => {
                for p in pixels {
                    rgba.extend_from_slice(&[*p, *p, *p, 255]);
                }
            }
            png::ColorType::GrayscaleAlpha => {
                for p in pixels.as_chunks::<2>().0 {
                    rgba.extend_from_slice(&[p[0], p[0], p[0], p[1]]);
                }
            }
            png::ColorType::Indexed => return Err(Error::Png("palette expansion failed".into())),
        }
        return Ok(DecodedTexture {
            resource: name,
            width,
            height,
            scale,
            rgba,
            inner_margin,
            draw_center,
            repeat,
        });
    }
    Err(Error::Format(format!("texture not found: {resource}")))
}
/// File-icon identifiers from `.sublime-file-icons`. Overlay themes merge after global mappings.
#[derive(Debug, Clone, Default)]
pub struct FileIcons {
    icons: BTreeMap<String, String>,
    /// Keys that are relative paths (contain '/'); most icon themes have none, and resolving
    /// runs for every visible row each frame.
    path_keys: Vec<String>,
}
impl FileIcons {
    /// Parses lenient JSON mapping extensions, file names and relative paths to icon identifiers.
    pub fn parse(source: &str) -> Result<Self, Error> {
        let source = vel_config::json::Document::parse(source)
            .map_err(Error::Format)?
            .value;
        let values = source
            .get("icons")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| Error::Format("icons must be object".into()))?;
        let mut icons = BTreeMap::new();
        for (key, value) in values {
            let value = value
                .as_str()
                .ok_or_else(|| Error::Format("file icon must be string".into()))?;
            icons.insert(key.clone(), value.into());
        }
        let mut icons = Self {
            icons,
            path_keys: Vec::new(),
        };
        icons.index_paths();
        Ok(icons)
    }
    /// Appends an icon theme, replacing existing identifiers by key.
    pub fn merge(&mut self, overlay: &Self) {
        self.icons.extend(overlay.icons.clone());
        self.index_paths();
    }
    fn index_paths(&mut self) {
        self.path_keys = self
            .icons
            .keys()
            .filter(|key| key.contains('/'))
            .cloned()
            .collect();
    }
    /// Resolves the longest path mapping, exact filename, then longest compound extension.
    /// Returns the icon's identifier without `.png`; package resource lookup belongs to the host.
    pub fn resolve(&self, path: &Path) -> Option<&str> {
        let path = path.to_str()?;
        if path.contains('\\') {
            self.resolve_str(&path.replace('\\', "/"))
        } else {
            self.resolve_str(path)
        }
    }
    fn resolve_str(&self, path: &str) -> Option<&str> {
        if let Some(icon) = self
            .path_keys
            .iter()
            .filter(|key| {
                path.ends_with(key.as_str())
                    && (path.len() == key.len()
                        || path.as_bytes()[path.len() - key.len() - 1] == b'/')
            })
            .max_by_key(|key| key.len())
            .and_then(|key| self.icons.get(key))
        {
            return Some(icon);
        }
        let name = path.rsplit('/').next()?;
        if let Some(icon) = self.icons.get(name) {
            return Some(icon);
        }
        for (at, _) in name.match_indices('.') {
            if let Some(icon) = self.icons.get(&name[at + 1..]) {
                return Some(icon);
            }
        }
        self.icons.get("*").map(String::as_str)
    }
}
