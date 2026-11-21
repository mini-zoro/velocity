//! Package resources with zip/loose overrides and deterministic User-last order.
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};
const MAX_LOOSE_DEPTH: usize = 32;
const MAX_LOOSE_ENTRIES: usize = 50_000;
/// Effective resource content and provenance.
#[derive(Clone, Debug)]
pub struct Resource {
    /// `Packages/<package>/<relative path>`.
    pub path: String,
    /// UTF-8 data (validated by `text`).
    pub bytes: Cow<'static, [u8]>,
    /// Archive or loose file on disk.
    pub origin: PathBuf,
    tier: Tier,
}
impl Resource {
    /// Whether this comes from shipped resources, rather than a data-directory package.
    pub fn shipped(&self) -> bool {
        self.tier == Tier::Shipped
    }
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
    plugins: BTreeMap<String, Tier>,
    archive_warnings: BTreeMap<String, String>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum Tier {
    Shipped,
    Installed,
    Loose,
}
impl Resources {
    /// Adds an embedded resource at lowest or current priority.
    pub fn insert(&mut self, path: &str, bytes: Vec<u8>, origin: PathBuf) {
        self.insert_tier(path, bytes, origin, Tier::Shipped);
    }
    /// Registers shipped data without opening a file or copying its payload.
    pub fn insert_embedded(&mut self, path: &str, bytes: &'static [u8], origin: PathBuf) {
        self.entries.insert(
            path.into(),
            Resource {
                path: path.into(),
                bytes: Cow::Borrowed(bytes),
                origin,
                tier: Tier::Shipped,
            },
        );
    }
    /// Records plugin provenance without ever reading or executing the file.
    pub fn insert_shipped_plugin(&mut self, path: &str) {
        self.plugins.insert(path.into(), Tier::Shipped);
    }
    fn insert_tier(&mut self, path: &str, bytes: Vec<u8>, origin: PathBuf, tier: Tier) {
        self.entries.insert(
            path.into(),
            Resource {
                path: path.into(),
                bytes: bytes.into(),
                origin,
                tier,
            },
        );
    }
    /// Loads installed archives then loose files. Loose files override archives.
    pub fn load(&mut self, data: &Path) -> Result<(), String> {
        self.load_inner(data, &mut None, |_| {})
    }
    /// Loads independent resources despite broken packages/files and returns their warnings.
    /// Used by background application loading so one bad package cannot hide other packages.
    pub fn load_with_warnings(&mut self, data: &Path) -> Vec<String> {
        self.load_with_warnings_observing(data, |_| {})
    }
    /// Like [Self::load_with_warnings], with a callback before reading each
    /// archive or loose resource and before scanning each eligible directory.
    /// The resource roots are observed even when absent.
    pub fn load_with_warnings_observing(
        &mut self,
        data: &Path,
        observe: impl FnMut(&Path),
    ) -> Vec<String> {
        let mut warnings = Vec::new();
        if let Err(error) = self.load_inner(data, &mut Some(&mut warnings), observe) {
            warnings.push(format!("{}: {error}", data.display()));
        }
        crate::settings_source::retain_valid(data, &mut self.entries, &mut warnings);
        warnings
    }
    fn load_inner(
        &mut self,
        data: &Path,
        warnings: &mut Option<&mut Vec<String>>,
        mut observe: impl FnMut(&Path),
    ) -> Result<(), String> {
        let installed = data.join("Installed Packages");
        let packages = data.join("Packages");
        observe(&installed);
        observe(&packages);
        // Also watch missing or dangling User directories. The target of an
        // external User symlink may appear without a change to Packages.
        observe(&packages.join("User"));
        if installed.is_dir() {
            let mut paths = fs::read_dir(&installed)
                .map_err(|e| e.to_string())?
                .map(|e| e.map(|e| e.path()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            paths.sort();
            for path in paths
                .into_iter()
                .filter(|p| !hidden(p) && p.extension().is_some_and(|e| e == "sublime-package"))
            {
                observe(&path);
                let result = self.archive(&path);
                if let Some(package) = path.file_stem().and_then(std::ffi::OsStr::to_str) {
                    if let Err(error) = &result {
                        self.archive_warnings
                            .insert(package.into(), format!("{}: {error}", path.display()));
                    } else {
                        self.archive_warnings.remove(package);
                    }
                }
                resource_result(result, &path, warnings)?;
            }
        }
        if packages.is_dir() {
            self.loose(&packages, Tier::Loose, warnings, observe)?;
        }
        Ok(())
    }
    fn archive(&mut self, path: &Path) -> Result<(), String> {
        let package = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("non-UTF8 package")?;
        let mut archive = zip::ZipArchive::new(open_regular(path)?).map_err(|e| e.to_string())?;
        let mut staged = Resources::default();
        for i in 0..archive.len() {
            let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
            let relative = file.enclosed_name().ok_or("unsafe archive resource path")?;
            if relative.ancestors().any(hidden) {
                continue;
            }
            if file.is_symlink() {
                return Err("archive contains a symlink".into());
            }
            if file.is_dir() {
                continue;
            }
            let key = format!("Packages/{package}/{}", relative.to_string_lossy());
            if key.ends_with(".py") {
                staged.plugins.insert(key, Tier::Installed);
                continue;
            }
            if file.size() > 32 * 1024 * 1024 {
                return Err("resource exceeds 32 MiB limit".into());
            }
            let mut bytes = Vec::new();
            read_limited(&mut file, &mut bytes)?;
            staged.insert_tier(&key, bytes, path.to_owned(), Tier::Installed);
        }
        // A valid archive replaces the *whole* shipped package of that name.
        // https://docs.sublimetext.io/guide/extensibility/packages.html
        let prefix = format!("Packages/{package}/");
        self.entries
            .retain(|p, v| !p.starts_with(&prefix) || v.tier != Tier::Shipped);
        self.plugins.retain(|p, _| !p.starts_with(&prefix));
        self.entries.extend(staged.entries);
        self.plugins.extend(staged.plugins);
        Ok(())
    }

    fn loose(
        &mut self,
        root: &Path,
        tier: Tier,
        warnings: &mut Option<&mut Vec<String>>,
        mut observe: impl FnMut(&Path),
    ) -> Result<(), String> {
        walk_loose(root, loose_limit(), warnings, |path, directory| {
            observe(path);
            if directory {
                return Ok(());
            }
            let relative = path.strip_prefix(root).map_err(|e| e.to_string())?;
            let key = format!("Packages/{}", relative.to_string_lossy());
            if key.ends_with(".py") {
                self.plugins.insert(key, tier);
            } else if tier != Tier::Shipped || shipped_resource(path) {
                let mut bytes = Vec::new();
                read_limited(&mut open_regular(path)?, &mut bytes)?;
                self.insert_tier(&key, bytes, path.to_owned(), tier);
            }
            Ok(())
        })
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
    /// Why an archive failed, for commands that fall back to a lower-priority resource.
    pub fn archive_warning(&self, package: &str) -> Option<&str> {
        self.archive_warnings.get(package).map(String::as_str)
    }
    /// Adds the supported runtime resources in the bundled language-package tree.
    pub fn load_shipped(&mut self, root: &Path) -> Result<(), String> {
        self.loose(root, Tier::Shipped, &mut None, |_| {})
    }
    /// Package names, including packages with Python files but no supported resources.
    pub fn package_names(&self) -> BTreeSet<String> {
        self.entries
            .keys()
            .chain(self.plugins.keys())
            .filter_map(|p| p.split('/').nth(1))
            .map(str::to_owned)
            .collect()
    }
    /// Enabled packages with Python files: count and whether all sources are shipped.
    pub fn plugins(&self) -> BTreeMap<&str, (usize, bool)> {
        let mut counts = BTreeMap::new();
        for (path, tier) in &self.plugins {
            if let Some(package) = path.split('/').nth(1)
                && self.package_enabled(package)
            {
                let (count, shipped) = counts.entry(package).or_insert((0, true));
                *count += 1;
                *shipped &= *tier == Tier::Shipped;
            }
        }
        counts
    }
    /// Finds one effective package resource.
    pub fn get(&self, path: &str) -> Option<&Resource> {
        let path = path.strip_prefix("res://").unwrap_or(path);
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
                    4
                } else {
                    1 + r.tier as u8
                },
                package,
                r.path.contains("(OSX)"),
                &r.path,
            )
        });
        out
    }
}

