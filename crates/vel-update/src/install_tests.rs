use super::*;

pub(crate) fn portable_files(version: &str) -> [(&'static str, Vec<u8>); 4] {
    [
        ("velocity.exe", b"fake gui".to_vec()),
        ("vel.exe", b"fake cli".to_vec()),
        ("README-Windows.txt", b"example readme".to_vec()),
        (
            "Resources/build-info.json",
            serde_json::json!({"version":version,"target":"x86_64-pc-windows-msvc"})
                .to_string()
                .into_bytes(),
        ),
    ]
}

fn portable(path: &Path, version: &str) {
    for (name, bytes) in portable_files(version) {
        let file = path.join(name);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, bytes).unwrap();
    }
}

#[test]
fn windows_swap_rolls_back_and_never_overwrites_old() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("Velocity with spaces");
    let staged = tmp.path().join("stage/Velocity");
    let old = old_path(&target).unwrap();
    assert_eq!(old, tmp.path().join("Velocity with spaces.old"));
    portable(&target, "0.1.3");
    portable(&staged, "0.2.0");
    let (before, after) = (Version::new(0, 1, 3), Version::new(0, 2, 0));
    fs::create_dir(&old).unwrap();
    assert!(swap(&target, &staged, &old, Platform::Windows).is_err());
    validate_portable(&target, &before).unwrap();
    fs::remove_dir(&old).unwrap();
    assert!(swap(&target, &tmp.path().join("absent"), &old, Platform::Windows).is_err());
    validate_portable(&target, &before).unwrap();
    assert!(!old.exists());
    let assert_swapped = || {
        validate_portable(&target, &after).unwrap();
        validate_portable(&old, &before).unwrap();
    };
    assert!(
        trial_replace(&target, &staged, &old, Platform::Windows, || {
            assert_swapped();
            false
        })
        .is_err()
    );
    validate_portable(&target, &before).unwrap();
    validate_portable(&staged, &after).unwrap();
    assert!(!old.exists());
    trial_replace(&target, &staged, &old, Platform::Windows, || {
        assert_swapped();
        true
    })
    .unwrap();
    validate_portable(&target, &after).unwrap();
    assert!(!old.exists());
}

#[test]
fn windows_installer_and_nonportable_paths_fall_back() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("Velocity");
    portable(&target, "0.1.3");
    let inspect = || Install::detect(&target.join("velocity.exe"), Platform::Windows, "0.1.3");
    inspect().unwrap();
    for extra in ["unins000.exe", "unins001.dat", "a project.txt"] {
        fs::write(target.join(extra), b"keep this").unwrap();
        assert_eq!(inspect().unwrap_err().reason, Reason::ManualInstall);
        assert!(target.join(extra).exists());
        fs::remove_file(target.join(extra)).unwrap();
    }
    for path in [
        "/Velocity/file",
        "Velocity/../file",
        "Velocity/dir\\other",
        "Velocity/file:stream",
        "Velocity/COM1.txt",
        "Velocity/a./b",
    ] {
        assert!(zip_path(path, Platform::Windows).is_err());
    }
    zip_path(
        "Velocity/Resources/Packages/Default (Windows).sublime-keymap",
        Platform::Windows,
    )
    .unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn macos_ditto_codesign_and_bundle_exchange() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let fixture = |path: &Path, version: &str, id: &str| {
        fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
        fs::create_dir_all(path.join("Contents/Resources")).unwrap();
        fs::copy("/usr/bin/true", path.join("Contents/MacOS/velocity")).unwrap();
        fs::set_permissions(
            path.join("Contents/MacOS/velocity"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let contents = serde_json::json!({"version": version});
        fs::write(
            path.join("Contents/Resources/build-info.json"),
            contents.to_string(),
        )
        .unwrap();
        let mut plist = plist::Dictionary::new();
        for (key, value) in [
            ("CFBundleIdentifier", id),
            ("CFBundleExecutable", "velocity"),
            ("CFBundleShortVersionString", version),
            ("CFBundlePackageType", "APPL"),
        ] {
            plist.insert(key.into(), value.into());
        }
        plist::to_file_xml(path.join("Contents/Info.plist"), &plist).unwrap();
        mac_tool("/usr/bin/codesign", |cmd| {
            cmd.args(["--force", "--sign", "-"]).arg(path)
        })
        .unwrap();
    };
    let target = temp.path().join("Renamed Velocity.app");
    fixture(&target, "0.1.3", "dev.velocity.Velocity");
    assert!(Install::detect(&executable(&target, Platform::Mac), Platform::Mac, "0.1.3").is_ok());
    let bundle = temp.path().join("export/Velocity.app");
    fixture(&bundle, "0.2.0", "dev.velocity.Velocity");
    let archive = temp.path().join("release.zip");
    mac_tool("/usr/bin/ditto", |cmd| {
        cmd.args(["-c", "-k", "--keepParent"])
            .arg(&bundle)
            .arg(&archive)
    })
    .unwrap();
    let hash = verify::digest(File::open(&archive).unwrap()).unwrap();
    let (before, after) = (Version::new(0, 1, 3), Version::new(0, 2, 0));
    let candidate = extract(
        &archive,
        &temp.path().join("unpack"),
        Platform::Mac,
        &after,
        &hash,
    )
    .unwrap();
    assert!(validate_bundle(&candidate, &before).is_err());
    let old = old_path(&target).unwrap();
    swap(&target, &candidate, &old, Platform::Mac).unwrap();
    validate_bundle(&target, &after).unwrap();
    validate_bundle(&old, &before).unwrap();
    rollback(&target, &old, &candidate, Platform::Mac).unwrap();
    validate_bundle(&target, &before).unwrap();
    assert!(!old.exists());
    fixture(&bundle, "0.2.0", "another.bundle");
    assert!(validate_bundle(&bundle, &after).is_err());
}
