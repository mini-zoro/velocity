//! Ordered package resources shared by the syntax and preference loaders.
use crate::LoadingError;
use std::{collections::BTreeMap, path::Path};

pub(crate) struct Resource {
    pub path: String,
    pub bytes: Vec<u8>,
    pub physical_path: Option<String>,
    order: usize,
}

pub(crate) struct Resources {
    pub files: Vec<Resource>,
    pub digest: blake3::Hash,
}

fn hash_field(hash: &mut blake3::Hasher, bytes: &[u8]) {
    hash.update(&(bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

fn package_path(path: &str) -> Result<String, LoadingError> {
    let normalized = path.replace('\\', "/");
    let relative = normalized.strip_prefix("Packages/").unwrap_or(&normalized);
    let parts: Vec<_> = relative
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    if relative.starts_with('/')
        || parts.is_empty()
        || parts.iter().any(|s| *s == ".." || s.contains(':'))
    {
        return Err(LoadingError::InvalidPath(path.into()));
    }
    Ok(format!("Packages/{}", parts.join("/")))
}

impl Resources {
    pub fn collect(
        roots: &[&Path],
        extra: &[(&str, &str)],
        schema: &[u8],
        accepts: fn(&Path) -> bool,
    ) -> Result<Self, LoadingError> {
        let mut hash = blake3::Hasher::new();
        hash_field(&mut hash, schema);
        hash.update(&(roots.len() as u64).to_le_bytes());
        let mut winners = BTreeMap::new();
        for (order, root) in roots.iter().enumerate() {
            let mut paths = Vec::new();
            for entry in walkdir::WalkDir::new(root)
                .follow_links(true)
                .into_iter()
                .filter_entry(|e| e.file_name() != ".git")
            {
                let entry = match entry {
                    Ok(entry) => entry,
                    // Following package-directory links must not turn a dangling file
                    // link into a fatal traversal error. Loops and other errors surface.
                    Err(e)
                        if e.path().is_some_and(|p| p.is_symlink())
                            && e.io_error()
                                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
                    {
                        continue;
                    }
                    Err(e) => return Err(LoadingError::WalkDir(Box::new(e))),
                };
                if crate::tm_language::is_file_entry(&entry) && accepts(entry.path()) {
                    paths.push(entry.into_path());
                }
            }
            paths.sort();
            hash.update(&(paths.len() as u64).to_le_bytes());
            for path in paths {
                let relative = path.strip_prefix(root).expect("walk stays beneath root");
                let virtual_path = package_path(&relative.to_string_lossy())?;
                let bytes = std::fs::read(&path)?;
                hash_field(&mut hash, virtual_path.as_bytes());
                hash_field(&mut hash, &bytes);
                winners.insert(
                    virtual_path.clone(),
                    Resource {
                        path: virtual_path,
                        bytes,
                        physical_path: path.to_str().map(|p| p.replace('\\', "/")),
                        order,
                    },
                );
            }
        }
        hash.update(&(extra.len() as u64).to_le_bytes());
        for (i, &(path, source)) in extra.iter().enumerate() {
            let path = package_path(path)?;
            if !accepts(Path::new(&path)) {
                return Err(LoadingError::InvalidPath(path));
            }
            hash_field(&mut hash, path.as_bytes());
            hash_field(&mut hash, source.as_bytes());
            winners.insert(
                path.clone(),
                Resource {
                    path,
                    bytes: source.as_bytes().to_vec(),
                    physical_path: None,
                    order: roots.len() + i,
                },
            );
        }
        let mut files: Vec<_> = winners.into_values().collect();
        // Keep resource precedence for distinct paths that declare the same name,
        // scope or equally specific selector. Replacement is by full path only.
        let is_user = |path: &str| path.starts_with("Packages/User/");
        files.sort_by(|a, b| {
            (a.order, is_user(&a.path), &a.path).cmp(&(b.order, is_user(&b.path), &b.path))
        });
        Ok(Self {
            files,
            digest: hash.finalize(),
        })
    }
}

pub(crate) fn is_syntax(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "sublime-syntax")
        || crate::tm_language::is_tm_language(path)
}

pub(crate) fn is_preferences(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "tmPreferences")
}
