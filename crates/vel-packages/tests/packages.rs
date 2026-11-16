use std::{fs, io::Write, path::Path};
use tempfile::TempDir;
use vel_packages::{
    Index, Location, PackageManager, ResourceKind, Support, Version, https::HttpsClient,
};
use zip::{ZipWriter, write::SimpleFileOptions};

fn archive(path: &Path, version: &str, entries: &[(&str, &[u8])]) {
    let mut zip = ZipWriter::new(fs::File::create(path).unwrap());
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("package-metadata.json", options).unwrap();
    write!(zip, "{{\"version\":\"{version}\"}}").unwrap();
    for (name, bytes) in entries {
        zip.start_file(*name, options).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
}
fn store() -> (TempDir, PackageManager) {
    let root = tempfile::tempdir().unwrap();
    let manager = PackageManager::new(root.path().join("profile"));
    (root, manager)
}

#[test]
fn discovery_is_offline_and_does_not_create_a_fresh_profile() {
    let (_root, manager) = store();
    assert!(manager.index_url().is_none());
    assert!(manager.list().unwrap().is_empty());
    assert!(
        manager
            .check_upgrades(&HttpsClient::default())
            .unwrap()
            .is_empty()
    );
    assert!(!manager.data_dir().exists());
}

#[test]
fn installs_lists_overlays_and_reports_all_declarative_and_python_resources() {
    let (root, manager) = store();
    let source = root.path().join("source.sublime-package");
    let entries = [
        ("Language.sublime-syntax", b"syntax".as_slice()),
        ("Language.tmLanguage.json", b"{}"),
        ("Color.sublime-color-scheme", b"{}"),
        ("Old.tmTheme", b"xml"),
        ("Theme.sublime-theme", b"[]"),
        ("Keys.sublime-keymap", b"[]"),
        ("x.sublime-snippet", b"xml"),
        ("x.sublime-completions", b"{}"),
        ("x.sublime-build", b"{}"),
        ("plugins/plugin.py", b"raise Exception('never execute')"),
        ("image.png", b"ignored resource kind"),
    ];
    archive(&source, "1.2.3", &entries);
    let source_bytes = fs::read(&source).unwrap();
    let installed = manager.install_local("Example", &source).unwrap();
    assert!(installed.backup.is_none());
    assert_eq!(fs::read(&source).unwrap(), source_bytes);
    let loose = manager.data_dir().join("Packages/Example");
    fs::create_dir_all(&loose).unwrap();
    fs::write(loose.join("Language.sublime-syntax"), "loose override").unwrap();
    fs::write(
        loose.join("package-metadata.json"),
        r#"{"version":"1.2.4"}"#,
    )
    .unwrap();
    let packages = manager.list().unwrap();
    assert_eq!(packages.len(), 1);
    let package = &packages[0];
    assert_eq!(package.name, "Example");
    assert_eq!(package.version, Some(Version::parse("1.2.4").unwrap()));
    assert!(matches!(
        &package.locations[..],
        [Location::Archive(_), Location::Loose(_)]
    ));
    assert_eq!(package.contributions.len(), 9);
    assert_eq!(
        package
            .contributions
            .iter()
            .map(|c| c.kind)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        7
    );
    assert!(
        package
            .contributions
            .iter()
            .any(|c| c.kind == ResourceKind::BuildSystem)
    );
    assert_eq!(package.python_plugins, ["plugins/plugin.py"]);
    assert_eq!(package.support(), Support::PartialNoPluginApi);
    assert_eq!(
        package.support().label(),
        "partially supported: no plugin API"
    );
    assert!(!package.disabled);
}

#[test]
fn replacement_is_atomic_recoverable_and_bad_zip_keeps_the_old_archive() {
    let (root, manager) = store();
    let source = root.path().join("source.sublime-package");
    archive(&source, "1.0.0", &[("x.sublime-syntax", b"original")]);
    let first = manager.install_local("Example", &source).unwrap();
    let original = fs::read(&first.path).unwrap();
    fs::write(&source, "not a zip").unwrap();
    assert!(manager.install_local("Example", &source).is_err());
    assert_eq!(fs::read(&first.path).unwrap(), original);
    archive(&source, "2.0.0", &[("x.sublime-syntax", b"new")]);
    let second = manager.install_local("Example", &source).unwrap();
    assert_eq!(fs::read(second.backup.unwrap()).unwrap(), original);
    assert_ne!(fs::read(&second.path).unwrap(), original);
    assert_eq!(manager.list().unwrap().len(), 1);
    let lock = fs::File::open(manager.data_dir().join(".velocity-packages.lock")).unwrap();
    lock.try_lock().unwrap();
}

#[test]
fn rejects_corrupt_crc_traversal_case_collisions_links_and_oversized_resources() {
    let (root, manager) = store();
    let source = root.path().join("unsafe.sublime-package");
    for path in [
        "../outside",
        "/absolute",
        "folder/../../outside",
        "C:\\evil",
        "folder\\evil",
    ] {
        archive(&source, "1.0.0", &[(path, b"x")]);
        assert!(manager.install_local("Unsafe", &source).is_err(), "{path}");
    }
    archive(
        &source,
        "1.0.0",
        &[("A.sublime-syntax", b"x"), ("a.sublime-syntax", b"x")],
    );
    assert!(manager.install_local("Unsafe", &source).is_err());
    {
        let mut zip = ZipWriter::new(fs::File::create(&source).unwrap());
        zip.add_symlink("link", "../escape", SimpleFileOptions::default())
            .unwrap();
        zip.finish().unwrap();
    }
    assert!(manager.install_local("Unsafe", &source).is_err());
    archive(&source, "1.0.0", &[("corrupt.txt", b"UNIQUE_CRC_CONTENT")]);
    let mut bytes = fs::read(&source).unwrap();
    let index = bytes
        .windows(18)
        .position(|w| w == b"UNIQUE_CRC_CONTENT")
        .unwrap();
    bytes[index] ^= 1;
    fs::write(&source, bytes).unwrap();
    assert!(manager.install_local("Unsafe", &source).is_err());
    {
        let mut zip = ZipWriter::new(fs::File::create(&source).unwrap());
        zip.start_file(
            "huge",
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
        let megabyte = vec![0; 1024 * 1024];
        for _ in 0..33 {
            zip.write_all(&megabyte).unwrap();
        }
        zip.finish().unwrap();
    }
    assert!(manager.install_local("Unsafe", &source).is_err());
    assert!(manager.list().unwrap().is_empty());
    assert!(!root.path().join("outside").exists());
}

#[test]
fn invalid_names_and_user_package_are_rejected_before_writing() {
    let (root, manager) = store();
    for name in [
        "",
        "..",
        ".hidden",
        "../escape",
        "a/b",
        "a\\b",
        "a:b",
        " trailing ",
        "a\n",
        "User",
        "user",
        "USER",
    ] {
        assert!(
            manager
                .install_local(name, &root.path().join("absent"))
                .is_err()
        );
        assert!(manager.remove(name).is_err());
        assert!(manager.set_enabled(name, false).is_err());
    }
    assert!(!manager.data_dir().exists());
}

#[test]
fn enable_disable_preserves_preferences_comments_and_makes_backups() {
    let (_root, manager) = store();
    let preferences = manager
        .data_dir()
        .join("Packages/User/Preferences.sublime-settings");
    fs::create_dir_all(preferences.parent().unwrap()).unwrap();
    let original = "{\n // keep this comment\n \"font_size\": 15,\n \"ignored_packages\": [\"Existing\"],\n}\n";
    fs::write(&preferences, original).unwrap();
    let backup = manager.set_enabled("Example", false).unwrap().unwrap();
    assert_eq!(fs::read_to_string(backup).unwrap(), original);
    let changed = fs::read_to_string(&preferences).unwrap();
    assert!(changed.contains("// keep this comment"));
    assert!(changed.contains("\"font_size\": 15"));
    assert_eq!(
        manager
            .ignored_packages()
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        ["Example", "Existing"]
    );
    manager.set_enabled("Example", true).unwrap();
    assert_eq!(
        manager
            .ignored_packages()
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        ["Existing"]
    );
    fs::write(&preferences, r#"{"ignored_packages": 3}"#).unwrap();
    assert!(manager.set_enabled("Example", false).is_err());
    assert_eq!(
        fs::read_to_string(&preferences).unwrap(),
        r#"{"ignored_packages": 3}"#
    );
}

#[test]
fn remove_covers_both_sources_preserves_recoverable_backups_and_is_idempotent() {
    let (root, manager) = store();
    let source = root.path().join("source.sublime-package");
    archive(&source, "1.0.0", &[("x.sublime-syntax", b"zip")]);
    manager.install_local("Example", &source).unwrap();
    let loose = manager.data_dir().join("Packages/Example");
    fs::create_dir_all(&loose).unwrap();
    fs::write(loose.join("x.sublime-syntax"), "loose").unwrap();
    let backups = manager.remove("Example").unwrap();
    assert_eq!(backups.len(), 2);
    assert!(backups.iter().all(|p| p.exists()));
    assert_eq!(
        fs::read_to_string(backups[1].join("x.sublime-syntax")).unwrap(),
        "loose"
    );
    assert!(manager.list().unwrap().is_empty());
    let mut resources = vel_config::resources::Resources::default();
    resources.load(manager.data_dir()).unwrap();
    assert!(
        resources.matching(".sublime-syntax").is_empty(),
        "backups must stay outside the loader's search roots"
    );
    assert!(manager.remove("Example").unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn symlinked_package_directories_and_preferences_never_redirect_writes_or_removal() {
    use std::os::unix::fs::symlink;
    let (root, manager) = store();
    let source = root.path().join("source.sublime-package");
    archive(&source, "1.0.0", &[("x.sublime-syntax", b"zip")]);
    let installed = manager.install_local("Example", &source).unwrap();
    let outside = root.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::create_dir_all(manager.data_dir().join("Packages")).unwrap();
    symlink(&outside, manager.data_dir().join("Packages/Example")).unwrap();
    assert!(manager.remove("Example").is_err());
    assert!(
        installed.path.exists(),
        "preflight must preserve the archive"
    );
    symlink(&outside, manager.data_dir().join("Packages/User")).unwrap();
    assert!(manager.set_enabled("Example", false).is_err());
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
}

#[test]
fn store_lock_rejects_overlapping_mutations_and_is_not_deleted_by_the_failed_caller() {
    let (_root, manager) = store();
    fs::create_dir(manager.data_dir()).unwrap();
    let lock = manager.data_dir().join(".velocity-packages.lock");
    fs::write(&lock, "other process").unwrap();
    let held = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock)
        .unwrap();
    held.try_lock().unwrap();
    assert!(manager.remove("Example").is_err());
    assert_eq!(fs::read_to_string(lock).unwrap(), "other process");
    drop(held);
    assert!(manager.remove("Example").unwrap().is_empty());
}

#[test]
fn first_user_disable_override_keeps_inherited_ignored_defaults() {
    let (_root, manager) = store();
    let inherited = ["Vintage".to_owned()].into_iter().collect();
    manager
        .set_enabled_inheriting("Example", false, &inherited)
        .unwrap();
    assert_eq!(
        manager
            .ignored_packages()
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        ["Example", "Vintage"]
    );
    manager
        .set_enabled_inheriting("Vintage", true, &inherited)
        .unwrap();
    assert_eq!(
        manager
            .ignored_packages()
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        ["Example"]
    );
}

#[test]
fn index_checks_semver_prereleases_unknown_versions_and_rejects_invalid_schema() {
    let (root, manager) = store();
    let source = root.path().join("source.sublime-package");
    archive(&source, "1.9.0", &[("x.sublime-syntax", b"zip")]);
    manager.install_local("Example", &source).unwrap();
    let release = |version: &str| {
        serde_json::json!({"name":"Example", "version":version,
        "url":"https://example.invalid/package", "sha256":"0".repeat(64)})
    };
    let index = Index::parse(
        &serde_json::to_vec(&serde_json::json!({"schema_version":1,
        "packages":[release("1.10.0"), release("2.0.0-beta.1"), release("1.9.0")]}))
        .unwrap(),
    )
    .unwrap();
    let packages = manager.list().unwrap();
    let upgrades = index.upgrades(&packages).unwrap();
    assert_eq!(upgrades.len(), 1);
    assert_eq!(
        upgrades[0].available.version,
        Version::parse("1.10.0").unwrap()
    );
    let mut unknown = packages;
    unknown[0].version = None;
    assert!(
        index.upgrades(&unknown).unwrap()[0]
            .installed_version
            .is_none()
    );
    unknown[0].version = Some(Version::parse("3.0.0").unwrap());
    assert!(index.upgrades(&unknown).unwrap().is_empty());
    for value in [
        serde_json::json!({"schema_version":2, "packages":[]}),
        serde_json::json!({"schema_version":1, "packages":[], "unknown":true}),
        serde_json::json!({"schema_version":1, "packages":[release("1.0.0"), release("1.0.0")]}),
        serde_json::json!({"schema_version":1, "packages":[{"name":"Example","version":"1.0.0","url":"http://insecure","sha256":"0".repeat(64)}]}),
    ] {
        assert!(Index::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}
