use crate::{Result, Version, invalid};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read},
    path::{Component, Path},
};

pub(crate) const MAX_ARCHIVE: u64 = 128 * 1024 * 1024;
const MAX_RESOURCE: u64 = 32 * 1024 * 1024;
const MAX_EXPANDED: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;

/// Declarative resources understood by Velocity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceKind {
    /// `.sublime-syntax` and TextMate grammars.
    Syntax,
    /// `.sublime-color-scheme` and `.tmTheme`.
    ColorScheme,
    /// `.sublime-theme`.
    Theme,
    /// `.sublime-keymap`.
    Keymap,
    /// `.sublime-snippet`.
    Snippet,
    /// `.sublime-completions`.
    Completions,
    /// `.sublime-build`.
    BuildSystem,
}
/// A resource contributed by a package, relative to that package's root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contribution {
    /// Recognized format family.
    pub kind: ResourceKind,
    /// Path in the archive/loose package.
    pub path: String,
}
/// Package compatibility, independent of installation success.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    /// No Python plugin files were found.
    Declarative,
    /// Partially supported: no plugin API. Declarative resources remain usable.
    PartialNoPluginApi,
}
impl Support {
    /// UI-ready compatibility explanation.
    pub fn label(self) -> &'static str {
        match self {
            Self::Declarative => "declarative resources supported",
            Self::PartialNoPluginApi => "partially supported: no plugin API",
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct Inventory {
    paths: BTreeMap<String, ()>,
    metadata: Option<Vec<u8>>,
}
impl Inventory {
    pub fn overlay(&mut self, other: Self) {
        self.paths.extend(other.paths);
        if other.metadata.is_some() {
            self.metadata = other.metadata;
        }
    }
    pub fn version(&self) -> Option<Version> {
        let metadata: serde_json::Value = serde_json::from_slice(self.metadata.as_ref()?).ok()?;
        Version::parse(metadata.get("version")?.as_str()?).ok()
    }
    pub fn finish(self) -> (Vec<Contribution>, Vec<String>, Option<Version>) {
        let version = self.version();
        let mut resources = Vec::new();
        let mut plugins = Vec::new();
        for path in self.paths.into_keys() {
            if path.ends_with(".py") {
                plugins.push(path.clone());
            }
            let kind = if [".sublime-syntax", ".tmLanguage", ".tmLanguage.json"]
                .iter()
                .any(|s| path.ends_with(s))
            {
                Some(ResourceKind::Syntax)
            } else if path.ends_with(".sublime-color-scheme") || path.ends_with(".tmTheme") {
                Some(ResourceKind::ColorScheme)
            } else if path.ends_with(".sublime-theme") {
                Some(ResourceKind::Theme)
            } else if path.ends_with(".sublime-keymap") {
                Some(ResourceKind::Keymap)
            } else if path.ends_with(".sublime-snippet") {
                Some(ResourceKind::Snippet)
            } else if path.ends_with(".sublime-completions") {
                Some(ResourceKind::Completions)
            } else if path.ends_with(".sublime-build") {
                Some(ResourceKind::BuildSystem)
            } else {
                None
            };
            if let Some(kind) = kind {
                resources.push(Contribution { kind, path });
            }
        }
        (resources, plugins, version)
    }
}

pub(crate) fn inspect(path: &Path) -> Result<Inventory> {
    let file = fs::File::open(path)?;
    if file.metadata()?.len() > MAX_ARCHIVE {
        return Err(invalid("archive exceeds 128 MiB"));
    }
    let mut zip = zip::ZipArchive::new(file).map_err(invalid)?;
    if zip.len() > MAX_ENTRIES {
        return Err(invalid("too many archive entries"));
    }
    if zip.is_empty() {
        return Err(invalid("empty package archive"));
    }
    let mut inventory = Inventory::default();
    let mut names = BTreeSet::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(invalid)?;
        let name = entry.name().to_owned();
        if name.contains('\\')
            || name.contains('\0')
            || name.contains(':')
            || Path::new(&name)
                .components()
                .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
            || entry.enclosed_name().is_none()
        {
            return Err(invalid("unsafe archive path"));
        }
        // Normalization and case folding avoid ambiguous overlays on default macOS volumes.
        let normalized = Path::new(&name)
            .components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/");
        if normalized.is_empty() || !names.insert(normalized.to_lowercase()) {
            return Err(invalid("duplicate or empty archive path"));
        }
        if let Some(mode) = entry.unix_mode() {
            let kind = mode & 0o170000;
            if kind != 0 && kind != 0o100000 && kind != 0o040000 {
                return Err(invalid("archive links and special files are unsupported"));
            }
        }
        if entry.is_dir() {
            continue;
        }
        if entry.size() > MAX_RESOURCE {
            return Err(invalid("resource exceeds 32 MiB"));
        }
        // Read through EOF: this checks actual data length, decompression errors and CRC.
        let is_metadata = normalized == "package-metadata.json";
        let actual_size = if is_metadata {
            let mut bytes = Vec::new();
            entry
                .by_ref()
                .take(MAX_RESOURCE + 1)
                .read_to_end(&mut bytes)?;
            let size = bytes.len() as u64;
            inventory.metadata = Some(bytes);
            size
        } else {
            io::copy(&mut entry.by_ref().take(MAX_RESOURCE + 1), &mut io::sink())?
        };
        if actual_size > MAX_RESOURCE || actual_size != entry.size() {
            return Err(invalid("oversized resource"));
        }
        total = total
            .checked_add(actual_size)
            .ok_or_else(|| invalid("archive size overflow"))?;
        if total > MAX_EXPANDED {
            return Err(invalid("expanded package exceeds 256 MiB"));
        }
        inventory.paths.insert(normalized, ());
    }
    Ok(inventory)
}
pub(crate) fn inspect_loose(root: &Path) -> Result<Inventory> {
    fn visit(root: &Path, dir: &Path, inventory: &mut Inventory, depth: usize) -> Result<()> {
        if depth > 64 {
            return Err(invalid("loose package nesting exceeds 64 directories"));
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            let path = entry.path();
            if ty.is_dir() {
                visit(root, &path, inventory, depth + 1)?;
            } else if ty.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .map_err(invalid)?
                    .to_str()
                    .ok_or_else(|| invalid("non-UTF8 resource path"))?
                    .to_owned();
                if relative == "package-metadata.json" {
                    if entry.metadata()?.len() > MAX_RESOURCE {
                        return Err(invalid("oversized metadata"));
                    }
                    inventory.metadata = Some(fs::read(&path)?);
                }
                inventory.paths.insert(relative, ());
                if inventory.paths.len() > MAX_ENTRIES {
                    return Err(invalid("too many loose resources"));
                }
            }
            // Symlinks are intentionally not traversed.
        }
        Ok(())
    }
    let mut inventory = Inventory::default();
    visit(root, root, &mut inventory, 0)?;
    Ok(inventory)
}
