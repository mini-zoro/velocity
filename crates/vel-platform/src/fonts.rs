//! CoreText fallback selection behind a device-free interface.
use std::{collections::HashMap, path::PathBuf};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{font_families, font_named, font_style};

/// Font bytes retained independently of replaceable user font files.
#[derive(Debug)]
pub enum FontData {
    /// Immutable, OS-owned font resource.
    Mapped(memmap2::Mmap),
    /// Snapshot of a user, app or other replaceable font.
    Owned(Vec<u8>),
}
impl std::ops::Deref for FontData {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Self::Mapped(data) => data,
            Self::Owned(data) => data,
        }
    }
}
/// Shared file snapshots; collection faces reuse the same retained data.
pub type FontFiles = HashMap<PathBuf, std::sync::Arc<FontData>>;
#[derive(Clone, Copy)]
struct FontVolume {
    read_only: bool,
    fsid: [i32; 2],
}
struct MappingPolicy {
    root: Option<FontVolume>,
    firmlinks: Option<Vec<PathBuf>>,
}
/// MobileAsset font locations, including the canonical Data-volume spelling.
pub fn is_mobile_asset_path(path: &std::path::Path) -> bool {
    path.starts_with("/System/Library/AssetsV2")
        || path.starts_with("/System/Volumes/Data/System/Library/AssetsV2")
}

