//! Adapted from Velocity’s MIT OR Apache-2.0 P0 prototype.
//! Menlo's ASCII fast path, lazy system fallbacks, swash shaping and rasterization.
use crate::{native, typography::WideAdvances};
use serde_json::{Value, json};
use std::{
    cell::{RefCell, RefMut},
    collections::HashMap,
    path::{Path, PathBuf},
    rc::{Rc, Weak},
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
use vel_platform::fonts::{FontFallback, SystemFontFallback, cluster_characters};

#[path = "emoji_presentation.rs"]
mod emoji;
#[path = "font_snapshot.rs"]
#[cfg(target_os = "macos")]
mod snapshot;

#[derive(Debug)]
struct Face {
    bytes: std::sync::Arc<vel_platform::fonts::FontData>,
    face_snapshot: bool,
    ligatures: bool,
    color_glyphs: bool,
    offset: u32,
    key: CacheKey,
    path: PathBuf,
    name: String,
    native: Option<native::NativeFont>,
    /// Variation instance CoreText resolved for this face (empty for static faces).
    coords: Vec<swash::NormalizedCoord>,
}
impl Face {
    fn load(
        path: &Path,
        name: &str,
        native: Option<native::NativeFont>,
        files: &mut HashMap<PathBuf, std::sync::Arc<vel_platform::fonts::FontData>>,
        outlines: bool,
    ) -> Result<Self, String> {
        // A MobileAsset collection (PingFang is 78 MB) is mapped through a private
        // clone when the volume can make one; only when it cannot is the selected
        // face copied out (13 MB of heap for PingFang SC alone).
        #[cfg(target_os = "macos")]
        let cloned = vel_platform::fonts::is_mobile_asset_path(path)
            .then(|| vel_platform::fonts::map_private_clone(path, files))
            .flatten();
        #[cfg(windows)]
        let cloned = None;
        #[cfg(target_os = "macos")]
        let snapshot = match cloned {
            Some(_) => None,
            None => snapshot::mobile_asset_face(path, name, native.as_deref(), outlines)?,
        };
        #[cfg(windows)]
        let snapshot = {
            let _ = outlines;
            None
        };
        let face_snapshot = snapshot.is_some();
        let bytes = match (cloned, snapshot) {
            (Some(bytes), _) => bytes,
            (None, Some(bytes)) => std::sync::Arc::new(vel_platform::fonts::FontData::Owned(bytes)),
            (None, None) => vel_platform::fonts::map_font_file(path, files)?,
        };
        let collection = FontDataRef::new(&bytes)
            .ok_or_else(|| format!("{} is not a supported font", path.display()))?;
        let named = || {
            collection.fonts().find(|f| {
                f.localized_strings()
                    .any(|s| s.id() == StringId::PostScript && s.to_string() == name)
            })
        };
        #[cfg(windows)]
        let font = named();
        #[cfg(target_os = "macos")]
        let font = if face_snapshot {
            // Already identified the exact face on the opened file descriptor.
            collection.fonts().next()
        } else {
            named().or_else(|| {
                // Private cascade names need not appear in the file's name table.
                // Match immutable SFNT tables to identify the exact collection member;
                // selecting the first member can silently use a different language face.
                let native = native.as_ref()?;
                let mut tables = Vec::new();
                for tag in [
                    swash::tag_from_bytes(b"name"),
                    swash::tag_from_bytes(b"cmap"),
                    swash::tag_from_bytes(b"head"),
                ] {
                    // SAFETY: immutable table copied from a retained CTFont.
                    let table =
                        unsafe { native.table(tag, objc2_core_text::CTFontTableOptions::empty()) }?;
                    tables.push((tag, table.to_vec()));
                }
                let mut matches = collection.fonts().filter(|f| {
                    tables
                        .iter()
                        .all(|(tag, bytes)| f.table(*tag) == Some(bytes.as_slice()))
                });
                let face = matches.next()?;
                if matches.next().is_some() {
                    return None;
                }
                Some(face)
            })
        };
        let font =
            font.ok_or_else(|| format!("{}: exact face {name} unavailable", path.display()))?;
        let actual = font
            .localized_strings()
            .find(|s| s.id() == StringId::PostScript)
            .map(|s| s.to_string())
            .unwrap_or_else(|| name.into());
        let ligatures = font.table(swash::tag_from_bytes(b"GSUB")).is_some() && font.features().any(|f| matches!(f.tag(), t if t == swash::tag_from_bytes(b"liga") || t == swash::tag_from_bytes(b"clig") || t == swash::tag_from_bytes(b"calt")));
        let (offset, key) = (font.offset, font.key);
        let color_glyphs = [b"sbix", b"COLR", b"CBDT"]
            .iter()
            .any(|tag| font.table(swash::tag_from_bytes(tag)).is_some());
        let coords: Vec<swash::NormalizedCoord> = match &native {
            Some(native) if font.variations().len() > 0 => font
                .variations()
                .normalized_coords(native::font_variation(native))
                .collect(),
            _ => Vec::new(),
        };
        let coords = if coords.iter().all(|c| *c == 0) {
            Vec::new()
        } else {
            coords
        };
        Ok(Self {
            bytes,
            face_snapshot,
            ligatures,
            color_glyphs,
            offset,
            key,
            path: path.into(),
            name: actual,
            native,
            coords,
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
    /// UTF-8 byte span of the shaped cluster in the original text.
    pub source: [usize; 2],
}

/// Proportional glyph positions and their measured baseline extents.
#[derive(Debug)]
pub struct UiRun {
    /// Positioned glyphs, with face identities valid for the owning Fonts.
    pub glyphs: Vec<ShapedGlyph>,
    /// Horizontal advance in pixels.
    pub advance: f32,
    /// Maximum ascent of the resolved faces.
    pub ascent: f32,
    /// Maximum descent of the resolved faces.
    pub descent: f32,
}
const SHAPED_BYTES: usize = 4 * 1024 * 1024;
const CHOICE_BYTES: usize = 2 * 1024 * 1024;
const CHOICE_KEYS: usize = 8192;
const DEFAULT_FONT: &str = if cfg!(windows) { "Consolas" } else { "Menlo" };
type ShapeKey = (
    String,
    u32,
    crate::scene::FontStyle,
    crate::typography::FontOptions,
    u64,
    u16,
);
type FaceKey = (PathBuf, String, Vec<(u32, u32)>);
type CascadeKey = u32;

thread_local! {
    static WINDOW_FONTS: RefCell<Weak<RefCell<FontStore>>> = const { RefCell::new(Weak::new()) };
}

#[derive(Debug)]
struct Primary {
    // Preserve the cascade's loaded-face shortcut in this primary's load order;
    // a face opened in an unrelated window must not join that search.
    known: Vec<u16>,
    families: HashMap<String, u16>,
    variants: HashMap<(u16, crate::scene::FontStyle), u16>,
    wide_advances: Option<WideAdvances>,
    metrics: crate::typography::FontMetrics,
    ascii: [u16; 128],
    ascii_complete: bool,
    cell: f32,
    pending: usize,
    blocked: bool,
    fallback_generation: u64,
}
impl Primary {
    fn new(id: u16, face: &Face) -> Self {
        let font = face.reference();
        let metrics = font.metrics(&face.coords).scale(13.);
        let ascii = std::array::from_fn(|i| font.charmap().map(char::from(i as u8)));
        let cell = font
            .glyph_metrics(&face.coords)
            .scale(13.)
            .advance_width(ascii[b'M' as usize]);
        Self {
            known: vec![id],
            families: HashMap::new(),
            variants: HashMap::new(),
            wide_advances: None,
            metrics: crate::typography::FontMetrics {
                ascent: f64::from(metrics.ascent),
                descent: f64::from(metrics.descent.abs()),
                leading: f64::from(metrics.leading),
            },
            ascii_complete: ascii[32..127].iter().all(|g| *g != 0),
            ascii,
            cell,
            pending: 0,
            blocked: false,
            fallback_generation: 0,
        }
    }
}

/// A client's font settings and metrics. Windows use a common font store;
/// explicit fixture loads have their own store and fallback resolver.
pub struct Fonts {
    store: Rc<RefCell<FontStore>>,
    primary: u16,
    options: crate::typography::FontOptions,
    observed_fallback: u64,
    ligatures: bool,
    /// Primary metrics; no fallback face changes the editor grid.
    pub metrics: crate::typography::FontMetrics,
    /// Primary font advance at 13 points.
    pub cell: f32,
    /// Primary ascent at 13 points.
    pub ascent: f32,
}

impl Fonts {
    /// Loads the default primary font in a standalone store.
    pub fn load() -> Result<Self, String> {
        Self::load_named(DEFAULT_FONT)
    }
    /// Loads a requested primary font in a standalone store.
    pub fn load_named(requested: &str) -> Result<Self, String> {
        let mut store = FontStore::default();
        let primary = store.load_primary(requested)?;
        Ok(Self::new(Rc::new(RefCell::new(store)), primary))
    }
    /// Loads the default primary in the window thread's shared font store.
    pub fn load_shared() -> Result<Self, String> {
        Self::load_shared_named(DEFAULT_FONT)
    }
    /// Borrows the window thread's font store, starting it on first use.
    pub fn load_shared_named(requested: &str) -> Result<Self, String> {
        let store = WINDOW_FONTS.with_borrow(Weak::upgrade).unwrap_or_else(|| {
            let store = Rc::new(RefCell::new(FontStore::default()));
            WINDOW_FONTS.with_borrow_mut(|slot| *slot = Rc::downgrade(&store));
            store
        });
        let primary = store.borrow_mut().load_primary(requested)?;
        Ok(Self::new(store, primary))
    }
    /// Loads a fixture or explicitly selected file without consulting the system service.
    pub fn load_file(path: &Path, name: &str) -> Result<Self, String> {
        let mut store = FontStore::default();
        let id = store.intern_face(path, name, None)?;
        store.add_primary(id);
        Ok(Self::new(Rc::new(RefCell::new(store)), id))
    }
    fn new(store: Rc<RefCell<FontStore>>, primary: u16) -> Self {
        let (cell, metrics, observed_fallback, ligatures) = {
            let s = store.borrow();
            let root = &s.roots[&primary];
            (
                root.cell,
                root.metrics,
                root.fallback_generation,
                s.faces[primary as usize].ligatures,
            )
        };
        Self {
            store,
            primary,
            options: Default::default(),
            observed_fallback,
            ligatures,
            metrics,
            cell,
            ascent: metrics.ascent as f32,
        }
    }
    /// Whether the primary face has a supported default substitution feature.
    pub fn has_ligatures(&self) -> bool {
        self.ligatures
    }
    /// Installs completed fallback choices and reports changes to every client once.
    pub fn poll_fallbacks(&mut self) -> bool {
        let generation = {
            let mut store = self.store.borrow_mut();
            store.poll_fallbacks();
            store.roots[&self.primary].fallback_generation
        };
        std::mem::replace(&mut self.observed_fallback, generation) != generation
    }
    /// Whether another display-linked frame should poll this primary's cascade.
    pub fn fallback_pending(&self) -> bool {
        let store = self.store.borrow();
        let root = &store.roots[&self.primary];
        root.pending != 0 || root.blocked
    }
    /// Releases shaping and raster scratch without forgetting resolved fallbacks.
    /// Returns the shaping payload before the trim.
    pub fn trim_shaping(&mut self) -> usize {
        self.store.borrow_mut().trim_shaping()
    }
    /// Drops optional, document-warmed state. Resolved faces are retained
    /// if other primary fonts in this store can reference their indices.
    pub fn clear_document_caches(&mut self) {
        self.store.borrow_mut().clear_document_caches();
    }
    /// Retained payload of the shaping and cascade caches, shared in the window path.
    pub fn document_cache_bytes(&self) -> (usize, usize) {
        let store = self.store.borrow();
        (store.shaped_bytes, store.choice_bytes)
    }
    /// Changes shaping features for this client without changing face identities.
    pub fn set_options(&mut self, options: crate::typography::FontOptions) {
        self.options = options;
    }
    /// Font identities, raster fallback counts and diagnostics.
    pub fn metadata(&self) -> Value {
        self.for_frame().metadata()
    }
    /// Unique owned and mapped backing bytes for this store, not resident memory.
    pub fn file_storage_bytes(&self) -> (usize, usize) {
        self.store.borrow().file_storage_bytes()
    }
    /// Measures representative glyphs with this primary's cascade.
    pub fn wide_advances(&mut self) -> WideAdvances {
        self.for_frame().wide_advances()
    }
    /// Shapes editor text using the regular face.
    pub fn shape(&mut self, text: &str, px: f32) -> Vec<ShapedGlyph> {
        self.shape_styled(text, px, Default::default())
    }
    /// Shapes using actual face variants, including fallback families.
    pub fn shape_styled(
        &mut self,
        text: &str,
        px: f32,
        style: crate::scene::FontStyle,
    ) -> Vec<ShapedGlyph> {
        self.shape_editor(text, px, style, WideAdvances::default())
    }
    /// Shapes an editor run using the same width classes as row geometry.
    pub fn shape_editor(
        &mut self,
        text: &str,
        px: f32,
        style: crate::scene::FontStyle,
        advances: WideAdvances,
    ) -> Vec<ShapedGlyph> {
        self.for_frame().shape_editor(text, px, style, advances)
    }
    /// Shapes a proportional UI run using the same cascade as painting.
    pub fn shape_ui(
        &mut self,
        text: &str,
        families: &str,
        px: f32,
        style: crate::scene::FontStyle,
    ) -> UiRun {
        self.for_frame().shape_ui(text, families, px, style)
    }
    /// Rasterizes one glyph at a quarter-pixel physical phase.
    pub fn rasterize(&mut self, face: u16, id: u16, px: f32, bucket: u8) -> Option<Image> {
        self.for_frame().rasterize(face, id, px, bucket)
    }
    // Borrow once for an entire render, not once per shaped line or bitmap.
    pub(crate) fn for_frame(&self) -> RefMut<'_, FontStore> {
        let mut store = self.store.borrow_mut();
        store.primary = self.primary;
        store.options = self.options;
        store
    }
}

#[derive(Default)]
pub(crate) struct FontStore {
    primary: u16,
    roots: HashMap<u16, Primary>,
    names: HashMap<String, u16>,
    faces: Vec<Face>,
    files: HashMap<PathBuf, std::sync::Arc<vel_platform::fonts::FontData>>,
    face_files: HashMap<FaceKey, u16>,
    fallback: vel_platform::fonts::AsyncFontFallback,
    blocked_primaries: Vec<u16>,
    choices: HashMap<(CascadeKey, char), u16>,
    emoji_choices: HashMap<(CascadeKey, String), u16>,
    text_choices: HashMap<(CascadeKey, String), u16>,
    choice_bytes: usize,
    options: crate::typography::FontOptions,
    warnings: Vec<String>,
    coretext_rasters: usize,
    #[cfg(target_os = "macos")]
    coretext_faces: std::collections::HashSet<u16>,
    scaler: ScaleContext,
    shaper: ShapeContext,
    scratch_used: bool,
    shaped: HashMap<ShapeKey, Vec<ShapedGlyph>>,
    shaped_bytes: usize,
}
impl FontStore {
    fn load_primary(&mut self, requested: &str) -> Result<u16, String> {
        if let Some(&id) = self.names.get(requested) {
            return Ok(id);
        }
        #[cfg(target_os = "macos")]
        let menlo = Path::new("/System/Library/Fonts/Menlo.ttc");
        #[cfg(target_os = "macos")]
        let (path, name, native) = if requested == "Menlo" && menlo.exists() {
            (menlo.into(), "Menlo-Regular".into(), None)
        } else {
            let (path, name, native) = native::font_location(requested)?;
            (path, name, Some(native))
        };
        #[cfg(windows)]
        let (path, name, native) = {
            let (path, name, native) = native::font_location(requested)?;
            (path, name, Some(native))
        };
        let id = self.intern_face(&path, &name, native)?;
        self.add_primary(id);
        self.names.insert(requested.into(), id);
        Ok(id)
    }
    fn add_primary(&mut self, id: u16) {
        self.roots
            .entry(id)
            .or_insert_with(|| Primary::new(id, &self.faces[id as usize]));
    }
    fn intern_face(
        &mut self,
        path: &Path,
        name: &str,
        native: Option<native::NativeFont>,
    ) -> Result<u16, String> {
        let mut variation = native.as_ref().map_or_else(Vec::new, |native| {
            native::font_variation(native)
                .into_iter()
                .map(|(tag, v)| (tag, v.to_bits()))
                .collect()
        });
        variation.sort_unstable();
        let key = (path.into(), name.into(), variation);
        if let Some(&id) = self.face_files.get(&key) {
            return Ok(id);
        }
        let id = u16::try_from(self.faces.len()).map_err(|_| "font face limit")?;
        self.faces
            .push(Face::load(path, name, native, &mut self.files, true)?);
        self.face_files.insert(key, id);
        Ok(id)
    }
    fn load_face(
        &mut self,
        primary: u16,
        path: &Path,
        name: &str,
        native: Option<native::NativeFont>,
    ) -> Result<u16, String> {
        let id = self.intern_face(path, name, native)?;
        let root = self.roots.get_mut(&primary).expect("primary font loaded");
        if !root.known.contains(&id) {
            root.known.push(id);
        }
        Ok(id)
    }
    pub(crate) fn raster_identity(&self, face: u16) -> u64 {
        self.faces[face as usize].key.value()
    }
    pub(crate) fn layout_identity(&self) -> (u64, u64) {
        (
            self.raster_identity(self.primary),
            self.roots[&self.primary].fallback_generation,
        )
    }
    fn cascade_key(primary: u16, base: u16) -> CascadeKey {
        (u32::from(primary) << 16) | u32::from(base)
    }
    pub(crate) fn fallback_pending(&self) -> bool {
        let root = &self.roots[&self.primary];
        root.pending != 0 || root.blocked
    }
    fn poll_fallbacks(&mut self) {
        let results = self.fallback.poll();
        let changed = !results.is_empty();
        for (cascade, key, descriptor) in results {
            let primary = (cascade >> 16) as u16;
            let base = cascade as u16;
            let root = self.roots.get_mut(&primary).expect("fallback primary");
            root.pending -= 1;
            root.fallback_generation += 1;
            use vel_platform::fonts::FallbackKey;
            let face = descriptor
                .and_then(|d| {
                    match self.load_face(
                        primary,
                        &d.path,
                        &d.postscript_name,
                        #[cfg(target_os = "macos")]
                        d.native.map(vel_platform::fonts::CascadeFont::into_native),
                        #[cfg(windows)]
                        Some(d.clone()),
                    ) {
                        Ok(face)
                            if match &key {
                                FallbackKey::Scalar(c) => {
                                    self.faces[face as usize].reference().charmap().map(*c) != 0
                                }
                                FallbackKey::Emoji(cluster) | FallbackKey::Text(cluster) => {
                                    cluster_characters(cluster).all(|c| {
                                        self.faces[face as usize].reference().charmap().map(c) != 0
                                    })
                                }
                            } =>
                        {
                            Some(face)
                        }
                        Ok(_) => None,
                        Err(e) => {
                            if self.warnings.len() < 32 {
                                self.warnings.push(e);
                            }
                            None
                        }
                    }
                })
                .unwrap_or(base);
            self.remember_choice(cascade, key, face);
        }
        if changed {
            // A different primary could have filled the bounded shared queue.
            // Expire .notdef layouts for clients that can now retry their misses.
            for primary in self.blocked_primaries.drain(..) {
                let root = self.roots.get_mut(&primary).expect("fallback primary");
                root.blocked = false;
                root.fallback_generation += 1;
            }
            self.shaped.clear();
            self.shaped_bytes = 0;
        }
    }
    fn remember_choice(
        &mut self,
        cascade: CascadeKey,
        key: vel_platform::fonts::FallbackKey,
        face: u16,
    ) {
        use vel_platform::fonts::FallbackKey;
        // Hash buckets, load-factor/growth slack, and cluster string capacity.
        let charge = 128
            + match &key {
                FallbackKey::Scalar(_) => 0,
                FallbackKey::Emoji(s) | FallbackKey::Text(s) => s.capacity(),
            };
        if charge > CHOICE_BYTES {
            return;
        }
        if self.choices.len() + self.emoji_choices.len() + self.text_choices.len() >= CHOICE_KEYS
            || self.choice_bytes + charge > CHOICE_BYTES
        {
            self.choices = HashMap::new();
            self.emoji_choices = HashMap::new();
            self.text_choices = HashMap::new();
            self.choice_bytes = 0;
        }
        let previous = match key {
            FallbackKey::Scalar(c) => self.choices.insert((cascade, c), face),
            FallbackKey::Emoji(s) => self.emoji_choices.insert((cascade, s), face),
            FallbackKey::Text(s) => self.text_choices.insert((cascade, s), face),
        };
        if previous.is_none() {
            self.choice_bytes += charge;
        }
    }

    fn trim_shaping(&mut self) -> usize {
        let bytes = std::mem::take(&mut self.shaped_bytes);
        if bytes != 0 || self.shaped.capacity() != 0 {
            self.shaped = HashMap::new();
        }
        if std::mem::take(&mut self.scratch_used) {
            self.shaper = ShapeContext::new();
            self.scaler = ScaleContext::new();
        }
        bytes
    }

    fn clear_document_caches(&mut self) {
        self.trim_shaping();
        self.choices = HashMap::new();
        self.emoji_choices = HashMap::new();
        self.text_choices = HashMap::new();
        self.choice_bytes = 0;
        self.fallback = Default::default();
        self.blocked_primaries.clear();
        for root in self.roots.values_mut() {
            root.pending = 0;
            root.blocked = false;
            root.fallback_generation += 1;
        }
        // In a single-primary store, drop document-warmed faces. Never
        // renumber a different client's primary face.
        if self.roots.len() == 1 && (self.faces.len() > 1 || self.files.len() > 1) {
            self.faces.truncate(1);
            self.faces.shrink_to_fit();
            let primary = &self.faces[0];
            self.files = HashMap::from([(primary.path.clone(), primary.bytes.clone())]);
            self.face_files.retain(|_, id| *id == 0);
            self.face_files.shrink_to_fit();
            let root = self.roots.get_mut(&0).expect("sole primary has index zero");
            root.known = vec![0];
            root.families = HashMap::new();
            root.variants = HashMap::new();
            #[cfg(target_os = "macos")]
            self.coretext_faces.clear();
        }
    }

    pub(crate) fn set_options(&mut self, options: crate::typography::FontOptions) {
        self.options = options;
    }
    fn request_fallback(&mut self, cascade: CascadeKey, key: vel_platform::fonts::FallbackKey) {
        let name = &self.faces[cascade as u16 as usize].name;
        let root = self
            .roots
            .get_mut(&self.primary)
            .expect("primary font loaded");
        if self.fallback.request(cascade, name, key) {
            root.pending += 1;
        } else if self.fallback.is_full() && !root.blocked {
            root.blocked = true;
            self.blocked_primaries.push(self.primary);
        }
    }
    fn fallback_face(&mut self, base: u16, cluster: &str) -> u16 {
        use vel_platform::fonts::FallbackKey;
        let cascade = Self::cascade_key(self.primary, base);
        if emoji_presentation(cluster) {
            let cluster = cluster.to_owned();
            let key = (cascade, cluster);
            if let Some(face) = self.emoji_choices.get(&key) {
                return *face;
            }
            self.request_fallback(cascade, FallbackKey::Emoji(key.1));
            return base; // Pending presentation never blocks the first frame.
        }
        if cluster.contains('\u{fe0e}') {
            let key = (cascade, cluster.to_owned());
            if let Some(face) = self.text_choices.get(&key) {
                return *face;
            }
            if let Some(&id) = self.roots[&self.primary].known.iter().find(|&&id| {
                let face = &self.faces[id as usize];
                !face.color_glyphs
                    && cluster_characters(cluster).all(|c| face.reference().charmap().map(c) != 0)
            }) {
                self.remember_choice(cascade, FallbackKey::Text(key.1), id);
                return id;
            }
            self.request_fallback(cascade, FallbackKey::Text(key.1));
            return base;
        }
        let mut chosen = base;
        for scalar in cluster_characters(cluster) {
            if self.faces[chosen as usize]
                .reference()
                .charmap()
                .map(scalar)
                != 0
            {
                continue;
            }
            if let Some(face) = self.choices.get(&(cascade, scalar)) {
                chosen = *face;
                continue;
            }
            if let Some(&id) = self.roots[&self.primary].known.iter().find(|&&id| {
                let face = &self.faces[id as usize];
                cluster_characters(cluster).all(|c| face.reference().charmap().map(c) != 0)
            }) {
                chosen = id;
                self.remember_choice(cascade, FallbackKey::Scalar(scalar), chosen);
                continue;
            }
            self.request_fallback(cascade, FallbackKey::Scalar(scalar));
        }
        chosen
    }
    /// Font identities, raster fallback counts and diagnostics.
    fn metadata(&self) -> Value {
        let root = &self.roots[&self.primary];
        let rasterizer = if cfg!(windows) {
            "swash"
        } else {
            "swash with CoreText fallback when a glyph has no swash image"
        };
        json!({"rasterizer":rasterizer,"coretext_fallback_raster_count":self.coretext_rasters,"point_size":13,"cell_points":root.cell,"ascent_points":root.metrics.ascent,"fallbacks_loaded_lazily":true,"mapped_files":self.files.values().filter(|d|matches!(&***d,vel_platform::fonts::FontData::Mapped(_))).count(),"cloned_files":self.files.values().filter(|d|matches!(&***d,vel_platform::fonts::FontData::Cloned(_))).count(),"font_files":self.files.len(),"face_snapshots":self.faces.iter().filter(|f|f.face_snapshot).count(),"fallback_pending":self.fallback_pending(),"line_height_points":root.metrics.line(13.,1.,0.,0.).0,"faces":self.faces.iter().map(|f|json!({"path":f.path,"postscript_name":f.name,"font_file_bytes":f.bytes.len(),"memory_mapped":matches!(&*f.bytes,vel_platform::fonts::FontData::Mapped(_)|vel_platform::fonts::FontData::Cloned(_)),"private_clone":matches!(&*f.bytes,vel_platform::fonts::FontData::Cloned(_)),"face_snapshot":f.face_snapshot})).collect::<Vec<_>>(),"warnings":self.warnings})
    }
    /// Unique face backing bytes: owned heap capacity and mapped file length.
    /// The latter is not a count of resident pages.
    fn file_storage_bytes(&self) -> (usize, usize) {
        let mut seen = std::collections::HashSet::new();
        let mut owned = 0;
        let mut mapped = 0;
        for bytes in self
            .faces
            .iter()
            .map(|f| &f.bytes)
            .chain(self.files.values())
        {
            if seen.insert(std::sync::Arc::as_ptr(bytes)) {
                match bytes.as_ref() {
                    vel_platform::fonts::FontData::Owned(data) => owned += data.capacity(),
                    vel_platform::fonts::FontData::Mapped(data)
                    | vel_platform::fonts::FontData::Cloned(data) => mapped += data.len(),
                }
            }
        }
        (owned, mapped)
    }
    fn variant(&mut self, base: u16, style: crate::scene::FontStyle) -> u16 {
        if style == crate::scene::FontStyle::default() {
            return base;
        }
        if let Some(face) = self.roots[&self.primary].variants.get(&(base, style)) {
            return *face;
        }
        let name = self.faces[base as usize].name.clone();
        let original = self.faces[base as usize]
            .native
            .clone()
            .map(Ok)
            .unwrap_or_else(|| native::font_location(&name).map(|(_, _, f)| f));
        let face = match original
            .and_then(|font| native::font_variant(&font, style))
            .and_then(|(p, n, f)| self.load_face(self.primary, &p, &n, Some(f)))
        {
            Ok(face) => face,
            Err(e) => {
                self.warnings.push(format!("{name}: {e}"));
                base
            }
        };
        self.roots
            .get_mut(&self.primary)
            .expect("primary font loaded")
            .variants
            .insert((base, style), face);
        face
    }
    /// Measures representative glyphs using the editor's cascade, not a Unicode
    /// column estimate. Keep the measurement faces out of the raster cascade.
    pub fn wide_advances(&mut self) -> WideAdvances {
        let root = &self.roots[&self.primary];
        if let Some(advances) = root.wide_advances {
            return advances;
        }
        let cell = root.cell;
        let primary = &self.faces[self.primary as usize];
        self.scratch_used = true;
        let cells = WideAdvances::SAMPLES.map(|cluster| {
            let first = cluster.chars().next().expect("sample");
            let emoji = emoji_presentation(cluster);
            let resolved = if emoji {
                SystemFontFallback.resolve_cluster(&primary.name, cluster)
            } else if primary.reference().charmap().map(first) == 0 {
                SystemFontFallback.resolve(&primary.name, first)
            } else {
                None
            };
            let face = resolved.and_then(|d| {
                Face::load(
                    &d.path,
                    &d.postscript_name,
                    #[cfg(target_os = "macos")]
                    d.native.map(vel_platform::fonts::CascadeFont::into_native),
                    #[cfg(windows)]
                    Some(d.clone()),
                    &mut self.files,
                    false,
                )
                .ok()
            });
            let face = face.as_ref().unwrap_or(primary);
            let font = face.reference();
            if font.charmap().map(first) == 0 {
                return 0.;
            }
            let mut shaper = self
                .shaper
                .builder(font)
                .size(13.)
                .script(first.script())
                .normalized_coords(&face.coords)
                .build();
            shaper.add_str(cluster);
            let mut advance = 0.;
            shaper.shape_with(|g| advance += g.advance());
            f64::from(advance) / f64::from(cell)
        });
        let advances = WideAdvances::new(cells);
        self.roots
            .get_mut(&self.primary)
            .expect("primary font loaded")
            .wide_advances = Some(advances);
        advances
    }
    /// Shapes an editor run using the same width classes as row geometry.
    pub(crate) fn shape_editor(
        &mut self,
        text: &str,
        px: f32,
        style: crate::scene::FontStyle,
        advances: WideAdvances,
    ) -> Vec<ShapedGlyph> {
        let primary = self.variant(self.primary, style);
        let root = &self.roots[&self.primary];
        let cell = root.cell;
        let shape_ascii = self.options.stylistic_sets != 0
            || ((self.faces[primary as usize].ligatures && self.options.disabled != 7
                || self.options.discretionary)
                && text
                    .as_bytes()
                    .windows(2)
                    .any(|pair| pair.iter().all(u8::is_ascii_punctuation)));
        if text.is_ascii()
            && !shape_ascii
            && (root.ascii_complete || text.bytes().all(|b| root.ascii[b as usize] != 0))
        {
            let face = primary;
            return text
                .bytes()
                .enumerate()
                .map(|(i, b)| ShapedGlyph {
                    face,
                    id: if face == self.primary {
                        root.ascii[b as usize]
                    } else {
                        self.faces[face as usize]
                            .reference()
                            .charmap()
                            .map(char::from(b))
                    },
                    x: i as f32 * cell * px / 13.,
                    y: 0.,
                    source: [i, i + 1],
                })
                .collect();
        }
        let key = (
            text.to_owned(),
            (px * 64.).round() as u32,
            style,
            self.options,
            advances.identity(),
            self.primary,
        );
        if let Some(g) = self.shaped.get(&key) {
            return g.clone();
        }

        // Keep each grapheme intact during fallback, then shape adjacent
        // graphemes of the same face as a run (Kana/Arabic/Indic contextual forms).
        self.scratch_used = true;
        let mut runs: Vec<(usize, usize, u16)> = Vec::new();
        let mut previous_kind = None;
        for (start, cluster) in text.grapheme_indices(true) {
            let kind = if cluster.is_ascii() {
                u8::from(cluster.bytes().all(|b| b.is_ascii_punctuation()))
            } else {
                2
            };
            let face = self.fallback_face(self.primary, cluster);
            let face = self.variant(face, style);
            let end = start + cluster.len();
            if let Some(previous) = runs.last_mut()
                && previous.2 == face
                && previous_kind == Some(kind)
            {
                previous.1 = end;
            } else {
                runs.push((start, end, face));
            }
            previous_kind = Some(kind);
        }
        let cell = cell * px / 13.;
        let mut offsets = vec![0.; text.len() + 1];
        let mut column = 0;
        let mut adjustment = 0.;
        for (at, g) in text.grapheme_indices(true) {
            let x = ((column as f64 + adjustment) * f64::from(cell)) as f32;
            offsets[at..at + g.len()].fill(x);
            let width = g.width().max(1);
            column += width;
            adjustment += advances.adjustment(g, width);
            offsets[at + g.len()] = ((column as f64 + adjustment) * f64::from(cell)) as f32;
        }
        let mut glyphs = Vec::new();
        for (start, end, face) in runs {
            let run = &text[start..end];
            let reference = self.faces[face as usize].reference();
            // ASCII letter runs cannot form programming ligatures. Stylistic sets
            // remain enabled; non-ASCII keeps its script shaping and joining.
            let defaults = if run.is_ascii() && !run.bytes().all(|b| b.is_ascii_punctuation()) {
                7
            } else {
                self.options.disabled
            };
            let mut features = vec![
                (swash::tag_from_bytes(b"liga"), u16::from(defaults & 1 == 0)),
                (swash::tag_from_bytes(b"clig"), u16::from(defaults & 2 == 0)),
                (swash::tag_from_bytes(b"calt"), u16::from(defaults & 4 == 0)),
                (
                    swash::tag_from_bytes(b"dlig"),
                    u16::from(self.options.discretionary),
                ),
            ];
            for set in 1..=20 {
                if self.options.stylistic_sets & (1 << (set - 1)) != 0 {
                    let tag = [b's', b's', b'0' + set / 10, b'0' + set % 10];
                    features.push((swash::tag_from_bytes(&tag), 1));
                }
            }
            let mut builder = self
                .shaper
                .builder(reference)
                .size(px)
                .features(features)
                .normalized_coords(&self.faces[face as usize].coords);
            if let Some(c) = run
                .chars()
                .find(|c| !c.is_ascii_punctuation() && !c.is_whitespace())
            {
                builder = builder.script(c.script());
            }
            let mut shaper = builder.build();
            shaper.add_str(run);
            shaper.shape_with(|cluster| {
                let from = cluster.source.start as usize;
                let to = cluster.source.end as usize;
                let x = offsets[start + from];
                let advance = cluster.advance();
                let target = offsets[start + to] - offsets[start + from];
                let factor = if advance > 0. { target / advance } else { 1. };
                let mut pen = 0.;
                for g in cluster.glyphs {
                    glyphs.push(ShapedGlyph {
                        face,
                        id: g.id,
                        x: x + pen * factor + g.x,
                        y: g.y,
                        source: [start + from, start + to],
                    });
                    pen += g.advance;
                }
            });
        }
        if self.warnings.len() < 32 && glyphs.iter().any(|g| g.id == 0) {
            self.warnings.push(format!(
                "missing glyph in {:?}; rendered .notdef",
                text.get(..text.floor_char_boundary(text.len().min(256)))
                    .unwrap_or(text)
            ));
        }
        self.remember_shaped(key, &glyphs);
        glyphs
    }

    fn remember_shaped(&mut self, key: ShapeKey, glyphs: &[ShapedGlyph]) {
        let charge = 256 + key.0.capacity() + size_of_val(glyphs);
        if charge <= SHAPED_BYTES {
            if self.shaped.len() >= 1024 || self.shaped_bytes + charge > SHAPED_BYTES {
                self.shaped = HashMap::new();
                self.shaped_bytes = 0;
            }
            self.shaped.insert(key, glyphs.to_vec());
            self.shaped_bytes += charge;
        }
    }
    /// Shapes a proportional UI run using the same face/fallback/raster stack as painting.
    /// Family lists are tried in order; generic CSS families map to native system faces.
    pub(crate) fn shape_ui(
        &mut self,
        text: &str,
        families: &str,
        px: f32,
        style: crate::scene::FontStyle,
    ) -> UiRun {
        let root = &self.roots[&self.primary];
        let base = if let Some(index) = root.families.get(families) {
            *index
        } else if root.families.len() >= 256 || root.known.len() >= 512 {
            self.primary
        } else {
            let mut index = self.primary;
            for requested in families
                .split(',')
                .map(|s| s.trim().trim_matches(['\"', '\'']))
            {
                let name = match requested {
                    "system" | "system-ui" | "sans-serif" => ".AppleSystemUIFont",
                    "monospace" => "Menlo",
                    "serif" => "Times",
                    s => s,
                };
                if let Ok((path, name, native)) = native::font_location(name)
                    && let Ok(face) = self.load_face(self.primary, &path, &name, Some(native))
                {
                    index = face;
                    break;
                }
            }
            let root = self
                .roots
                .get_mut(&self.primary)
                .expect("primary font loaded");
            root.families.insert(families.into(), index);
            index
        };

        let mut runs: Vec<(usize, usize, u16)> = Vec::new();
        for (start, cluster) in text.grapheme_indices(true) {
            let base = self.fallback_face(base, cluster);
            let face = self.variant(base, style);
            if let Some(last) = runs.last_mut()
                && last.2 == face
            {
                last.1 = start + cluster.len();
            } else {
                runs.push((start, start + cluster.len(), face));
            }
        }
        let mut out = UiRun {
            glyphs: Vec::new(),
            advance: 0.,
            ascent: 0.,
            descent: 0.,
        };
        for (start, end, face) in runs {
            let reference = self.faces[face as usize].reference();
            let coords = &self.faces[face as usize].coords;
            let metrics = reference.metrics(coords).scale(px);
            out.ascent = out.ascent.max(metrics.ascent);
            out.descent = out.descent.max(metrics.descent.abs());
            let run = &text[start..end];
            let mut builder = self
                .shaper
                .builder(reference)
                .size(px)
                .normalized_coords(coords);
            if let Some(c) = run
                .chars()
                .find(|c| !c.is_ascii_punctuation() && !c.is_whitespace())
            {
                builder = builder.script(c.script());
            }
            let mut shaper = builder.build();
            shaper.add_str(run);
            shaper.shape_with(|cluster| {
                let source = [
                    start + cluster.source.start as usize,
                    start + cluster.source.end as usize,
                ];
                for g in cluster.glyphs {
                    out.glyphs.push(ShapedGlyph {
                        face,
                        id: g.id,
                        x: out.advance + g.x,
                        y: g.y,
                        source,
                    });
                    out.advance += g.advance;
                }
            });
        }
        out
    }
    /// Rasterizes one glyph at a quarter-pixel physical phase.
    pub(crate) fn rasterize(&mut self, face: u16, id: u16, px: f32, bucket: u8) -> Option<Image> {
        self.scratch_used = true;
        let no_antialias = bucket & 4 != 0;
        let bucket = bucket & 3;
        let font = self.faces[face as usize].reference();
        if id != 0 && id == font.charmap().map(' ') {
            return None;
        }
        let mut scaler = self
            .scaler
            .builder(font)
            .size(px)
            .hint(true)
            .normalized_coords(&self.faces[face as usize].coords)
            .build();
        let image = Render::new(&[
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
        ])
        .format(Format::Alpha)
        .offset(Vector::new(bucket as f32 * 0.25, 0.))
        .render(&mut scaler, id);
        if let Some(image) = image.filter(|i| i.placement.width > 0 && i.placement.height > 0) {
            let mut image = image;
            if no_antialias && image.content == Content::Mask {
                image
                    .data
                    .iter_mut()
                    .for_each(|a| *a = if *a >= 128 { 255 } else { 0 });
            }
            return Some(image);
        }
        #[cfg(windows)]
        return None;
        #[cfg(target_os = "macos")]
        {
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
                Ok(mut image) => {
                    if no_antialias && image.content == Content::Mask {
                        image
                            .data
                            .iter_mut()
                            .for_each(|a| *a = if *a >= 128 { 255 } else { 0 });
                    }
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
}

#[cfg(test)]
#[test]
fn distinct_cjk_and_large_shaped_runs_fit_cpu_budgets_and_clear_on_close() {
    use vel_platform::fonts::FallbackKey;
    let mut fonts = Fonts::load().unwrap();
    let baseline = fonts.file_storage_bytes();
    let mut scalars = (0x3400..=0x9fff)
        .chain(0x20000..)
        .filter_map(char::from_u32)
        .take(30_000);
    for _ in 0..300 {
        let run: String = scalars.by_ref().take(100).collect();
        for c in run.chars() {
            fonts
                .store
                .borrow_mut()
                .remember_choice(0, FallbackKey::Scalar(c), 0);
        }
        fonts.shape(&run, 13.);
        assert!(fonts.store.borrow().choices.len() <= CHOICE_KEYS);
        assert!(fonts.store.borrow().choice_bytes <= CHOICE_BYTES);
        assert!(fonts.store.borrow().shaped_bytes <= SHAPED_BYTES);
    }
    let glyph = ShapedGlyph {
        face: 0,
        id: 1,
        x: 0.,
        y: 0.,
        source: [0, 1],
    };
    let oversized = vec![glyph; SHAPED_BYTES / size_of::<ShapedGlyph>() + 1];
    let before = fonts.store.borrow().shaped_bytes;
    fonts.store.borrow_mut().remember_shaped(
        (
            "oversized".into(),
            832,
            Default::default(),
            Default::default(),
            0,
            0,
        ),
        &oversized,
    );
    assert_eq!(fonts.store.borrow().shaped_bytes, before);
    assert!(before > 0);
    fonts.clear_document_caches();
    assert_eq!(fonts.document_cache_bytes(), (0, 0));
    assert_eq!(
        fonts.store.borrow().shaped.capacity() + fonts.store.borrow().choices.capacity(),
        0
    );
    assert_eq!(fonts.file_storage_bytes(), baseline);
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
        let store = self.store.borrow();
        f.debug_struct("Fonts")
            .field("faces", &store.faces)
            .field("cell", &self.cell)
            .field("warnings", &store.warnings)
            .finish_non_exhaustive()
    }
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn window_fonts_share_storage_but_keep_primary_and_size_distinct() {
    let mut first = Fonts::load_shared().unwrap();
    let mut second = Fonts::load_shared().unwrap();
    let mut other = Fonts::load_shared_named("Monaco").unwrap();
    assert!(Rc::ptr_eq(&first.store, &second.store));
    assert!(Rc::ptr_eq(&first.store, &other.store));
    assert_eq!(first.primary, second.primary);
    assert_ne!(first.primary, other.primary);

    let text = "Ma\u{00e4}";
    let before = first.shape(text, 13.);
    let warm = first.document_cache_bytes();
    assert!(warm.0 > 0);
    let again = second.shape(text, 13.);
    assert_eq!(first.document_cache_bytes(), warm);
    assert_eq!(again[1].id, before[1].id);
    assert_eq!(again[1].x, before[1].x);
    let bigger = second.shape(text, 26.);
    assert_eq!(bigger[1].x, 2. * before[1].x);
    assert_ne!(other.shape(text, 13.)[0].face, before[0].face);

    let store = Rc::downgrade(&first.store);
    drop(first);
    drop(second);
    assert!(store.upgrade().is_some());
    drop(other);
    assert!(store.upgrade().is_none());
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    /// Hang guard: stops a wait that would otherwise never end. It is not a latency
    /// bound; the test checks the condition it waits for.
    const HANG_GUARD: std::time::Duration = std::time::Duration::from_secs(120);

    #[test]
    fn bold_italic_use_distinct_real_faces_without_changing_grid_advance() {
        let mut fonts = super::Fonts::load().unwrap();
        assert!(
            !fonts.has_ligatures(),
            "Menlo's AAT features must not route ASCII through the OpenType shaper"
        );
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
    /// The grid is measured at the instance glyphs are shaped with: static Menlo exactly as
    /// its default instance, a variable primary face at CoreText's coordinates.
    #[test]
    fn primary_grid_matches_the_shaped_instance() {
        let menlo = super::Fonts::load().unwrap();
        let store = menlo.store.borrow();
        assert!(store.faces[0].coords.is_empty());
        let face = store.faces[0].reference();
        let m = store.roots[&menlo.primary].ascii[b'M' as usize];
        let cell = face.glyph_metrics(&[]).scale(13.).advance_width(m);
        assert_eq!(menlo.cell.to_bits(), cell.to_bits());
        let metrics = face.metrics(&[]).scale(13.);
        assert_eq!(menlo.ascent.to_bits(), metrics.ascent.to_bits());
        assert_eq!(menlo.metrics.descent, f64::from(metrics.descent.abs()));
        #[cfg(target_os = "macos")]
        {
            let mut system = super::Fonts::load_named(".AppleSystemUIFont").unwrap();
            if !system.store.borrow().faces[0].coords.is_empty() {
                let run = system.shape_ui(
                    "M",
                    ".AppleSystemUIFont",
                    13.,
                    crate::scene::FontStyle::default(),
                );
                assert_eq!(run.glyphs[0].face, 0);
                assert!(
                    (run.advance - system.cell).abs() < 0.01,
                    "cell {} vs shaped M {}",
                    system.cell,
                    run.advance
                );
            }
        }
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn bold_ui_runs_keep_their_weight_in_the_cjk_fallback() {
        // The system UI font's Chinese fallback answers a bold request with a virtual font
        // that has no file of its own (only `wght` 700). Side bar headings are bold 11 pt:
        // "打开的文件" must be as bold as "OPEN FILES", not the regular face.
        let mut fonts = super::Fonts::load().unwrap();
        let bold = crate::scene::FontStyle {
            bold: true,
            italic: false,
        };
        let deadline = std::time::Instant::now() + HANG_GUARD;
        loop {
            let _ = fonts.shape_ui("打开", "system", 22., Default::default());
            let _ = fonts.shape_ui("打开", "system", 22., bold);
            if !fonts.fallback_pending() {
                break;
            }
            while fonts.fallback_pending() {
                assert!(std::time::Instant::now() < deadline);
                fonts.poll_fallbacks();
                std::thread::park_timeout(std::time::Duration::from_millis(1));
            }
        }
        let regular = fonts.shape_ui("打开", "system", 22., Default::default());
        let heavy = fonts.shape_ui("打开", "system", 22., bold);
        let (r, b) = (regular.glyphs[0], heavy.glyphs[0]);
        assert!(r.id != 0 && b.id != 0, "{}", fonts.metadata());
        assert!(
            fonts
                .store
                .borrow()
                .warnings
                .iter()
                .all(|w| !w.contains("no file URL")),
            "{:?}",
            fonts.store.borrow().warnings
        );
        let ink = |face: u16, id: u16, fonts: &mut super::Fonts| {
            let image = fonts.rasterize(face, id, 22., 0).unwrap();
            image.data.iter().map(|a| u32::from(*a)).sum::<u32>()
        };
        let regular_ink = ink(r.face, r.id, &mut fonts);
        let bold_ink = ink(b.face, b.id, &mut fonts);
        assert!(
            bold_ink > regular_ink * 6 / 5,
            "bold {bold_ink} vs regular {regular_ink} ({} / {})",
            fonts.store.borrow().faces[r.face as usize].name,
            fonts.store.borrow().faces[b.face as usize].name
        );
        // Full-width characters keep their one-em advance in either weight.
        assert!((heavy.advance - regular.advance).abs() < 0.5);
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn variable_system_font_bold_and_text_cut_follow_coretext() {
        // The system UI font is one variable file: CoreText's bold face is that file at
        // wght 700 and small text uses the opsz 17 cut. Both must reach the rasterizer.
        let mut fonts = super::Fonts::load().unwrap();
        let regular = fonts.shape_ui("Folders", "system", 13., crate::scene::FontStyle::default());
        let bold = fonts.shape_ui(
            "Folders",
            "system",
            13.,
            crate::scene::FontStyle {
                bold: true,
                italic: false,
            },
        );
        let (r, b) = (regular.glyphs[0].face, bold.glyphs[0].face);
        if fonts.store.borrow().faces[r as usize].path
            != fonts.store.borrow().faces[b as usize].path
        {
            return; // A static family: real faces, nothing to apply.
        }
        assert!(
            !fonts.store.borrow().faces[b as usize].coords.is_empty(),
            "bold instance"
        );
        assert!(
            !fonts.store.borrow().faces[r as usize].coords.is_empty(),
            "text optical size"
        );
        assert!(bold.advance > regular.advance + 0.5);
        let ink = |face: u16, id: u16, fonts: &mut super::Fonts| {
            let image = fonts.rasterize(face, id, 13., 0).unwrap();
            image.data.iter().map(|a| u32::from(*a)).sum::<u32>()
        };
        let regular_ink = ink(r, regular.glyphs[0].id, &mut fonts);
        let bold_ink = ink(b, bold.glyphs[0].id, &mut fonts);
        assert!(
            bold_ink > regular_ink * 11 / 10,
            "{bold_ink} vs {regular_ink}"
        );
    }
    use super::*;
    #[test]
    fn ascii_fast_path_mixed_fallback_and_coretext_bitmap_are_headless() {
        let mut fonts = Fonts::load().unwrap();
        let ascii = fonts.shape("ABC", 26.);
        assert_eq!(ascii.len(), 3);
        assert!((ascii[1].x - fonts.cell * 2.).abs() < 0.001);
        let _ = fonts.shape("a你好🦀é", 26.);
        let deadline = std::time::Instant::now() + HANG_GUARD;
        while fonts.fallback_pending() {
            assert!(std::time::Instant::now() < deadline);
            fonts.poll_fallbacks();
            std::thread::park_timeout(std::time::Duration::from_millis(1));
        }
        let mixed = fonts.shape("a你好🦀é", 26.);
        assert!(
            mixed.iter().all(|g| g.id != 0),
            "mixed={mixed:?} metadata={}",
            fonts.metadata()
        );
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

#[cfg(all(test, target_os = "macos"))]
mod round17_tests {
    use super::*;
    fn fixture() -> Fonts {
        Fonts::load_file(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/ligature-fixture.ttf"),
            "VelocityFixture-Regular",
        )
        .unwrap()
    }
    #[test]
    fn features_shape_ascii_cache_runs_and_split_ligature_width_on_the_caret_grid() {
        let mut fonts = fixture();
        assert!(fonts.has_ligatures());
        let glyphs = fonts.shape("->x", 13.);
        assert_eq!(glyphs.len(), 2);
        assert!(
            (glyphs[1].x as f64 - crate::typography::grid_x(2, fonts.cell as f64)).abs() < 0.001
        );
        // The caret between '-' and '>' splits the two-cell cluster in half.
        assert!(
            (crate::typography::grid_x(1, fonts.cell as f64) - glyphs[1].x as f64 / 2.).abs()
                < 0.001
        );
        let count = fonts.store.borrow().shaped.len();
        fonts.shape("->x", 13.);
        assert_eq!(fonts.store.borrow().shaped.len(), count);
        fonts.set_options(crate::typography::FontOptions::parse(["no_liga"]));
        assert_eq!(fonts.shape("->x", 13.).len(), 3);
        fonts.set_options(crate::typography::FontOptions::default());
        assert_eq!(fonts.shape("!=", 13.).len(), 1);
        fonts.set_options(crate::typography::FontOptions::parse(["no_calt"]));
        assert_eq!(fonts.shape("!=", 13.).len(), 2);
        let regular = fonts.shape("x", 13.)[0].id;
        fonts.set_options(crate::typography::FontOptions::parse(["ss01"]));
        assert_ne!(fonts.shape("x", 13.)[0].id, regular);
        assert_eq!(fonts.store.borrow().files.len(), 1);
        assert_eq!(fonts.store.borrow().faces.len(), 1);
        assert!(!fonts.fallback_pending());
    }
    #[test]
    fn font_file_snapshots_are_deduplicated_without_loading_fallbacks() {
        let fonts = fixture();
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/ligature-fixture.ttf");
        assert_eq!(
            fonts
                .store
                .borrow_mut()
                .load_face(0, &path, "VelocityFixture-Regular", None)
                .unwrap(),
            0
        );
        assert_eq!(fonts.store.borrow().files.len(), 1);
        assert!(std::sync::Arc::strong_count(&fonts.store.borrow().faces[0].bytes) >= 2);
        assert!(fonts.store.borrow().choices.is_empty());
        assert!(!fonts.fallback_pending());
    }
    #[test]
    #[ignore = "explicit font timing, RSS and native cascade measurement"]
    fn round17_font_timing_rss_and_ligature_cost() {
        use std::{
            hint::black_box,
            time::{Duration, Instant},
        };
        let rss_before = vel_platform::metrics::rss_bytes().unwrap();
        let mut fonts = Fonts::load().unwrap();
        let rss_primary = vel_platform::metrics::rss_bytes().unwrap();
        let start = Instant::now();
        black_box(fonts.shape("你", 26.));
        let first = start.elapsed();
        let deadline = Instant::now() + Duration::from_secs(15);
        while fonts.fallback_pending() {
            assert!(Instant::now() < deadline);
            fonts.poll_fallbacks();
            std::thread::park_timeout(Duration::from_micros(100));
        }
        let ready = start.elapsed();
        black_box(fonts.shape("你", 26.));
        let rss_after = vel_platform::metrics::rss_bytes().unwrap();
        println!(
            "FONT_TIMING {}",
            json!({"first_non_ascii_ms":first.as_secs_f64()*1000.,"fallback_ready_ms":ready.as_secs_f64()*1000.,"rss_before":rss_before,"rss_primary":rss_primary,"rss_after":rss_after,"default_metrics":format!("{:?}",fonts.metrics),"line_height_1x":fonts.metrics.line(13.,1.,0.,0.).0,"line_height_2x":fonts.metrics.line(13.,2.,0.,0.).0,"metadata":fonts.metadata()})
        );
        let path = std::env::var_os("VELOCITY_LIGATURE_FONT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/ligature-fixture.ttf")
            });
        let name = if path
            .file_name()
            .is_some_and(|n| n == "FiraCode-Regular.ttf")
        {
            "FiraCode-Regular"
        } else {
            "VelocityFixture-Regular"
        };
        let mut ligatures = Fonts::load_file(&path, name).unwrap();
        assert!(ligatures.has_ligatures());
        for (label, font) in [("Menlo", &mut fonts), (name, &mut ligatures)] {
            let cold = Instant::now();
            for i in 0..4000 {
                black_box(font.shape(&format!("-> != === => ++ -- /* */ {i}"), 26.));
            }
            let cold = cold.elapsed();
            black_box(font.shape("-> != === => ++ -- /* */", 26.));
            let warm = Instant::now();
            for _ in 0..20_000 {
                black_box(font.shape("-> != === => ++ -- /* */", 26.));
            }
            println!(
                "SHAPING_TIMING {}",
                json!({"font":label,"cold_run_us":cold.as_secs_f64()*1e6/4000.,"warm_run_us":warm.elapsed().as_secs_f64()*1e6/20_000.,"file":path,"mapped_files":font.store.borrow().files.len()})
            );
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod fallback_mock_tests {
    use super::*;
    /// Hang guard: stops a wait that would otherwise never end. It is not a latency
    /// bound; the test checks the condition it waits for.
    const HANG_GUARD: std::time::Duration = std::time::Duration::from_secs(120);

    #[test]
    fn missing_scripts_resolve_lazily_once_per_scalar_and_load_one_mapped_file() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        struct Resolver {
            calls: Arc<AtomicUsize>,
            path: PathBuf,
        }
        impl vel_platform::fonts::FontFallback for Resolver {
            fn resolve(&mut self, _: &str, _: char) -> Option<vel_platform::fonts::FontDescriptor> {
                self.calls.fetch_add(1, Ordering::Relaxed);
                Some(vel_platform::fonts::FontDescriptor::file(
                    self.path.clone(),
                    "VelocityFallback-Regular".into(),
                ))
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let calls = Arc::new(AtomicUsize::new(0));
        let mut fonts = Fonts::load_file(
            &root.join("ligature-fixture.ttf"),
            "VelocityFixture-Regular",
        )
        .unwrap();
        fonts.store.borrow_mut().fallback =
            vel_platform::fonts::AsyncFontFallback::with_resolver(Resolver {
                calls: calls.clone(),
                path: root.join("fallback-fixture.ttf"),
            });
        fonts.shape("->x", 13.);
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert_eq!(fonts.store.borrow().faces.len(), 1);
        let scripts = "한ไअאع𝒜⠿\u{e000}";
        fonts.shape(scripts, 13.);
        let deadline = std::time::Instant::now() + HANG_GUARD;
        while fonts.fallback_pending() {
            assert!(std::time::Instant::now() < deadline);
            fonts.poll_fallbacks();
            std::thread::yield_now();
        }
        for c in scripts.chars() {
            let glyphs = fonts.shape(&c.to_string(), 13.);
            assert!(
                glyphs.iter().all(|g| g.id != 0 && g.face == 1),
                "{c}: {glyphs:?}"
            );
        }
        fonts.shape(scripts, 13.);
        assert_eq!(calls.load(Ordering::Relaxed), 8);
        assert_eq!(fonts.store.borrow().faces.len(), 2);
        assert_eq!(fonts.store.borrow().files.len(), 2);
    }
}

// Unicode presentation sequences are resolved as a unit, before primary coverage.
// https://www.unicode.org/reports/tr51/#Emoji_Sequences
pub(crate) fn emoji_presentation(cluster: &str) -> bool {
    if cluster.contains('\u{fe0e}') {
        return false;
    }
    // A joiner alone is also used by Indic/Arabic shaping. Only pictographic
    // ZWJ sequences request emoji presentation; text-script clusters keep their face.
    (cluster.contains('\u{fe0f}')
        && cluster
            .chars()
            .any(|c| c.is_emoji() || c.is_extended_pictographic()))
        || (cluster.contains('\u{200d}')
            && cluster.chars().any(Codepoint::is_extended_pictographic))
        || cluster.chars().any(emoji::default_presentation)
        || (cluster.contains('\u{20e3}')
            && cluster
                .chars()
                .next()
                .is_some_and(|c| matches!(c, '#' | '*' | '0'..='9')))
}

#[cfg(all(test, target_os = "macos"))]
mod review_regressions {
    use super::*;
    /// Hang guard: stops a wait that would otherwise never end. It is not a latency
    /// bound; the test checks the condition it waits for.
    const HANG_GUARD: std::time::Duration = std::time::Duration::from_secs(120);

    #[test]
    fn replacing_user_font_in_place_keeps_shaping_the_loaded_snapshot() {
        let path = std::env::temp_dir().join(format!(
            "velocity-font-replace-{}-{}.ttf",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/ligature-fixture.ttf");
        std::fs::copy(source, &path).unwrap();
        let mut fonts = Fonts::load_file(&path, "VelocityFixture-Regular").unwrap();
        let before = fonts.shape("->x", 13.);
        std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        fonts.store.borrow_mut().shaped.clear();
        let after = fonts.shape("->x", 13.);
        assert_eq!(
            before.iter().map(|g| g.id).collect::<Vec<_>>(),
            after.iter().map(|g| g.id).collect::<Vec<_>>()
        );
        assert!(after.iter().all(|g| g.id != 0));
        // A private APFS clone (or, off APFS, a heap snapshot); never a mapping of
        // the replaceable file itself.
        assert!(matches!(
            &*fonts.store.borrow().faces[0].bytes,
            vel_platform::fonts::FontData::Owned(_) | vel_platform::fonts::FontData::Cloned(_)
        ));
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn emoji_presentation_cluster_table_precedes_primary_text_coverage() {
        use vel_platform::fonts::{AsyncFontFallback, FontDescriptor, FontFallback};
        struct Resolver(PathBuf);
        impl FontFallback for Resolver {
            fn resolve(&mut self, _: &str, _: char) -> Option<FontDescriptor> {
                panic!("text covered by primary must not cascade")
            }
            fn resolve_cluster(&mut self, _: &str, _: &str) -> Option<FontDescriptor> {
                Some(FontDescriptor::file(
                    self.0.clone(),
                    "VelocityEmoji-Regular".into(),
                ))
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let mut fonts = Fonts::load_file(
            &root.join("ligature-fixture.ttf"),
            "VelocityFixture-Regular",
        )
        .unwrap();
        fonts.store.borrow_mut().fallback =
            AsyncFontFallback::with_resolver(Resolver(root.join("emoji-fixture.ttf")));
        let cases = [
            ("❤️", true),
            ("☺️", true),
            ("✔️", true),
            ("⚠️", true),
            ("©️", true),
            ("↔️", true),
            ("☀️", true),
            ("♥️", true),
            ("✌️", true),
            ("1️⃣", true),
            ("#️⃣", true),
            ("*️⃣", true),
            ("👩‍💻", true),
            ("🇨🇳", true),
            ("👍🏽", true),
            ("❤", false),
            ("❤︎", false),
            ("☺︎", false),
            ("x\u{fe0f}", false),
            ("क्\u{200d}ष", false),
        ];
        for (cluster, emoji) in cases {
            assert_eq!(emoji_presentation(cluster), emoji, "{cluster}");
            fonts.shape(cluster, 13.);
        }
        let deadline = std::time::Instant::now() + HANG_GUARD;
        while fonts.fallback_pending() {
            assert!(std::time::Instant::now() < deadline);
            fonts.poll_fallbacks();
            std::thread::yield_now();
        }
        for (cluster, emoji) in cases {
            let glyphs = fonts.shape(cluster, 13.);
            assert!(!glyphs.is_empty(), "{cluster}");
            assert!(
                glyphs.iter().all(|g| g.face == u16::from(emoji)),
                "{cluster}: {glyphs:?}"
            );
        }
        assert_eq!(fonts.store.borrow().faces.len(), 2);
        assert_eq!(fonts.store.borrow().files.len(), 2);
        assert!(!fonts.fallback_pending());
    }
    #[test]
    fn text_presentation_ignores_loaded_and_cached_emoji_faces_in_either_order() {
        use vel_platform::fonts::{AsyncFontFallback, FontDescriptor, FontFallback};
        struct Resolver(PathBuf);
        impl FontFallback for Resolver {
            fn resolve(&mut self, _: &str, scalar: char) -> Option<FontDescriptor> {
                assert_eq!(scalar, '⛑');
                Some(FontDescriptor::file(
                    self.0.join("fallback-fixture.ttf"),
                    "VelocityFallback-Regular".into(),
                ))
            }
            fn resolve_cluster(&mut self, _: &str, _: &str) -> Option<FontDescriptor> {
                Some(FontDescriptor::file(
                    self.0.join("emoji-fixture.ttf"),
                    "VelocityEmoji-Regular".into(),
                ))
            }
        }
        fn drain(fonts: &mut Fonts) {
            let deadline = std::time::Instant::now() + HANG_GUARD;
            while fonts.fallback_pending() {
                assert!(std::time::Instant::now() < deadline);
                fonts.poll_fallbacks();
                std::thread::yield_now();
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        for emoji_first in [false, true] {
            let mut fonts = Fonts::load_file(
                &root.join("ligature-fixture.ttf"),
                "VelocityFixture-Regular",
            )
            .unwrap();
            fonts.store.borrow_mut().fallback =
                AsyncFontFallback::with_resolver(Resolver(root.clone()));
            if emoji_first {
                fonts.shape("😀", 13.);
                drain(&mut fonts);
                assert!(fonts.store.borrow().faces[1].color_glyphs);
                // Exercise both the already-loaded-face shortcut and a cached scalar.
                fonts.shape("⛑", 13.);
            }
            fonts.shape("⛑\u{fe0e}", 13.);
            drain(&mut fonts);
            if !emoji_first {
                fonts.shape("😀", 13.);
                drain(&mut fonts);
            }
            fonts.store.borrow_mut().shaped.clear();
            let glyphs = fonts.shape("⛑\u{fe0e}", 13.);
            assert!(!glyphs.is_empty());
            assert!(
                glyphs.iter().all(|glyph| {
                    let face = &fonts.store.borrow().faces[glyph.face as usize];
                    glyph.id != 0 && !face.color_glyphs && face.name == "VelocityFallback-Regular"
                }),
                "emoji_first={emoji_first}: {glyphs:?}"
            );
            assert!(!fonts.fallback_pending());
        }
    }
    #[test]
    fn presentation_fallback_converges_for_color_only_text_missing_and_combining_clusters() {
        use std::sync::{Arc, Mutex};
        use vel_platform::fonts::{AsyncFontFallback, FontDescriptor, FontFallback};
        struct Resolver {
            root: PathBuf,
            calls: Arc<Mutex<HashMap<String, usize>>>,
        }
        impl Resolver {
            fn result(&self, key: String, emoji: bool, missing: bool) -> Option<FontDescriptor> {
                *self.calls.lock().unwrap().entry(key).or_default() += 1;
                if missing {
                    return None;
                }
                Some(FontDescriptor::file(
                    self.root.join(if emoji {
                        "emoji-fixture.ttf"
                    } else {
                        "fallback-fixture.ttf"
                    }),
                    if emoji {
                        "VelocityEmoji-Regular"
                    } else {
                        "VelocityFallback-Regular"
                    }
                    .into(),
                ))
            }
        }
        impl FontFallback for Resolver {
            fn resolve(&mut self, _: &str, scalar: char) -> Option<FontDescriptor> {
                self.result(
                    format!("scalar:{scalar}"),
                    matches!(scalar, '⌚' | '🌡' | '🖥'),
                    matches!(scalar, '\u{0378}' | '\u{10ffff}'),
                )
            }
            fn resolve_cluster(&mut self, _: &str, cluster: &str) -> Option<FontDescriptor> {
                self.result(format!("emoji:{cluster}"), true, false)
            }
            fn resolve_text_cluster(&mut self, _: &str, cluster: &str) -> Option<FontDescriptor> {
                self.result(
                    format!("text:{cluster}"),
                    cluster.chars().any(|c| matches!(c, '⌚' | '🌡' | '🖥')),
                    cluster.contains('\u{0378}'),
                )
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let calls = Arc::new(Mutex::new(HashMap::new()));
        let mut fonts = Fonts::load_file(
            &root.join("ligature-fixture.ttf"),
            "VelocityFixture-Regular",
        )
        .unwrap();
        fonts.store.borrow_mut().fallback = AsyncFontFallback::with_resolver(Resolver {
            root,
            calls: calls.clone(),
        });
        let cases = [
            ("⌚\u{fe0e}", true),
            ("🌡\u{fe0e}", true),
            ("🖥\u{fe0e}", true),
            ("a⌚\u{fe0e}b", true),
            ("⛑\u{fe0e}", true),
            ("❤\u{fe0e}", true),
            ("☺\u{fe0e}", true),
            ("❤️", true),
            ("⌚️", true),
            ("🖥️", true),
            ("⛑️", true),
            ("😀", true),
            ("1️⃣", true),
            ("1\u{fe0e}\u{20e3}", true),
            ("#\u{fe0e}\u{20e3}", true),
            ("*\u{fe0e}\u{20e3}", true),
            ("#\u{fe0f}\u{20e3}", true),
            ("*\u{fe0f}\u{20e3}", true),
            ("1\u{20e3}", true),
            ("#\u{20e3}", true),
            ("*\u{20e3}", true),
            ("👩‍💻", true),
            ("🇨🇳", true),
            ("👍🏽", true),
            ("\u{e000}", true),
            ("\u{0301}", true),
            ("\u{0378}", false),
            ("\u{0378}\u{fe0e}", false),
            ("\u{0378}\u{fe0f}", false),
            ("\u{10ffff}", false),
        ];
        for (text, covered) in cases {
            fonts.shape(text, 13.);
            // Match the user's shape/wait/poll path. Fixture resolution performs no
            // native IO; at most two worker publications may change this frame.
            let deadline = std::time::Instant::now() + HANG_GUARD;
            let mut invalidations = 0;
            while fonts.fallback_pending() {
                assert!(std::time::Instant::now() < deadline, "{text}");
                std::thread::park_timeout(std::time::Duration::from_millis(8));
                invalidations += usize::from(fonts.poll_fallbacks());
                fonts.shape(text, 13.);
            }
            assert!(invalidations <= 2, "{text}: {invalidations}");
            let glyphs = fonts.shape(text, 13.);
            assert!(!glyphs.is_empty(), "{text}");
            assert_eq!(
                glyphs.iter().all(|glyph| glyph.id != 0),
                covered,
                "{text}: {glyphs:?}"
            );
            for _ in 0..32 {
                fonts.shape(text, 13.);
                assert!(!fonts.poll_fallbacks(), "{text} invalidated again");
                assert!(!fonts.fallback_pending(), "{text} requested again");
            }
        }
        assert!(calls.lock().unwrap().values().all(|count| *count == 1));
    }
    fn settled_glyphs(fonts: &mut Fonts, text: &str) -> Vec<ShapedGlyph> {
        fonts.shape(text, 13.);
        let deadline = std::time::Instant::now() + HANG_GUARD;
        let mut invalidations = 0;
        while fonts.fallback_pending() {
            assert!(std::time::Instant::now() < deadline, "{text}");
            std::thread::park_timeout(std::time::Duration::from_millis(8));
            invalidations += usize::from(fonts.poll_fallbacks());
            fonts.shape(text, 13.);
        }
        assert!(invalidations <= 2, "{text}: {invalidations} invalidations");
        let glyphs = fonts.shape(text, 13.);
        assert!(!glyphs.is_empty(), "{text}");
        assert!(
            glyphs.iter().all(|g| g.id != 0),
            "{text}: {glyphs:?}: {:?}",
            fonts.metadata()
        );
        for _ in 0..32 {
            fonts.shape(text, 13.);
            assert!(
                !fonts.poll_fallbacks(),
                "{text}: invalidated after resolving"
            );
            assert!(
                !fonts.fallback_pending(),
                "{text}: requested after resolving"
            );
        }
        glyphs
    }

    #[test]
    fn default_emoji_presentation_is_independent_of_text_face_load_order() {
        use vel_platform::fonts::{AsyncFontFallback, FontDescriptor, FontFallback};
        struct Resolver(PathBuf);
        impl FontFallback for Resolver {
            fn resolve(&mut self, _: &str, _: char) -> Option<FontDescriptor> {
                Some(FontDescriptor::file(
                    self.0.join("fallback-fixture.ttf"),
                    "VelocityFallback-Regular".into(),
                ))
            }
            fn resolve_cluster(&mut self, _: &str, _: &str) -> Option<FontDescriptor> {
                Some(FontDescriptor::file(
                    self.0.join("emoji-fixture.ttf"),
                    "VelocityEmoji-Regular".into(),
                ))
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        for (scalar, default_emoji) in [
            ('⌚', true),
            ('⌛', true),
            ('⏰', true),
            ('☕', true),
            ('⚡', true),
            ('❤', false),
        ] {
            for text_first in [false, true] {
                let mut fonts = Fonts::load_file(
                    &root.join("ligature-fixture.ttf"),
                    "VelocityFixture-Regular",
                )
                .unwrap();
                fonts.store.borrow_mut().fallback =
                    AsyncFontFallback::with_resolver(Resolver(root.clone()));
                let bare = scalar.to_string();
                let text = format!("{scalar}\u{fe0e}");
                let emoji = format!("{scalar}\u{fe0f}");
                let order = if text_first {
                    [&text, &bare, &emoji]
                } else {
                    [&bare, &emoji, &text]
                };
                for _ in 0..2 {
                    for cluster in order {
                        let expected_emoji =
                            cluster == &emoji || (cluster == &bare && default_emoji);
                        let glyphs = settled_glyphs(&mut fonts, cluster);
                        assert!(
                            glyphs
                                .iter()
                                .all(|g| fonts.store.borrow().faces[g.face as usize].color_glyphs
                                    == expected_emoji),
                            "{cluster} text_first={text_first}: {glyphs:?}"
                        );
                    }
                }
            }
        }
        // These lie inside the former blanket 1F000..1FAFF test but default to text.
        for c in ['🀀', '🌡', '🖥', '❤'] {
            assert!(!emoji_presentation(&c.to_string()));
        }
    }

    #[test]
    fn native_keycaps_cover_combining_marks_and_converge_with_every_selector() {
        for reversed in [false, true] {
            let mut fonts = Fonts::load().unwrap();
            for base in ['1', '#', '*'] {
                let selectors = if reversed {
                    ["\u{fe0f}", "\u{fe0e}", ""]
                } else {
                    ["\u{fe0e}", "", "\u{fe0f}"]
                };
                for selector in selectors {
                    let text = format!("{base}{selector}\u{20e3}");
                    let glyphs = settled_glyphs(&mut fonts, &text);
                    if selector == "\u{fe0e}" {
                        assert!(
                            glyphs
                                .iter()
                                .all(|g| !fonts.store.borrow().faces[g.face as usize].color_glyphs),
                            "{text}: text keycap lost its available text face"
                        );
                    }
                    assert!(
                        glyphs.iter().all(|g| {
                            let face = &fonts.store.borrow().faces[g.face as usize];
                            cluster_characters(&text)
                                .all(|c| face.reference().charmap().map(c) != 0)
                        }),
                        "{text}: fallback did not cover the whole cluster"
                    );
                }
            }
        }
    }

    #[test]
    fn native_vs15_color_only_fallbacks_stop_requesting_frames_after_resolution() {
        let mut fonts = Fonts::load().unwrap();
        for text in ["⌚\u{fe0e}", "🌡\u{fe0e}", "🖥\u{fe0e}", "a⌚\u{fe0e}b"] {
            fonts.shape(text, 13.);
            let deadline = std::time::Instant::now() + HANG_GUARD;
            let mut publications = 0;
            while fonts.fallback_pending() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "{text}: {:?}",
                    fonts.metadata()
                );
                std::thread::park_timeout(std::time::Duration::from_millis(8));
                publications += usize::from(fonts.poll_fallbacks());
                fonts.shape(text, 13.);
            }
            assert!(publications <= 2, "{text}: {publications}");
            let glyphs = fonts.shape(text, 13.);
            assert!(!glyphs.is_empty(), "{text}");
            assert!(
                glyphs.iter().all(|glyph| glyph.id != 0),
                "{text}: {:?}",
                fonts.metadata()
            );
            for _ in 0..64 {
                fonts.shape(text, 13.);
                std::thread::park_timeout(std::time::Duration::from_millis(8));
                assert!(!fonts.poll_fallbacks(), "{text} invalidated again");
                assert!(!fonts.fallback_pending(), "{text} requested again");
            }
        }
    }
}