fn ancestors(root: &Path) -> BTreeSet<PathBuf> {
    root.canonicalize()
        .ok()
        .into_iter()
        .flat_map(|p| {
            p.ancestors()
                .skip(1)
                .map(Path::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn hidden(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.as_encoded_bytes().starts_with(b"."))
}

fn ignored_link_target(path: &Path, profile: Option<&Path>) -> bool {
    profile
        .and_then(|profile| path.strip_prefix(profile).ok())
        .and_then(|relative| relative.iter().next())
        .is_some_and(|name| matches!(name.to_str(), Some("Cache" | "Index" | "Backup")))
        || matches!(
            path.file_name().and_then(std::ffi::OsStr::to_str),
            Some(
                "Cache"
                    | "Index"
                    | "Backup"
                    | ".git"
                    | ".hg"
                    | ".svn"
                    | ".bzr"
                    | "CVS"
                    | "node_modules"
                    | "__pycache__"
                    | "target"
            )
        )
}

fn loose_resource(path: &Path) -> bool {
    shipped_resource(path)
        || path
            .file_name()
            .is_some_and(|name| name == "package-metadata.json")
        || matches!(
            path.extension().and_then(std::ffi::OsStr::to_str),
            // Theme resources also feed minihtml, whose image decoder accepts
            // JPEG and GIF. Theme textures themselves are PNG, not SVG:
            // https://www.sublimetext.com/docs/themes.html#texture-images
            Some("py" | "png" | "jpg" | "jpeg" | "gif" | "hidden-tmTheme" | "hidden-color-scheme")
        )
}

fn loose_limit() -> usize {
    #[cfg(test)]
    if let Some(limit) = TEST_LIMIT.get() {
        return limit;
    }
    MAX_LOOSE_ENTRIES
}

#[cfg(test)]
thread_local! {
    static TEST_LIMIT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

fn walk_loose(
    root: &Path,
    limit: usize,
    warnings: &mut Option<&mut Vec<String>>,
    mut visit: impl FnMut(&Path, bool) -> Result<(), String>,
) -> Result<(), String> {
    let top = fs::read_dir(root).map_err(|e| e.to_string())?;
    let mut visited = ancestors(root);
    let profile = root.parent().and_then(|parent| parent.canonicalize().ok());
    if let Ok(canonical) = root.canonicalize() {
        visited.insert(canonical);
    }
    let mut packages = BTreeMap::new();
    for entry in top.take(MAX_LOOSE_ENTRIES).flatten() {
        let name = entry.file_name();
        if !name.as_encoded_bytes().starts_with(b".")
            && let Ok(kind) = entry.file_type()
            && (kind.is_dir() || kind.is_symlink())
        {
            // Prefer User even when it links to the same physical directory
            // as another root. Otherwise reserve physical roots before aliases.
            let priority = if name == "User" {
                0
            } else if kind.is_dir() {
                1
            } else {
                2
            };
            packages.insert((priority, name), (entry.path(), kind.is_symlink()));
        }
    }
    for name in ["User", "Default"] {
        let path = root.join(name);
        if path.is_dir() && !packages.values().any(|(other, _)| *other == path) {
            let linked = path.is_symlink();
            packages.insert((0, name.into()), (path, linked));
        }
    }
    let mut reserved = Vec::new();
    for (package, linked) in packages.into_values() {
        match package.canonicalize() {
            Ok(path)
                if (!linked || !ignored_link_target(&path, profile.as_deref()))
                    && visited.insert(path.clone())
                    && path.is_dir() =>
            {
                reserved.push(package);
            }
            Ok(_) => (),
            Err(e) => resource_result(Err(e.to_string()), &package, warnings)?,
        }
    }

    for package in reserved {
        let mut directories = VecDeque::from([(package.clone(), 0, false)]);
        let mut links = VecDeque::new();
        let mut remaining = limit;
        let mut capped = false;
        while let Some((dir, depth, linked)) = directories.pop_front().or_else(|| links.pop_front())
        {
            if depth > MAX_LOOSE_DEPTH || remaining == 0 {
                capped = true;
                if remaining == 0 {
                    break;
                }
                continue;
            }
            if depth != 0 {
                match dir.canonicalize() {
                    Ok(path)
                        if (linked && ignored_link_target(&path, profile.as_deref()))
                            || !visited.insert(path.clone()) =>
                    {
                        continue;
                    }
                    Ok(_) => (),
                    Err(e) => {
                        resource_result(Err(e.to_string()), &dir, warnings)?;
                        continue;
                    }
                }
            }
            resource_result(visit(&dir, true), &dir, warnings)?;
            let entries = match fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(e) => {
                    resource_result(Err(e.to_string()), &dir, warnings)?;
                    continue;
                }
            };
            let mut relevant = Vec::new();
            for entry in entries {
                if remaining == 0 {
                    capped = true;
                    break;
                }
                remaining -= 1;
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(e) => {
                        resource_result(Err(e.to_string()), &dir, warnings)?;
                        continue;
                    }
                };
                let name = entry.file_name();
                if name.as_encoded_bytes().starts_with(b".") {
                    continue;
                }
                let path = dir.join(&name);
                let kind = match entry.file_type() {
                    Ok(kind) => kind,
                    Err(e) => {
                        resource_result(Err(e.to_string()), &path, warnings)?;
                        continue;
                    }
                };
                if kind.is_dir() || kind.is_symlink() || (kind.is_file() && loose_resource(&path)) {
                    relevant.push((path, kind));
                }
            }
            relevant.sort_by(|a, b| a.0.cmp(&b.0));
            for (path, kind) in relevant {
                let linked = kind.is_symlink();
                let kind = if linked {
                    match fs::metadata(&path) {
                        Ok(m) => m.file_type(),
                        Err(e) => {
                            resource_result(Err(e.to_string()), &path, warnings)?;
                            continue;
                        }
                    }
                } else {
                    kind
                };
                if kind.is_dir() {
                    (if linked { &mut links } else { &mut directories }).push_back((
                        path,
                        depth + 1,
                        linked,
                    ));
                } else if kind.is_file() && loose_resource(&path) {
                    if linked
                        && path
                            .canonicalize()
                            .is_ok_and(|target| ignored_link_target(&target, profile.as_deref()))
                    {
                        continue;
                    }
                    resource_result(visit(&path, false), &path, warnings)?;
                }
            }
        }
        if capped {
            let name = package.file_name().unwrap_or_default().to_string_lossy();
            resource_result(
                Err(format!(
                    "package {name}: loose package traversal limit reached"
                )),
                &package,
                warnings,
            )?;
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
#[test]
fn loose_directory_dag_keeps_canonical_packages_and_ignores_hidden_files() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let packages = root.path().join("Packages");
    fs::create_dir(&packages).unwrap();
    for i in (0..15).rev() {
        let path = packages.join(format!("Pkg{i:02}"));
        fs::create_dir(&path).unwrap();
        fs::write(path.join("Fixture.sublime-settings"), "{}").unwrap();
        if i < 14 {
            for alias in ["alias-a", "alias-b"] {
                symlink(packages.join(format!("Pkg{:02}", i + 1)), path.join(alias)).unwrap();
            }
        }
    }
    let external = tempfile::tempdir().unwrap();
    symlink(external.path(), packages.join("User")).unwrap();
    fs::write(external.path().join("Preferences.sublime-settings"), "{}").unwrap();
    fs::write(
        external.path().join("._Preferences.sublime-settings"),
        b"not JSON",
    )
    .unwrap();
    fs::write(
        external.path().join(".MyHidden.sublime-settings"),
        b"not JSON",
    )
    .unwrap();
    symlink(&packages, external.path().join("cycle")).unwrap();
    let mut cursor = external.path().to_owned();
    for _ in 0..=MAX_LOOSE_DEPTH {
        cursor.push("deep");
        fs::create_dir(&cursor).unwrap();
    }
    fs::write(cursor.join("Too Deep.sublime-settings"), "{}").unwrap();
    let mut resources = Resources::default();
    let warnings = resources.load_with_warnings(root.path());
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("traversal limit"));
    assert_eq!(resources.matching(".sublime-settings").len(), 16);
    assert!(
        resources
            .get("Packages/User/Preferences.sublime-settings")
            .is_some()
    );
    assert!(
        resources
            .get("Packages/Pkg14/Fixture.sublime-settings")
            .is_some()
    );
}

#[cfg(all(test, unix))]
#[test]
fn loose_limit_keeps_linked_user_and_default_after_an_oversized_package() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let packages = root.path().join("Packages");
    let big = packages.join("Bulk");
    let default = packages.join("Default");
    let user = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    fs::create_dir_all(&big).unwrap();
    fs::create_dir_all(&default).unwrap();
    symlink(user.path(), packages.join("User")).unwrap();
    for i in 0..12 {
        fs::write(big.join(format!("{i}.txt")), "not a resource").unwrap();
    }
    fs::write(default.join("Defaults.sublime-settings"), "{}").unwrap();
    fs::write(user.path().join("Preferences.sublime-settings"), "{}").unwrap();
    fs::create_dir(external.path().join("node_modules")).unwrap();
    symlink(
        external.path().join("node_modules"),
        user.path().join("node_modules"),
    )
    .unwrap();
    fs::write(
        user.path().join("node_modules/Unrelated.sublime-settings"),
        "not JSON",
    )
    .unwrap();
    fs::write(user.path().join("ignore.log"), "not a resource").unwrap();
    TEST_LIMIT.set(Some(4));
    let mut resources = Resources::default();
    let mut watched = Vec::new();
    let warnings = resources.load_with_warnings_observing(root.path(), |path| {
        watched.push(path.to_owned());
    });
    TEST_LIMIT.set(None);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("package Bulk"), "{warnings:?}");
    assert!(
        resources
            .get("Packages/Default/Defaults.sublime-settings")
            .is_some()
    );
    assert!(
        resources
            .get("Packages/User/Preferences.sublime-settings")
            .is_some()
    );
    assert!(resources.get("Packages/User/ignore.log").is_none());
    assert!(watched.contains(&packages.join("User/Preferences.sublime-settings")));
    assert!(
        watched
            .iter()
            .all(|p| !p.ends_with("node_modules") && !p.ends_with("ignore.log"))
    );
}

#[cfg(all(test, unix))]
#[test]
fn loose_images_and_ordinary_package_directories_are_loaded_and_watched() {
    let root = tempfile::tempdir().unwrap();
    let packages = root.path().join("Packages");
    let package = packages.join("Backup");
    let dirs = "Cache/Index/Backup/target/node_modules/CVS";
    fs::create_dir_all(package.join(dirs)).unwrap();
    for ext in ["png", "jpg", "jpeg", "gif"] {
        fs::write(package.join(dirs).join(format!("sample.{ext}")), ext).unwrap();
    }
    let private = root.path().join("Cache/private");
    fs::create_dir_all(&private).unwrap();
    fs::write(private.join("must-not-load.gif"), "profile cache").unwrap();
    std::os::unix::fs::symlink(&private, package.join("alias")).unwrap();
    let mut resources = Resources::default();
    let mut watched = Vec::new();
    let warnings = resources.load_with_warnings_observing(root.path(), |path| {
        watched.push(path.to_owned());
    });
    assert!(warnings.is_empty(), "{warnings:?}");
    for ext in ["png", "jpg", "jpeg", "gif"] {
        let key = format!("Packages/Backup/{dirs}/sample.{ext}");
        assert_eq!(resources.get(&key).unwrap().text().unwrap(), ext);
        assert!(watched.contains(&root.path().join(&key)));
    }
    assert!(
        resources
            .get("Packages/Backup/alias/must-not-load.gif")
            .is_none()
    );
    assert!(!watched.contains(&package.join("alias")));
}

fn shipped_resource(path: &Path) -> bool {
    path.file_name()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|name| name.ends_with(".tmLanguage.json"))
        || path
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|ext| {
                ext.starts_with("sublime-")
                    || matches!(ext, "tmLanguage" | "tmPreferences" | "tmTheme")
            })
        || path.to_string_lossy().ends_with(".tmLanguage.json")
}