impl MappingPolicy {
    fn permits(&self, canonical: &std::path::Path, volume: Option<FontVolume>) -> bool {
        let (Some(volume), Some(firmlinks)) = (volume, &self.firmlinks) else {
            return false; // Missing mount information must never make a writable font a mapping.
        };
        !canonical.starts_with("/System/Volumes/Data")
            && !is_mobile_asset_path(canonical)
            && !firmlinks.iter().any(|path| canonical.starts_with(path))
            && (volume.read_only || self.root.is_some_and(|root| root.fsid == volume.fsid))
    }
}
#[cfg(target_os = "macos")]
fn font_volume(path: &std::path::Path) -> Option<FontVolume> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut info = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: a live NUL-terminated path and correctly sized writable statfs buffer.
    if unsafe { libc::statfs(path.as_ptr(), info.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: successful statfs initializes the entire returned structure. Darwin's
    // fsid_t is two int32_t values, including in libc's opaque representation.
    // https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/statfs.2.html
    let info = unsafe { info.assume_init() };
    Some(FontVolume {
        read_only: info.f_flags & libc::MNT_RDONLY as u32 != 0,
        // SAFETY: fsid_t's documented ABI is exactly two initialized int32_t values.
        fsid: unsafe { std::mem::transmute::<libc::fsid_t, [i32; 2]>(info.f_fsid) },
    })
}
#[cfg(not(target_os = "macos"))]
fn font_volume(_: &std::path::Path) -> Option<FontVolume> {
    None
}
fn mapping_policy() -> &'static MappingPolicy {
    static POLICY: std::sync::OnceLock<MappingPolicy> = std::sync::OnceLock::new();
    POLICY.get_or_init(|| MappingPolicy {
        root: font_volume(std::path::Path::new("/System/Library/Fonts")),
        // AssetsV2 is on Data, despite its /System prefix. Consult the host's
        // actual firmlink table instead of guessing immutability from directory names.
        firmlinks: std::fs::read_to_string("/usr/share/firmlinks")
            .ok()
            .map(|source| {
                source
                    .lines()
                    .filter_map(|line| line.split_whitespace().next().map(PathBuf::from))
                    .collect()
            }),
    })
}
/// Maps immutable system resources and snapshots replaceable fonts in full.
/// Renderers can read selected MobileAsset collection faces instead.
pub fn map_font_file(
    path: &std::path::Path,
    files: &mut FontFiles,
) -> Result<std::sync::Arc<FontData>, String> {
    let canonical = path.canonicalize().map_err(|e| e.to_string())?;
    if let Some(bytes) = files.get(&canonical) {
        return Ok(bytes.clone());
    }
    let immutable = mapping_policy().permits(&canonical, font_volume(&canonical));
    let data = if immutable {
        let file = std::fs::File::open(&canonical).map_err(|e| e.to_string())?;
        // SAFETY: canonicalization and mount/firmlink checks establish a read-only
        // system resource. User/app/mobile-asset fonts can be truncated or replaced
        // in place, so they are copied below. The Arc
        // retains the mapping for every borrowed table and collection face.
        FontData::Mapped(
            unsafe { memmap2::MmapOptions::new().map(&file) }.map_err(|e| e.to_string())?,
        )
    } else {
        FontData::Owned(std::fs::read(&canonical).map_err(|e| e.to_string())?)
    };
    let bytes = std::sync::Arc::new(data);
    files.insert(canonical, bytes.clone());
    Ok(bytes)
}
/// Stable identity of one face in a memory-mapped font file.
#[derive(Clone, Debug)]
pub struct FontDescriptor {
    /// Font file, including fonts in AssetsV2 and user directories.
    pub path: PathBuf,
    /// Face identity within collections.
    pub postscript_name: String,
    /// Retain the actual resolved face, including private cascade fonts.
    #[cfg(target_os = "macos")]
    pub native: Option<CascadeFont>,
}
impl FontDescriptor {
    /// Describes a file-backed face, useful for custom resolvers and headless fixtures.
    pub fn file(path: PathBuf, postscript_name: String) -> Self {
        Self {
            path,
            postscript_name,
            #[cfg(target_os = "macos")]
            native: None,
        }
    }
}
/// Injectable system font service; no AppKit or window is needed.
pub trait FontFallback {
    /// Selects the user's language cascade for a missing scalar.
    fn resolve(&mut self, primary: &str, scalar: char) -> Option<FontDescriptor>;
    /// Resolves the presentation of a whole emoji grapheme before scalar coverage.
    fn resolve_cluster(&mut self, primary: &str, cluster: &str) -> Option<FontDescriptor> {
        self.resolve(primary, cluster.chars().next()?)
    }
    /// Prefers text presentation for a whole cluster; the cascade may return color
    /// when no text face covers it. That answer is a final presentation choice too.
    fn resolve_text_cluster(&mut self, primary: &str, cluster: &str) -> Option<FontDescriptor> {
        self.resolve(primary, cluster.chars().next()?)
    }
}
impl<R: FontFallback + ?Sized> FontFallback for Box<R> {
    fn resolve(&mut self, primary: &str, scalar: char) -> Option<FontDescriptor> {
        (**self).resolve(primary, scalar)
    }
    fn resolve_cluster(&mut self, primary: &str, cluster: &str) -> Option<FontDescriptor> {
        (**self).resolve_cluster(primary, cluster)
    }
    fn resolve_text_cluster(&mut self, primary: &str, cluster: &str) -> Option<FontDescriptor> {
        (**self).resolve_text_cluster(primary, cluster)
    }
}
/// Characters that a single fallback face must cover for a grapheme. Combining
/// marks (including the enclosing keycap) are required; only presentation/joining
/// controls may be absent from the cmap. Shared by native selection and rendering.
pub fn cluster_characters(cluster: &str) -> impl Iterator<Item = char> + '_ {
    cluster
        .chars()
        .filter(|c| !matches!(*c as u32, 0x200d | 0xfe00..=0xfe0f | 0xe0100..=0xe01ef))
}
/// Native resolver. Only instantiated when a glyph is missing.
#[derive(Debug, Default)]
pub struct SystemFontFallback;
impl FontFallback for SystemFontFallback {
    fn resolve(&mut self, primary: &str, scalar: char) -> Option<FontDescriptor> {
        system_face(primary, &scalar.to_string())
    }
    fn resolve_cluster(&mut self, _: &str, cluster: &str) -> Option<FontDescriptor> {
        // CTFontCreateForString receives the whole presentation sequence. Starting
        // with the emoji face prevents text cmap coverage swallowing VS16/keycaps.
        system_face(
            if cfg!(windows) {
                "Segoe UI Emoji"
            } else {
                "AppleColorEmoji"
            },
            cluster,
        )
    }
    fn resolve_text_cluster(&mut self, primary: &str, cluster: &str) -> Option<FontDescriptor> {
        // Let CoreText evaluate VS15 with the primary's text cascade, including its
        // last-resort color face. A selector requests presentation, not missing glyphs.
        // https://developer.apple.com/documentation/coretext/ctfontcreateforstring(_:_:_:)
        // https://www.unicode.org/reports/tr51/#Presentation_Style
        system_face(primary, cluster)
    }
}
fn system_face(primary: &str, text: &str) -> Option<FontDescriptor> {
    #[cfg(target_os = "macos")]
    {
        use objc2_core_foundation::{CFRange, CFString, CFURL};
        use objc2_core_text::{CTFont, CTFontDescriptor, CTFontUIFontType, kCTFontURLAttribute};
        // Private system faces (".SFNS-Regular", ".AppleSystemUIFont") must not round-trip
        // through CTFontCreateWithName: CoreText substitutes Times New Roman, and the whole
        // fallback cascade (CJK, symbols) would then come from a serif family.
        let primary = if primary.starts_with('.') || matches!(primary, "system" | "system-ui") {
            // SAFETY: the system UI type, positive size and default language are valid.
            unsafe { CTFont::new_ui_font_for_language(CTFontUIFontType::System, 13., None) }?
        } else {
            // SAFETY: strings and the in-bounds UTF-16 range outlive the call.
            unsafe { CTFont::with_name(&CFString::from_str(primary), 13., std::ptr::null()) }
        };
        // SAFETY: the retained font/string and its in-bounds UTF-16 range are valid.
        let mut font = unsafe {
            primary.for_string(
                &CFString::from_str(text),
                CFRange::new(0, text.encode_utf16().count() as isize),
            )
        };
        let covers = |font: &CTFont| {
            // SAFETY: an immutable character set retained independently of this font.
            let characters = unsafe { font.character_set() };
            cluster_characters(text).all(|c| characters.is_long_character_member(c as u32))
        };
        if !covers(&font) {
            // CTFontCreateForString may return a face for the first run only (e.g.
            // Monaco's digit without U+20E3). Continue the ordered cascade until one
            // face covers every base and combining mark, using the same rule as
            // the renderer's final cmap validation. This runs only on the worker.
            // https://developer.apple.com/documentation/coretext/ctfontcopydefaultcascadelistforlanguages(_:_:)
            // SAFETY: no language array is supplied; the retained primary is valid.
            let cascade = unsafe { primary.default_cascade_list_for_languages(None) }?;
            // SAFETY: CoreText documents this array's elements as CTFontDescriptors.
            let cascade = unsafe { cascade.cast_unchecked::<CTFontDescriptor>() };
            font = cascade.iter().find_map(|descriptor| {
                // SAFETY: a retained descriptor, with an optional null transform.
                let candidate =
                    unsafe { CTFont::with_font_descriptor(&descriptor, 13., std::ptr::null()) };
                covers(&candidate).then_some(candidate)
            })?;
        }
        // SAFETY: immutable exported key and a live retained font.
        let url = unsafe { font.attribute(kCTFontURLAttribute) }?
            .downcast::<CFURL>()
            .ok()?;
        Some(FontDescriptor {
            path: url.to_file_path()?,
            // SAFETY: immutable name read from the retained face.
            postscript_name: unsafe { font.post_script_name() }.to_string(),
            native: Some(CascadeFont(font)),
        })
    }
    #[cfg(windows)]
    return windows::fallback(primary, text);
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (primary, text);
        None
    }
}
/// Identity of a scalar miss or a presentation-sensitive whole cluster.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FallbackKey {
    /// Ordinary missing scalar.
    Scalar(char),
    /// Emoji presentation sequence.
    Emoji(String),
    /// Text presentation preference, including a settled color-only fallback.
    Text(String),
}
/// Caches successful and unavailable fallback choices per primary face and scalar.
/// Kept separate from loaded font data so tests can substitute the system service.
#[derive(Debug)]
pub struct FallbackCache<R> {
    resolver: R,
    choices: HashMap<(u16, char), Option<FontDescriptor>>,
    bytes: usize,
}
impl<R: FontFallback> FallbackCache<R> {
    /// Empty cache; performs no system work.
    pub fn new(resolver: R) -> Self {
        Self {
            resolver,
            choices: HashMap::new(),
            bytes: 0,
        }
    }
    /// Resolves once, including missing private-use glyphs with no installed coverage.
    pub fn resolve(&mut self, face: u16, primary: &str, scalar: char) -> Option<&FontDescriptor> {
        let key = (face, scalar);
        if !self.choices.contains_key(&key) {
            const BYTES: usize = 2 * 1024 * 1024;
            let choice = self.resolver.resolve(primary, scalar);
            let charge = 128
                + choice
                    .as_ref()
                    .map_or(0, |d| d.path.capacity() + d.postscript_name.capacity());
            if charge > BYTES {
                return None;
            }
            if self.choices.len() == 8192 || self.bytes + charge > BYTES {
                self.choices.clear();
                self.bytes = 0;
            }
            self.choices.insert(key, choice);
            self.bytes += charge;
        }
        self.choices[&key].as_ref()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_font_mapping_policy_uses_mocked_mounts_and_firmlinks() {
        let root = FontVolume {
            read_only: true,
            fsid: [1, 2],
        };
        let data = FontVolume {
            read_only: false,
            fsid: [3, 4],
        };
        let policy = MappingPolicy {
            root: Some(root),
            firmlinks: Some(vec!["/Users".into(), "/Library".into()]),
        };
        // These are the statfs results after canonicalizing each path, including aliases.
        for (path, volume, expected) in [
            ("/System/Library/Fonts/Menlo.ttc", Some(root), true),
            (
                "/System/Library/PrivateFrameworks/FontServices.framework/Versions/A/Resources/Reserved/PingFangUI.ttc",
                Some(root),
                true,
            ),
            (
                "/System/Library/Fonts/Menlo.ttc",
                Some(FontVolume {
                    read_only: false,
                    ..root
                }),
                true,
            ),
            (
                "/System/Library/Fonts/Menlo.ttc",
                Some(FontVolume {
                    read_only: true,
                    ..data
                }),
                true,
            ),
            (
                "/System/Library/AssetsV2/font.asset/PingFang.ttc",
                Some(data),
                false,
            ),
            (
                "/System/Library/AssetsV2/font.asset/PingFang.ttc",
                Some(root),
                false,
            ),
            (
                "/System/Volumes/Data/System/Library/AssetsV2/font.asset/PingFang.ttc",
                Some(root),
                false,
            ),
            ("/Users/example/Library/Fonts/font.ttf", Some(data), false),
            ("/Library/Fonts/font.ttf", Some(data), false),
            ("/private/tmp/font.ttf", Some(data), false),
            ("/System/Library/Fonts/font.ttf", None, false),
        ] {
            assert_eq!(
                policy.permits(std::path::Path::new(path), volume),
                expected,
                "{path}"
            );
        }
        let unknown = MappingPolicy {
            root: None,
            firmlinks: None,
        };
        assert!(!unknown.permits(
            std::path::Path::new("/System/Library/Fonts/font.ttf"),
            Some(root)
        ));
    }
    #[test]
    fn scripts_and_unavailable_private_use_are_cached_independently() {
        #[derive(Debug, Default)]
        struct Mock {
            calls: usize,
        }
        impl FontFallback for Mock {
            fn resolve(&mut self, _: &str, scalar: char) -> Option<FontDescriptor> {
                self.calls += 1;
                (scalar != '\u{e000}').then(|| FontDescriptor {
                    path: "fixture.ttc".into(),
                    postscript_name: format!("Script-{}", scalar as u32),
                    #[cfg(target_os = "macos")]
                    native: None,
                })
            }
        }
        let mut cache = FallbackCache::new(Mock::default());
        assert_eq!(cache.resolver.calls, 0);
        for _ in 0..3 {
            for c in ['한', 'ไ', 'अ', 'א', 'ع', '𝒜', '⠿', '\u{e000}'] {
                assert_eq!(cache.resolve(0, "primary", c).is_some(), c != '\u{e000}');
            }
        }
        assert_eq!(cache.resolver.calls, 8);
    }
}

/// Nonblocking cascade worker. Missing faces never hold up the first frame.
#[derive(Default)]
pub struct AsyncFontFallback {
    resolver: Option<Box<dyn FontFallback + Send>>,
    worker: Option<std::sync::mpsc::Sender<(u32, String, FallbackKey)>>,
    ready: Option<std::sync::mpsc::Receiver<(u32, FallbackKey, Option<FontDescriptor>)>>,
    pending: std::collections::HashSet<(u32, FallbackKey)>,
}
impl AsyncFontFallback {
    const IN_FLIGHT: usize = 128;

