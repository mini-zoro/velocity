//! Native path spelling and profile locations.
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

/// Compare normalized paths, without following symlinks.
#[inline]
pub fn equal(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    {
        let a = display_path(a);
        let b = display_path(b);
        a.components()
            .zip(b.components())
            .all(|(a, b)| components_equal(a, b))
            && a.components().count() == b.components().count()
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}

/// Compare components with Win32's ordinal case folding, not the current locale.
#[cfg(windows)]
pub fn components_equal(a: std::path::Component<'_>, b: std::path::Component<'_>) -> bool {
    if a == b {
        return true;
    }
    std::mem::discriminant(&a) == std::mem::discriminant(&b)
        && vel_platform::windows_path_component_eq(a.as_os_str(), b.as_os_str())
}

/// Remove the Win32 verbatim prefix for display and lexical comparisons only.
#[cfg(windows)]
pub fn display_path(path: &Path) -> std::borrow::Cow<'_, Path> {
    use std::path::{Component, Prefix};
    let mut parts = path.components();
    if let Some(Component::Prefix(prefix)) = parts.next() {
        let mut display = match prefix.kind() {
            Prefix::VerbatimDisk(drive) => PathBuf::from(format!("{}:", char::from(drive))),
            Prefix::VerbatimUNC(server, share) => {
                let mut unc = std::ffi::OsString::from(r"\\");
                unc.push(server);
                unc.push(r"\");
                unc.push(share);
                PathBuf::from(unc)
            }
            _ => return path.into(),
        };
        if parts.next() == Some(Component::RootDir) {
            let mut rooted = display.into_os_string();
            rooted.push(r"\");
            display = rooted.into();
        }
        display.extend(parts);
        return std::borrow::Cow::Owned(display);
    }
    path.into()
}

/// The regular user-facing spelling of a path, without changing the I/O path.
pub fn display(path: &Path) -> String {
    #[cfg(windows)]
    let display = display_path(path);
    #[cfg(windows)]
    let path = display.as_ref();
    path.display().to_string()
}

/// Resolve the conventional Windows profile roots from caller-supplied environment values.
#[cfg(windows)]
pub fn windows_data_dir(
    override_dir: Option<&std::ffi::OsStr>,
    roaming: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    override_dir
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| profile_child(roaming, ""))
}

/// Logs below the Windows local (non-roaming) profile.
#[cfg(windows)]
pub fn windows_log_dir(local: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    profile_child(local, "Logs")
}

/// Cache below the Windows local (non-roaming) profile.
#[cfg(windows)]
pub fn windows_cache_dir(local: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    profile_child(local, "Cache")
}

#[cfg(windows)]
fn profile_child(base: Option<&std::ffi::OsStr>, child: &str) -> Option<PathBuf> {
    let root = Path::new(base.filter(|p| !p.is_empty())?).join("Velocity");
    Some(if child.is_empty() {
        root
    } else {
        root.join(child)
    })
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn profile_and_win32_spellings() {
        assert_eq!(
            windows_data_dir(None, Some(OsStr::new(r"C:\Profile\Roaming"))).unwrap(),
            Path::new(r"C:\Profile\Roaming\Velocity"),
        );
        assert_eq!(
            windows_data_dir(Some(OsStr::new(r"D:\Custom")), None).unwrap(),
            Path::new(r"D:\Custom"),
        );
        let local = Some(OsStr::new(r"C:\Profile\Local"));
        assert_eq!(
            windows_log_dir(local).unwrap(),
            Path::new(r"C:\Profile\Local\Velocity\Logs")
        );
        assert_eq!(
            windows_cache_dir(local).unwrap(),
            Path::new(r"C:\Profile\Local\Velocity\Cache")
        );
        assert!(equal(Path::new(r"\\?\C:\Work\A"), Path::new("c:/work/a")));
        assert!(!equal(Path::new(r"C:\Work\A"), Path::new(r"D:\Work\A")));
        assert!(equal(
            Path::new("C:/Work/\u{00c4}.txt"),
            Path::new("c:/work/\u{00e4}.TXT"),
        ));
        assert!(equal(
            Path::new(r"\\?\UNC\Server\Share\A"),
            Path::new(r"\\server\share\a")
        ));
        assert_eq!(
            display(Path::new(r"\\?\UNC\Server\Share\A")),
            r"\\Server\Share\A"
        );
    }
}
