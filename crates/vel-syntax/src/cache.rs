//! Content-addressed cache for parsed definitions; regex machines are never serialized.
use crate::{
    LoadingError, SyntaxSet, SyntaxSetBuilder,
    resources::{Resources, is_syntax},
};
use bincode::Options;
use std::{
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};
static CACHE_WRITES: AtomicU64 = AtomicU64::new(0);
const MAGIC: &[u8; 8] = b"VELSYN01";
/// Source used for constructing the syntax index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheStatus {
    /// Source definitions were parsed (YAML or TextMate).
    Yaml,
    /// A binary index with matching source hashes was reused.
    Hit,
}
impl SyntaxSet {
    /// Loads a package tree, caching the parsed index under its source-content digest.
    /// Corrupt or incompatible cache entries are rebuilt from source definitions.
    pub fn load_cached(root: &Path, cache_dir: &Path) -> Result<(Self, CacheStatus), LoadingError> {
        Self::load_cached_roots(&[root], &[], cache_dir)
    }

    /// Loads ordered package roots and in-memory definitions through one binary cache.
    ///
    /// Later roots replace earlier resources at the same package-relative path;
    /// `extra` entries override every root, in slice order. Virtual paths accept
    /// `Packages/Pkg/File.sublime-syntax` or `Pkg/File.sublime-syntax` (also both
    /// TextMate extensions). Each root contains packages; its `User/` resources
    /// load last, after the other packages in that root.
    /// Missing roots and traversal errors are reported; omit optional absent roots.
    /// File and directory symlinks are followed; dangling links are ignored.
    ///
    /// The digest includes the ordered contents of every input, even shadowed
    /// definitions. Source bytes are read once and reused on a miss. Malformed
    /// definitions produce the same warnings as directory loading. The cache stores
    /// canonical virtual paths, so moving identical roots does not stale the index.
    pub fn load_cached_roots(
        roots: &[&Path],
        extra: &[(&str, &str)],
        cache_dir: &Path,
    ) -> Result<(Self, CacheStatus), LoadingError> {
        let sources = Resources::collect(roots, extra, b"velocity-syntax-roots-v1-14", is_syntax)?;
        let path = cache_dir.join(format!("{}.bin", sources.digest.to_hex()));
        if let Some(mut set) = read_cache::<Self>(&path) {
            set.bind_physical_paths(&sources);
            return Ok((set, CacheStatus::Hit));
        }
        let set = Self::from_resources(sources);
        write_cache(&path, &set)?;
        Ok((set, CacheStatus::Yaml))
    }

    /// Loads ordered roots and virtual definitions with the same overrides as
    /// [`Self::load_cached_roots`], without reading or writing a binary cache.
    pub fn load_roots(roots: &[&Path], extra: &[(&str, &str)]) -> Result<Self, LoadingError> {
        Ok(Self::from_resources(Resources::collect(
            roots, extra, b"", is_syntax,
        )?))
    }

    fn from_resources(sources: Resources) -> Self {
        let mut builder = SyntaxSetBuilder::new();
        for source in &sources.files {
            builder.add_resource(&source.path, &source.bytes);
        }
        let mut set = builder.build();
        set.bind_physical_paths(&sources);
        set
    }

    fn bind_physical_paths(&mut self, sources: &Resources) {
        self.bind_resource_paths(sources.files.iter().filter_map(|source| {
            source
                .physical_path
                .as_deref()
                .map(|physical| (source.path.as_str(), physical))
        }));
    }
    /// Loads one or more package directories without an on-disk cache.
    pub fn load_directories<P: AsRef<Path>>(roots: &[P]) -> Result<Self, LoadingError> {
        Self::load_roots(&roots.iter().map(AsRef::as_ref).collect::<Vec<_>>(), &[])
    }
}

pub(crate) fn read_cache<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 40
        || &bytes[..8] != MAGIC
        || blake3::hash(&bytes[40..]).as_bytes() != &bytes[8..40]
    {
        return None;
    }
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(bytes.len() as u64)
        .deserialize(&bytes[40..])
        .ok()
}

pub(crate) fn write_cache<T: serde::Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    std::fs::create_dir_all(path.parent().expect("cache has a parent"))?;
    let payload = bincode::serialize(value).map_err(std::io::Error::other)?;
    let mut bytes = Vec::with_capacity(40 + payload.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(blake3::hash(&payload).as_bytes());
    bytes.extend_from_slice(&payload);
    let temp = path.with_extension(format!(
        "{}-{}.tmp",
        std::process::id(),
        CACHE_WRITES.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&temp, bytes)?;
    let result = std::fs::rename(&temp, path);
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}