    /// Supplies an alternate cascade service without starting it before the first miss.
    pub fn with_resolver(resolver: impl FontFallback + Send + 'static) -> Self {
        Self {
            resolver: Some(Box::new(resolver)),
            ..Self::default()
        }
    }
    /// Enqueues one request per (cascade id, key); returns true only on enqueue.
    pub fn request(&mut self, face: u32, name: &str, key: FallbackKey) -> bool {
        const CLUSTER_BYTES: usize = 8 * 1024;
        if self.is_full()
            || matches!(&key, FallbackKey::Emoji(s) | FallbackKey::Text(s) if s.len() > CLUSTER_BYTES)
            || !self.pending.insert((face, key.clone()))
        {
            return false;
        }
        if self.worker.is_none() {
            let (tx, rx) = std::sync::mpsc::channel::<(u32, String, FallbackKey)>();
            let (results, ready) = std::sync::mpsc::channel();
            let mut resolver = self
                .resolver
                .take()
                .unwrap_or_else(|| Box::new(SystemFontFallback));
            match std::thread::Builder::new()
                .name("font-cascade".into())
                .spawn(move || {
                    while let Ok((face, name, key)) = rx.recv() {
                        // The renderer owns both positive and negative choices.
                        let result = match &key {
                            FallbackKey::Scalar(c) => resolver.resolve(&name, *c),
                            FallbackKey::Emoji(s) => resolver.resolve_cluster(&name, s),
                            FallbackKey::Text(s) => resolver.resolve_text_cluster(&name, s),
                        };
                        if results.send((face, key, result)).is_err() {
                            break;
                        }
                    }
                }) {
                Ok(_) => {
                    self.worker = Some(tx);
                    self.ready = Some(ready);
                }
                Err(_) => {
                    self.pending.remove(&(face, key.clone()));
                    return false;
                }
            }
        }
        if self
            .worker
            .as_ref()
            .is_none_or(|tx| tx.send((face, name.into(), key.clone())).is_err())
        {
            self.pending.remove(&(face, key));
            return false;
        }
        true
    }
    /// Takes completed selections, including misses, without waiting.
    pub fn poll(&mut self) -> Vec<(u32, FallbackKey, Option<FontDescriptor>)> {
        let mut results = Vec::new();
        let mut disconnected = false;
        if let Some(rx) = &self.ready {
            loop {
                match rx.try_recv() {
                    Ok(result) => {
                        self.pending.remove(&(result.0, result.1.clone()));
                        results.push(result);
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        if disconnected {
            for (face, scalar) in self.pending.drain() {
                results.push((face, scalar, None));
            }
            self.worker = None;
            self.ready = None;
        }
        results
    }
    /// Whether another frame should check for completion.
    pub fn pending(&self) -> bool {
        !self.pending.is_empty()
    }
    /// Whether loading a different cascade needs to wait for a free worker slot.
    pub fn is_full(&self) -> bool {
        self.pending.len() == Self::IN_FLIGHT
    }
}

/// An immutable retained CoreText cascade face transferred from the font worker.
#[cfg(target_os = "macos")]
#[derive(Clone, Debug)]
pub struct CascadeFont(objc2_core_foundation::CFRetained<objc2_core_text::CTFont>);
#[cfg(target_os = "macos")]
// SAFETY: CTFontCreateForString returns an immutable CoreText font, never a mutable
// AppKit object. Apple's Core Text thread-safety contract explicitly permits font
// objects to cross threads: https://developer.apple.com/documentation/coretext/
unsafe impl Send for CascadeFont {}
#[cfg(target_os = "macos")]
impl CascadeFont {
    /// Transfers the actual retained face to the renderer without resolving its name again.
    pub fn into_native(self) -> objc2_core_foundation::CFRetained<objc2_core_text::CTFont> {
        self.0
    }
}

impl std::fmt::Debug for AsyncFontFallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncFontFallback")
            .field("pending", &self.pending)
            .finish_non_exhaustive()
    }
}
