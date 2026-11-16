//! Confirmation-gated installation of the bundled command-line launcher.
use crate::{Result, ensure_dir, invalid};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Two menu choices; neither is written until the user confirms a concrete plan.
pub fn install_directories(home: &Path) -> [PathBuf; 2] {
    [PathBuf::from("/usr/local/bin"), home.join(".local/bin")]
}

/// Concrete source and destination to show in the app's confirmation prompt.
#[derive(Clone, Debug)]
pub struct CliInstallPlan {
    launcher: PathBuf,
    link: PathBuf,
}
impl CliInstallPlan {
    /// Locates `Contents/SharedSupport/bin/velocity` in an installed app bundle.
    /// The bin directory is normally one of [`install_directories`]; custom paths aid testing.
    pub fn for_bundle(bundle: &Path, bin_directory: &Path) -> Result<Self> {
        let bundle = fs::canonicalize(bundle)?;
        let launcher = fs::canonicalize(bundle.join("Contents/SharedSupport/bin/velocity"))?;
        if !launcher.starts_with(&bundle) || !launcher.is_file() {
            return Err(invalid("launcher must be a file inside the app bundle"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if fs::metadata(&launcher)?.permissions().mode() & 0o111 == 0 {
                return Err(invalid("launcher is not executable"));
            }
        }
        Ok(Self {
            launcher,
            link: crate::resolve_root(bin_directory.to_owned()).join("velocity"),
        })
    }
    /// Resolved launcher inside the app bundle.
    pub fn launcher(&self) -> &Path {
        &self.launcher
    }
    /// Symlink that will be created if confirmed.
    pub fn link(&self) -> &Path {
        &self.link
    }
}
/// Outcome of the `install_cli` menu command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliInstallOutcome {
    /// The user declined; nothing was written, including parent directories.
    Declined,
    /// The correct link already existed; nothing changed.
    AlreadyInstalled,
    /// A new link was installed. No privilege escalation is attempted.
    Installed(PathBuf),
}

/// Executes a plan after the app's confirmation callback returns true.
/// Existing unrelated files/symlinks are never overwritten; system-bin permission errors
/// should cause the menu to offer `~/.local/bin`. Does not invoke sudo or a shell.
#[cfg(unix)]
pub fn install_cli(
    plan: &CliInstallPlan,
    confirm: impl FnOnce(&CliInstallPlan) -> bool,
) -> Result<CliInstallOutcome> {
    if let Ok(target) = fs::read_link(&plan.link) {
        let absolute = if target.is_absolute() {
            target
        } else {
            plan.link.parent().expect("link parent").join(target)
        };
        if fs::canonicalize(absolute).ok().as_ref() == Some(&plan.launcher) {
            return Ok(CliInstallOutcome::AlreadyInstalled);
        }
    }
    match fs::symlink_metadata(&plan.link) {
        Ok(_) => return Err(invalid("velocity already exists at the destination")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    if !confirm(plan) {
        return Ok(CliInstallOutcome::Declined);
    }
    ensure_dir(plan.link.parent().expect("link parent"))?;
    std::os::unix::fs::symlink(&plan.launcher, &plan.link)?;
    Ok(CliInstallOutcome::Installed(plan.link.clone()))
}