fn open_regular(path: &Path) -> Result<fs::File, String> {
    checked_open(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn checked_open(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Check the descriptor too: a raced-in FIFO must not block open.
        options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "resource is not a regular file",
        ));
    }
    Ok(file)
}

fn resource_result(
    result: Result<(), String>,
    path: &Path,
    warnings: &mut Option<&mut Vec<String>>,
) -> Result<(), String> {
    if let Err(error) = result {
        if let Some(warnings) = warnings {
            warnings.push(format!("{}: {error}", path.display()));
        } else {
            return Err(error);
        }
    }
    Ok(())
}

/// Reads only the requested resource: its loose override, then its supplying archive member.
/// No directory traversal or unrelated resource contents are loaded. Callers apply exclusions
/// and fall back to lower-priority roots or embedded resources after reporting an error.
pub fn read_one(data: &Path, path: &str) -> Result<Option<Resource>, String> {
    let (package, relative) = path
        .strip_prefix("Packages/")
        .and_then(|path| path.split_once('/'))
        .filter(|_| {
            !path.contains('\\')
                && path
                    .split('/')
                    .all(|p| !p.is_empty() && !p.starts_with('.'))
        })
        .ok_or("invalid package resource path")?;
    let loose = data.join(path);
    let mut bytes = Vec::new();
    let origin = match checked_open(&loose) {
        Ok(mut file) => {
            read_limited(&mut file, &mut bytes).map_err(|e| format!("{}: {e}", loose.display()))?;
            loose
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let archive_path = data
                .join("Installed Packages")
                .join(format!("{package}.sublime-package"));
            let file = match checked_open(&archive_path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(format!("{}: {error}", archive_path.display())),
            };
            let mut archive = zip::ZipArchive::new(file)
                .map_err(|e| format!("{}: {e}", archive_path.display()))?;
            let mut file = match archive.by_name(relative) {
                Ok(file) => file,
                Err(zip::result::ZipError::FileNotFound) => return Ok(None),
                Err(error) => return Err(format!("{}: {error}", archive_path.display())),
            };
            if file.size() > 32 * 1024 * 1024 {
                return Err("resource exceeds 32 MiB limit".into());
            }
            read_limited(&mut file, &mut bytes)?;
            archive_path
        }
        Err(error) => return Err(format!("{}: {error}", loose.display())),
    };
    Ok(Some(Resource {
        path: path.into(),
        bytes: bytes.into(),
        origin,
        tier: Tier::Loose,
    }))
}
fn read_limited(reader: &mut impl Read, bytes: &mut Vec<u8>) -> Result<(), String> {
    const LIMIT: u64 = 32 * 1024 * 1024;
    reader
        .take(LIMIT + 1)
        .read_to_end(bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > LIMIT {
        return Err("resource exceeds 32 MiB limit".into());
    }
    Ok(())
}
