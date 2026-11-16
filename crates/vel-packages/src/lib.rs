//! Local package management, with explicitly requested HTTPS downloads and no plugin runtime.
//!
//! Construction and discovery never access the network. A caller must supply an index URL
//! and explicitly call [`PackageManager::check_upgrades`] to fetch an index.
#![forbid(unsafe_code)]

mod archive;
pub mod cli;
pub mod https;
mod index;

pub use archive::{Contribution, ResourceKind, Support};
pub use index::{Index, Release, Upgrade};
pub use semver::Version;

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};
use tempfile::NamedTempFile;
use vel_config::json::Document;

/// Package operation failure. Invalid input never replaces an installed archive.
#[derive(Debug)]
pub enum Error {
    /// Filesystem or transport process failure.
    Io(io::Error),
    /// Malformed or unsupported package, settings, index, or request.
    Invalid(String),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => e.fmt(f),
            Self::Invalid(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for Error {}
impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
/// Result of a package or CLI installation operation.
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn invalid(message: impl ToString) -> Error {
    Error::Invalid(message.to_string())
}

/// One source contributing to an installed package, ordered archive then loose overlay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Location {
    /// A ZIP archive inside `Installed Packages`.
    Archive(PathBuf),
    /// A directory inside `Packages`.
    Loose(PathBuf),
}

/// Effective resources after loose files have overlaid archive files of the same name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    /// Directory/archive basename, without the archive extension.
    pub name: String,
    /// Both sources are included when the package has loose overrides.
    pub locations: Vec<Location>,
    /// Version from effective `package-metadata.json`, when present and valid SemVer.
    pub version: Option<Version>,
    /// Whether the user-level `ignored_packages` setting disables this package.
    pub disabled: bool,
    /// Recognized declarative resources, sorted by relative path.
    pub contributions: Vec<Contribution>,
    /// Python paths, reported for compatibility only; never executed.
    pub python_plugins: Vec<String>,
}
impl Package {
    /// Describes support without rejecting packages that also contain Python plugins.
    pub fn support(&self) -> Support {
        if self.python_plugins.is_empty() {
            Support::Declarative
        } else {
            Support::PartialNoPluginApi
        }
    }
}

/// Installed archive and an optional recoverable copy of its previous version.
#[derive(Clone, Debug)]
pub struct Installed {
    /// Final archive path, visible to the resource loader after one atomic rename.
    pub path: PathBuf,
    /// Previous archive, retained under `data/.velocity-backups` when replacing a package.
    pub backup: Option<PathBuf>,
}

