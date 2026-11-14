//! Content-addressed cache for parsed definitions; regex machines are never serialized.
use crate::{LoadingError, SyntaxSet, SyntaxSetBuilder};
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
    /// YAML definitions were parsed.
    Yaml,
    /// A binary index with matching source hashes was reused.
    Hit,
}
impl SyntaxSet {
    /// Loads a package tree, caching the parsed index under its source-content digest.
    /// Corrupt or incompatible cache entries are rebuilt from YAML.
    pub fn load_cached(root: &Path, cache_dir: &Path) -> Result<(Self, CacheStatus), LoadingError> {
        let mut paths = Vec::new();
        for e in walkdir::WalkDir::new(root) {
            let e = e.map_err(|e| LoadingError::WalkDir(Box::new(e)))?;
            if e.path().extension().is_some_and(|x| x == "sublime-syntax") {
                paths.push(e.into_path());
            }
        }
        paths.sort();
        let mut hash = blake3::Hasher::new();
        hash.update(b"velocity-syntax-v1-7");
        for path in &paths {
            hash.update(
                path.strip_prefix(root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .as_bytes(),
            );
            hash.update(&std::fs::read(path)?);
        }
        std::fs::create_dir_all(cache_dir)?;
        let path = cache_dir.join(format!("{}.bin", hash.finalize().to_hex()));
        if let Ok(bytes) = std::fs::read(&path)
            && bytes.len() >= 40
            && &bytes[..8] == MAGIC
            && blake3::hash(&bytes[40..]).as_bytes() == &bytes[8..40]
            && let Ok(set) = bincode::DefaultOptions::new()
                .with_fixint_encoding()
                .with_limit(bytes.len() as u64)
                .deserialize(&bytes[40..])
        {
            return Ok((set, CacheStatus::Hit));
        }
        let mut b = SyntaxSetBuilder::new();
        b.add_from_folder(root, true)?;
        let set = b.build();
        let payload = bincode::serialize(&set).map_err(std::io::Error::other)?;
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
        std::fs::rename(temp, path)?;
        Ok((set, CacheStatus::Yaml))
    }
    /// Loads one or more package directories without an on-disk cache.
    pub fn load_directories<P: AsRef<Path>>(roots: &[P]) -> Result<Self, LoadingError> {
        let mut b = SyntaxSetBuilder::new();
        for r in roots {
            b.add_from_folder(r, true)?;
        }
        Ok(b.build())
    }
}
