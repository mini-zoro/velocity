#![cfg(unix)]
#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};
use vel_packages::cli::{CliInstallOutcome, CliInstallPlan, install_cli, install_directories};

#[test]
fn cli_install_requires_confirmation_is_idempotent_and_preserves_unrelated_files() {
    let root = tempfile::tempdir().unwrap();
    let bundle = root.path().join("App With Spaces.app");
    let launcher = bundle.join("Contents/SharedSupport/bin/velocity");
    fs::create_dir_all(launcher.parent().unwrap()).unwrap();
    fs::write(&launcher, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o755)).unwrap();
    let bin = root.path().join(".local/bin");
    let plan = CliInstallPlan::for_bundle(&bundle, &bin).unwrap();
    let resolved_launcher = fs::canonicalize(&launcher).unwrap();
    assert_eq!(
        install_cli(&plan, |p| {
            assert_eq!(p.launcher(), resolved_launcher);
            false
        })
        .unwrap(),
        CliInstallOutcome::Declined
    );
    assert!(!bin.exists());
    assert!(matches!(
        install_cli(&plan, |_| true).unwrap(),
        CliInstallOutcome::Installed(_)
    ));
    assert_eq!(fs::read_link(plan.link()).unwrap(), resolved_launcher);
    assert_eq!(
        install_cli(&plan, |_| panic!("must be idempotent")).unwrap(),
        CliInstallOutcome::AlreadyInstalled
    );
    fs::remove_file(plan.link()).unwrap();
    fs::write(plan.link(), "unrelated tool").unwrap();
    assert!(install_cli(&plan, |_| true).is_err());
    assert_eq!(fs::read_to_string(plan.link()).unwrap(), "unrelated tool");
    fs::remove_file(plan.link()).unwrap();
    symlink("/missing/old-app", plan.link()).unwrap();
    assert!(install_cli(&plan, |_| true).is_err());
    let choices = install_directories(root.path());
    assert_eq!(choices[0].to_str().unwrap(), "/usr/local/bin");
    assert_eq!(choices[1], bin);
}

#[test]
fn escaped_or_nonexecutable_launcher_cannot_be_installed() {
    let root = tempfile::tempdir().unwrap();
    let bundle = root.path().join("Velocity.app");
    let launcher = bundle.join("Contents/SharedSupport/bin/velocity");
    fs::create_dir_all(launcher.parent().unwrap()).unwrap();
    let outside = root.path().join("outside");
    fs::write(&outside, "outside").unwrap();
    symlink(&outside, &launcher).unwrap();
    assert!(CliInstallPlan::for_bundle(&bundle, root.path()).is_err());
    fs::remove_file(&launcher).unwrap();
    fs::write(&launcher, "not executable").unwrap();
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(CliInstallPlan::for_bundle(&bundle, root.path()).is_err());
}