/// Package store for one app data directory. No writes occur during construction.
#[derive(Clone, Debug)]
pub struct PackageManager {
    data: PathBuf,
    index_url: Option<String>,
}
impl PackageManager {
    /// Uses `data/Packages`, `data/Installed Packages`, and `Packages/User` settings.
    pub fn new(data: impl Into<PathBuf>) -> Self {
        Self {
            data: resolve_root(data.into()),
            index_url: None,
        }
    }
    /// Returns the app data directory.
    pub fn data_dir(&self) -> &Path {
        &self.data
    }
    /// Sets an optional user-selected HTTPS index. This performs no request.
    pub fn set_index_url(&mut self, url: Option<String>) -> Result<()> {
        if let Some(url) = &url {
            https::validate_url(url)?;
        }
        self.index_url = url;
        Ok(())
    }
    /// Returns the explicit index URL; a new manager has none.
    pub fn index_url(&self) -> Option<&str> {
        self.index_url.as_deref()
    }
    /// Lists loose and archived packages in deterministic order without accessing the network.
    /// Symlinks are not followed. Malformed archives are returned as errors with their paths.
    pub fn list(&self) -> Result<Vec<Package>> {
        let ignored = self.ignored_packages()?;
        let mut packages: BTreeMap<String, (Vec<Location>, archive::Inventory)> = BTreeMap::new();
        for entry in read_dir(&self.data.join("Installed Packages"))? {
            let path = entry.path();
            if !entry.file_type()?.is_file()
                || path.extension().is_none_or(|s| s != "sublime-package")
            {
                continue;
            }
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| invalid("non-UTF8 package name"))?;
            validate_name(name)?;
            let inventory =
                archive::inspect(&path).map_err(|e| invalid(format!("{}: {e}", path.display())))?;
            packages.insert(name.into(), (vec![Location::Archive(path)], inventory));
        }
        for entry in read_dir(&self.data.join("Packages"))? {
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("non-UTF8 package name"))?;
            if name.starts_with('.') {
                continue;
            }
            validate_name(&name)?;
            let (locations, inventory) = packages.entry(name).or_default();
            let loose = archive::inspect_loose(&entry.path())?;
            inventory.overlay(loose);
            locations.push(Location::Loose(entry.path()));
        }
        Ok(packages
            .into_iter()
            .map(|(name, (locations, inventory))| {
                let (contributions, python_plugins, version) = inventory.finish();
                Package {
                    disabled: ignored.contains(&name),
                    name,
                    locations,
                    version,
                    contributions,
                    python_plugins,
                }
            })
            .collect())
    }
    /// Validates all ZIP entries and CRCs before atomically installing a local archive.
    /// Existing archives are backed up; the caller's source file is never moved or changed.
    pub fn install_local(&self, name: &str, source: &Path) -> Result<Installed> {
        validate_mutable_name(name)?;
        let _lock = self.lock()?;
        let mut temp = self.staging()?;
        let copied = io::copy(
            &mut fs::File::open(source)?.take(archive::MAX_ARCHIVE + 1),
            &mut temp,
        )?;
        if copied > archive::MAX_ARCHIVE {
            return Err(invalid("archive exceeds 128 MiB"));
        }
        self.commit_archive(name, temp)
    }
    /// Downloads over verified HTTPS into the destination filesystem, verifies the ZIP, and renames.
    pub fn install_https(
        &self,
        name: &str,
        url: &str,
        client: &https::HttpsClient,
    ) -> Result<Installed> {
        validate_mutable_name(name)?;
        https::validate_url(url)?;
        let _lock = self.lock()?;
        let temp = self.staging()?;
        client.download(url, temp.path(), archive::MAX_ARCHIVE)?;
        self.commit_archive(name, temp)
    }
    /// Installs an index entry only after verifying its SHA-256 and complete ZIP contents.
    pub fn install_release(
        &self,
        release: &Release,
        client: &https::HttpsClient,
    ) -> Result<Installed> {
        release.validate()?;
        let _lock = self.lock()?;
        let temp = self.staging()?;
        client.download(&release.url, temp.path(), archive::MAX_ARCHIVE)?;
        if index::sha256(temp.path())? != release.sha256.to_ascii_lowercase() {
            return Err(invalid("package SHA-256 mismatch"));
        }
        let inventory = archive::inspect(temp.path())?;
        if inventory.version() != Some(release.version.clone()) {
            return Err(invalid("package metadata version does not match index"));
        }
        self.commit_archive(&release.name, temp)
    }
    fn staging(&self) -> Result<NamedTempFile> {
        let dir = self.data.join("Installed Packages");
        ensure_dir(&dir)?;
        Ok(NamedTempFile::new_in(dir)?)
    }
    fn commit_archive(&self, name: &str, temp: NamedTempFile) -> Result<Installed> {
        archive::inspect(temp.path())?;
        let path = self
            .data
            .join("Installed Packages")
            .join(format!("{name}.sublime-package"));
        let backup = backup_file(&path, &self.data.join(".velocity-backups"))?;
        temp.as_file().sync_all()?;
        temp.persist(&path).map_err(|e| Error::Io(e.error))?;
        sync_dir(path.parent().expect("archive parent"))?;
        Ok(Installed { path, backup })
    }
    /// Removes both archive and loose overrides from discovery by moving them to backups.
    /// Returns backup paths for undo. If a later move fails, earlier moves are rolled back.
    /// `User` is protected because it contains preferences and personal data.
    pub fn remove(&self, name: &str) -> Result<Vec<PathBuf>> {
        validate_mutable_name(name)?;
        let _lock = self.lock()?;
        let sources = [
            self.data
                .join("Installed Packages")
                .join(format!("{name}.sublime-package")),
            self.data.join("Packages").join(name),
        ];
        let mut moves: Vec<(PathBuf, PathBuf)> = Vec::new();
        for source in sources {
            reject_symlink(&source)?;
            let metadata = match fs::symlink_metadata(&source) {
                Ok(m) => m,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            if metadata.file_type().is_symlink() {
                return Err(invalid("refusing to remove a symlink"));
            }
            // Preflight both sources before moving either one.
            moves.push((source, PathBuf::new()));
        }
        for (source, backup) in &mut moves {
            *backup = reserve_backup(source, &self.data.join(".velocity-backups"))?;
        }
        for i in 0..moves.len() {
            let (source, backup) = &moves[i];
            if let Err(error) = fs::rename(source, backup) {
                for (original, saved) in moves[..i].iter().rev() {
                    fs::rename(saved, original)?;
                }
                return Err(error.into());
            }
        }
        Ok(moves.into_iter().map(|(_, backup)| backup).collect())
    }
    /// Reads the user-level `ignored_packages` setting, accepting comments and trailing commas.
    pub fn ignored_packages(&self) -> Result<BTreeSet<String>> {
        let document = self.settings()?;
        let Some(value) = document.value.get("ignored_packages") else {
            return Ok(BTreeSet::new());
        };
        let values = value
            .as_array()
            .ok_or_else(|| invalid("ignored_packages must be an array"))?;
        values
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| invalid("ignored_packages entries must be strings"))
            })
            .collect()
    }
    /// Atomically edits just `ignored_packages`, preserving other preferences and comments.
    /// Returns a backup path if preferences already existed. Lane A must reload effective settings.
    pub fn set_enabled(&self, name: &str, enabled: bool) -> Result<Option<PathBuf>> {
        self.set_enabled_inheriting(name, enabled, &BTreeSet::new())
    }
    /// Like [`Self::set_enabled`], using inherited defaults if the user has not set
    /// `ignored_packages`. Pass the lower-priority settings layer to preserve disabled
    /// defaults such as Vintage when creating the first user override.
    pub fn set_enabled_inheriting(
        &self,
        name: &str,
        enabled: bool,
        inherited: &BTreeSet<String>,
    ) -> Result<Option<PathBuf>> {
        validate_mutable_name(name)?;
        let _lock = self.lock()?;
        let document = self.settings()?;
        let mut ignored = if document.value.get("ignored_packages").is_some() {
            self.ignored_packages()?
        } else {
            inherited.clone()
        };
        if enabled {
            ignored.remove(name);
        } else {
            ignored.insert(name.into());
        }
        let updated = document
            .set("ignored_packages", &serde_json::json!(ignored))
            .map_err(invalid)?;
        let path = self.settings_path();
        ensure_dir(path.parent().expect("settings parent"))?;
        let backup = backup_file(&path, &self.data.join(".velocity-backups"))?;
        let mut temp = NamedTempFile::new_in(path.parent().expect("settings parent"))?;
        temp.write_all(updated.as_bytes())?;
        temp.as_file().sync_all()?;
        temp.persist(&path).map_err(|e| Error::Io(e.error))?;
        sync_dir(path.parent().expect("settings parent"))?;
        Ok(backup)
    }
    fn settings_path(&self) -> PathBuf {
        self.data.join("Packages/User/Preferences.sublime-settings")
    }
    fn settings(&self) -> Result<Document> {
        let path = self.settings_path();
        reject_symlink(&path)?;
        let source = match fs::read_to_string(path) {
            Ok(source) => source,
            Err(e) if e.kind() == io::ErrorKind::NotFound => "{}\n".into(),
            Err(e) => return Err(e.into()),
        };
        let document = Document::parse(&source).map_err(invalid)?;
        if !document.value.is_object() {
            return Err(invalid("preferences must be a JSON object"));
        }
        Ok(document)
    }
    fn lock(&self) -> Result<StoreLock> {
        ensure_dir(&self.data)?;
        let path = self.data.join(".velocity-packages.lock");
        reject_symlink(&path)?;
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        file.try_lock().map_err(invalid)?;
        file.set_len(0)?;
        writeln!(file, "{}", std::process::id())?;
        Ok(StoreLock { _file: file })
    }
}

