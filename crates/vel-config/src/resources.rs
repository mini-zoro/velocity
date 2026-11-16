//! Package resources with zip/loose overrides and deterministic User-last order.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
};
/// Effective resource content and provenance.
#[derive(Clone, Debug)]
pub struct Resource {
    /// `Packages/<package>/<relative path>`.
    pub path: String,
    /// UTF-8 data (validated by `text`).
    pub bytes: Vec<u8>,
    /// Archive or loose file on disk.
    pub origin: PathBuf,
}
impl Resource {
    /// Validates UTF-8 without altering BOMs or comments.
    pub fn text(&self) -> Result<&str, String> {
        std::str::from_utf8(&self.bytes).map_err(|e| format!("{}: {e}", self.path))
    }
}
/// Resource overlay. Add bundled resources, then installed/user data directories.
#[derive(Default, Debug)]
pub struct Resources {
    entries: BTreeMap<String, Resource>,
    ignored: BTreeSet<String>,
}
impl Resources {
    /// Adds an embedded resource at lowest or current priority.
    pub fn insert(&mut self, path: &str, bytes: Vec<u8>, origin: PathBuf) {
        self.entries.insert(
            path.into(),
            Resource {
                path: path.into(),
                bytes,
                origin,
            },
        );
    }
    /// Loads installed archives then loose files. Loose files override archives.
    pub fn load(&mut self, data: &Path) -> Result<(), String> {
        let installed = data.join("Installed Packages");
        if installed.is_dir() {
            let mut paths = fs::read_dir(&installed)
                .map_err(|e| e.to_string())?
                .map(|e| e.map(|e| e.path()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            paths.sort();
            for path in paths
                .into_iter()
                .filter(|p| p.extension().is_some_and(|e| e == "sublime-package"))
            {
                let package = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or("non-UTF8 package")?;
                let mut archive =
                    zip::ZipArchive::new(fs::File::open(&path).map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?;
                for i in 0..archive.len() {
                    let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
                    if file.is_dir() {
                        continue;
                    }
                    let Some(relative) = file.enclosed_name() else {
                        return Err("unsafe archive resource path".into());
                    };
                    if file.size() > 32 * 1024 * 1024 {
                        return Err("resource exceeds 32 MiB limit".into());
                    }
                    let mut bytes = Vec::new();
                    file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                    self.insert(
                        &format!("Packages/{package}/{}", relative.to_string_lossy()),
                        bytes,
                        path.clone(),
                    );
                }
            }
        }
        let packages = data.join("Packages");
        if packages.is_dir() {
            self.loose(&packages, &packages)?;
        }
        Ok(())
    }
    fn loose(&mut self, root: &Path, dir: &Path) -> Result<(), String> {
        let mut entries = fs::read_dir(dir)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let ty = entry.file_type().map_err(|e| e.to_string())?;
            let path = entry.path();
            if ty.is_dir() {
                self.loose(root, &path)?;
            } else if ty.is_file() {
                let relative = path.strip_prefix(root).map_err(|e| e.to_string())?;
                self.insert(
                    &format!("Packages/{}", relative.to_string_lossy()),
                    fs::read(&path).map_err(|e| e.to_string())?,
                    path,
                );
            }
        }
        Ok(())
    }
    /// Applies the effective package exclusion to every resource consumer.
    /// User preferences always remain readable so a disabled package can be enabled.
    pub fn set_ignored(&mut self, ignored: BTreeSet<String>) {
        self.ignored = ignored;
        self.ignored.remove("User");
    }
    /// Whether a package contributes resources in this overlay.
    pub fn package_enabled(&self, package: &str) -> bool {
        !self.ignored.contains(package)
    }
    /// Adds only public syntax and metadata resources from a package root.
    pub fn load_syntaxes(&mut self, root: &Path) -> Result<(), String> {
        fn visit(resources: &mut Resources, root: &Path, dir: &Path) -> Result<(), String> {
            for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                let ty = entry.file_type().map_err(|e| e.to_string())?;
                let path = entry.path();
                if ty.is_dir() {
                    visit(resources, root, &path)?;
                } else if ty.is_file()
                    && path.to_str().is_some_and(|p| {
                        [
                            ".sublime-syntax",
                            ".tmLanguage",
                            ".tmLanguage.json",
                            ".tmPreferences",
                        ]
                        .iter()
                        .any(|suffix| p.ends_with(suffix))
                    })
                {
                    let relative = path.strip_prefix(root).map_err(|e| e.to_string())?;
                    resources.insert(
                        &format!("Packages/{}", relative.to_string_lossy()),
                        fs::read(&path).map_err(|e| e.to_string())?,
                        path,
                    );
                }
            }
            Ok(())
        }
        visit(self, root, root)
    }
    /// Finds one effective package resource.
    pub fn get(&self, path: &str) -> Option<&Resource> {
        self.entries
            .get(path)
            .filter(|r| self.package_enabled(r.path.split('/').nth(1).unwrap_or("")))
    }
    /// Lists an extension in package order: Default first, User last.
    pub fn matching(&self, suffix: &str) -> Vec<&Resource> {
        let mut out: Vec<_> = self
            .entries
            .values()
            .filter(|r| {
                r.path.ends_with(suffix)
                    && self.package_enabled(r.path.split('/').nth(1).unwrap_or(""))
            })
            .collect();
        out.sort_by_key(|r| {
            let package = r.path.split('/').nth(1).unwrap_or("");
            (
                if package == "Default" {
                    0
                } else if package == "User" {
                    2
                } else {
                    1
                },
                r.path.contains("(OSX)"),
                &r.path,
            )
        });
        out
    }
}