#[derive(Debug)]
struct StoreLock {
    // Closing releases the OS lock, including after a crash. Never unlink this path: doing
    // so could leave concurrent processes locking different inodes for the same store.
    _file: fs::File,
}
fn read_dir(path: &Path) -> Result<Vec<fs::DirEntry>> {
    reject_symlink(path)?;
    match fs::read_dir(path) {
        Ok(entries) => Ok(entries.collect::<io::Result<Vec<_>>>()?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}
pub(crate) fn validate_name(name: &str) -> Result<()> {
    if name.trim() != name
        || name.is_empty()
        || name.len() > 128
        || name.starts_with('.')
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
        || !matches!(
            Path::new(name).components().next(),
            Some(Component::Normal(_))
        )
    {
        return Err(invalid("invalid package name"));
    }
    Ok(())
}
pub(crate) fn validate_mutable_name(name: &str) -> Result<()> {
    validate_name(name)?;
    if name.eq_ignore_ascii_case("User") {
        return Err(invalid("the User package is protected"));
    }
    Ok(())
}
pub(crate) fn reject_symlink(path: &Path) -> Result<()> {
    // Check ancestors as well as the leaf: a symlinked Packages/User must not redirect writes.
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                // macOS /tmp and /var are system aliases; canonicalize the caller's data path
                // at construction sites when using them. Do not permit aliases inside a store.
                return Err(invalid(format!(
                    "symlink in managed path: {}",
                    ancestor.display()
                )));
            }
            Ok(_) => (),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
pub(crate) fn ensure_dir(path: &Path) -> Result<()> {
    reject_symlink(path)?;
    fs::create_dir_all(path)?;
    Ok(())
}
fn reserve_backup(source: &Path, dir: &Path) -> Result<PathBuf> {
    // Backups must live outside Packages and Installed Packages so existing resource
    // loaders cannot accidentally discover removed/old syntaxes, themes or settings.
    ensure_dir(dir)?;
    let reservation = tempfile::Builder::new()
        .prefix("package-")
        .tempdir_in(dir)?;
    let path = reservation.keep().join(
        source
            .file_name()
            .ok_or_else(|| invalid("backup needs a filename"))?,
    );
    Ok(path)
}
fn backup_file(path: &Path, backup_directory: &Path) -> Result<Option<PathBuf>> {
    reject_symlink(path)?;
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_file() => {
            let backup = reserve_backup(path, backup_directory)?;
            fs::copy(path, &backup)?;
            fs::File::open(&backup)?.sync_all()?;
            Ok(Some(backup))
        }
        Ok(_) => Err(invalid("destination is not a regular file")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn sync_dir(path: &Path) -> Result<()> {
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

fn resolve_root(path: PathBuf) -> PathBuf {
    let absolute = std::path::absolute(&path).unwrap_or(path);
    let mut ancestor = absolute.as_path();
    let mut missing = Vec::new();
    loop {
        if let Ok(mut resolved) = fs::canonicalize(ancestor) {
            for component in missing.into_iter().rev() {
                resolved.push(component);
            }
            return resolved;
        }
        if let (Some(name), Some(parent)) = (ancestor.file_name(), ancestor.parent()) {
            missing.push(name);
            ancestor = parent;
        } else {
            return absolute;
        }
    }
}
