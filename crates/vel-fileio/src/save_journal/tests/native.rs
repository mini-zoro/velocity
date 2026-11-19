use super::*;

#[test]
fn staged_reference_survives_both_sides_of_creation_and_reaps_appledouble() {
    let profile = tempfile::tempdir().unwrap();
    let directory = tempfile::tempdir().unwrap();
    for mode in ["before-stage", "during-stage", "before-journal"] {
        child(profile.path(), &directory.path().join("target"), mode);
        let messages = recover_saves(Some(profile.path()));
        assert_eq!(
            messages.len(),
            usize::from(mode == "during-stage"),
            "{messages:?}"
        );
        assert_eq!(directory.path().read_dir().unwrap().count(), 0);
        assert_eq!(
            profile.path().join("Recovery").read_dir().unwrap().count(),
            0
        );
    }
}

#[test]
fn interrupted_private_stage_is_reaped_with_no_sibling_reference() {
    let profile = tempfile::tempdir().unwrap();
    let target = profile.path().join("existing.txt");
    fs::write(&target, "original").unwrap();
    child(profile.path(), &target, "during-private-stage");
    let recovery = profile.path().join("Recovery");
    let files: Vec<_> = recovery.read_dir().unwrap().flatten().collect();
    assert_eq!(files.len(), 1);
    assert!(staged_name(&files[0].path(), PRIVATE_STAGE));
    assert!(recover_saves(Some(profile.path())).is_empty());
    assert_eq!(recovery.read_dir().unwrap().count(), 0);
    assert_eq!(fs::read_to_string(&target).unwrap(), "original");
}

#[test]
fn first_save_through_a_link_cleans_the_real_parents_stale_stage() {
    let profile = tempfile::tempdir().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let canonical = directory.path().canonicalize().unwrap();
    child(
        profile.path(),
        &canonical.join("interrupted"),
        "during-stage",
    );
    let link = profile.path().join("link");
    std::os::unix::fs::symlink(&canonical, &link).unwrap();
    let result = save(&link.join("new-document"), profile.path(), "new contents").unwrap();
    assert!(
        result
            .warning
            .unwrap()
            .contains("Removed incomplete save stage")
    );
    assert_eq!(
        fs::read_to_string(canonical.join("new-document")).unwrap(),
        "new contents"
    );
    assert_eq!(canonical.read_dir().unwrap().count(), 1);
    assert_eq!(
        profile.path().join("Recovery").read_dir().unwrap().count(),
        0
    );
}

#[test]
fn native_image_child() {
    let Some(profile) = std::env::var_os("VELOCITY_IMAGE_PROFILE") else {
        return;
    };
    let target = PathBuf::from(std::env::var_os("VELOCITY_IMAGE_TARGET").unwrap());
    let root = Path::new(&profile).join("Recovery");
    let mode = std::env::var("VELOCITY_IMAGE_PHASE").unwrap();
    if mode == "before-journal" {
        ensure_directory(&root).unwrap();
        for prefix in [JOURNAL_TEMP, REFERENCE_TEMP] {
            let stage = Stage::new(&root, &preparation_prefix(prefix), None).unwrap();
            assert!(stage.path().is_file());
            std::mem::forget(stage);
        }
        die().unwrap();
    }
    if mode == "after-clone" {
        use_faults(&[(Phase::ClonePrepared, die)], || {
            save(&target, Path::new(&profile), "new")
        })
        .unwrap();
        panic!("save did not stop");
    }
    if let Some(size) = mode.strip_prefix("grow-") {
        undo::PARTIAL_WRITE.set(Some(4 * 1024 * 1024));
        let new = "n".repeat(size.parse().unwrap());
        use_faults(&[(Phase::Write, die)], || {
            save(&target, Path::new(&profile), &new)
        })
        .unwrap();
        panic!("save did not stop");
    }
    if let Some(size) = mode.strip_prefix("shrink-rollback-") {
        let new = "n".repeat(size.parse().unwrap());
        use_faults(
            &[(Phase::SyncTarget, io_failure), (Phase::RollbackTail, die)],
            || save(&target, Path::new(&profile), &new),
        )
        .unwrap();
        panic!("save did not stop");
    }
    if let Some(size) = mode.strip_prefix("shrink-forward-") {
        let new = "n".repeat(size.parse().unwrap());
        use_faults(
            &[(Phase::Backup, unsupported), (Phase::Truncate, die)],
            || save(&target, Path::new(&profile), &new),
        )
        .unwrap();
        panic!("save did not stop");
    }
    if mode == "recover-kill" {
        use_faults(&[(Phase::RollbackWrite, die)], || {
            recover_saves(Some(Path::new(&profile)))
        });
        panic!("recovery did not reach pwrite");
    }
    if let Some(point) = mode.strip_prefix("foreign-") {
        let size = target.metadata().unwrap().len() as usize;
        let phase = match point {
            "before-write" => Phase::BeforeWrite,
            "partial" => Phase::Write,
            "before-flush" => Phase::SyncTarget,
            _ => panic!("invalid interruption point"),
        };
        use_faults(&[(phase, die)], || {
            save(&target, Path::new(&profile), &"n".repeat(size))
        })
        .unwrap();
        panic!("save did not stop");
    }
    if mode == "inline-preparing" {
        use_faults(&[(Phase::SmallPreparing, die)], || {
            save(&target, Path::new(&profile), "never copied into target")
        })
        .unwrap();
        panic!("save did not stop");
    }
    if let Some(offset) = mode.strip_prefix("inline-") {
        let offset: usize = offset.parse().unwrap();
        undo::PARTIAL_WRITE.set(Some(offset));
        let revision = "new content!".repeat(1024);
        use_faults(
            &[(
                if offset == 0 {
                    Phase::SmallCreated
                } else {
                    Phase::Write
                },
                die,
            )],
            || save(&target, Path::new(&profile), &revision),
        )
        .unwrap();
        panic!("save did not stop");
    }
    let faults: &[Fault] = match mode.as_str() {
        "before-stage" => &[(Phase::BeforeStage, die)],
        "during-stage" => &[(Phase::AfterStage, || {
            let root =
                PathBuf::from(std::env::var_os("VELOCITY_IMAGE_PROFILE").unwrap()).join("Recovery");
            let reference = fs::read_dir(root)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .find(|p| p.extension().is_some_and(|e| e == "save-stage"))
                .unwrap();
            let mut value: serde_json::Value =
                serde_json::from_reader(File::open(&reference).unwrap()).unwrap();
            let path: JournalPath = serde_json::from_value(value["file"].clone()).unwrap();
            let inode = Entry::of(&fs::metadata(path.get()).unwrap());
            // A pre-upgrade reference stamped the empty inode. Inject an
            // obsolete ID on APFS as the native FAT write may do by itself.
            value["inode"] = serde_json::to_value(Entry {
                inode: inode.inode ^ 1,
                ..inode
            })
            .unwrap();
            fs::write(reference, serde_json::to_vec(&value).unwrap()).unwrap();
            fs::write(path.get(), vec![b'y'; 128 * 1024]).unwrap();
            let filename = path.get().file_name().unwrap().to_string_lossy();
            fs::write(
                path.get().with_file_name(format!("._{filename}")),
                "companion",
            )
            .unwrap();
            die()
        })],
        "during-private-stage" => &[
            (Phase::Backup, unsupported),
            (Phase::AfterStage, || {
                let root = PathBuf::from(std::env::var_os("VELOCITY_IMAGE_PROFILE").unwrap())
                    .join("Recovery");
                let stage = fs::read_dir(&root)
                    .unwrap()
                    .flatten()
                    .map(|e| e.path())
                    .find(|p| staged_name(p, PRIVATE_STAGE))
                    .unwrap();
                fs::write(&stage, vec![b'y'; 128 * 1024]).unwrap();
                die()
            }),
        ],
        "forward" => &[(Phase::Backup, unsupported), (Phase::Write, die)],
        _ => panic!("unknown child operation"),
    };
    let revision = "y".repeat(430_000);
    use_faults(faults, || save(&target, Path::new(&profile), &revision)).unwrap();
    panic!("save did not stop");
}

#[allow(unsafe_code)]
fn die() -> io::Result<()> {
    // SAFETY: This is called only in the dedicated subprocess.
    unsafe { libc::raise(libc::SIGKILL) };
    unreachable!()
}

fn child(profile: &Path, target: &Path, phase: &str) {
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "save_journal::tests::native::native_image_child"])
        .env("VELOCITY_IMAGE_PROFILE", profile)
        .env("VELOCITY_IMAGE_TARGET", target)
        .env("VELOCITY_IMAGE_PHASE", phase)
        .status()
        .unwrap();
    assert_eq!(status.signal(), Some(libc::SIGKILL), "{phase}: {status}");
}

fn archived_versions(profile: &Path) -> Vec<String> {
    fs::read_dir(profile.join("Recovery"))
        .unwrap()
        .flatten()
        .filter(|entry| staged_name(&entry.path(), retained::PREFIX))
        .map(|entry| fs::read_to_string(entry.path()).unwrap())
        .collect()
}

#[test]
fn killed_recovery_resumes_growing_and_shrinking_clone_saves() {
    // Non-aligned lengths also test the page split at the old and new EOF.
    for (before, after, phase) in [
        (2_700_003, 6_300_011, "grow"),
        (3_200_037, 700_003, "shrink-rollback"),
    ] {
        let profile = tempfile::tempdir().unwrap();
        let path = profile.path().join("document");
        let old = "o".repeat(before);
        fs::write(&path, &old).unwrap();
        child(profile.path(), &path, &format!("{phase}-{after}"));
        assert_ne!(fs::read(&path).unwrap(), old.as_bytes());
        if phase == "shrink-rollback" {
            assert_eq!(path.metadata().unwrap().len(), after as u64 + 1024 * 1024);
        } else {
            assert!(path.metadata().unwrap().len() > before as u64);
        }
        child(profile.path(), &path, "recover-kill");
        assert_eq!(path.metadata().unwrap().len(), before as u64);
        assert_ne!(fs::read(&path).unwrap(), old.as_bytes());
        let notices = recover_saves(Some(profile.path()));
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("Restored original"), "{notices:?}");
        assert_eq!(fs::read_to_string(&path).unwrap(), old);
        assert_eq!(
            profile.path().join("Recovery").read_dir().unwrap().count(),
            0
        );
    }
}

#[test]
fn staged_shrink_replays_only_with_a_verified_boundary_page() {
    for (before, after, foreign) in [
        (300_000, 1, None),
        (300_000, 133_000, None),
        (10_000_000, 1000, None),
        (300_000, 150_000, Some(149_995)),
        (300_000, 150_000, Some(150_005)),
        (3_000_000, 1_500_000, Some(1_499_995)),
    ] {
        let profile = tempfile::tempdir().unwrap();
        let path = profile.path().join("document");
        fs::write(&path, "o".repeat(before)).unwrap();
        child(profile.path(), &path, &format!("shrink-forward-{after}"));
        let new = "n".repeat(after);
        let expected = if let Some(offset) = foreign {
            regular::writable(&path)
                .unwrap()
                .write_all_at(b"other edit", offset)
                .unwrap();
            fs::read_to_string(&path).unwrap()
        } else {
            new.clone()
        };
        assert_eq!(path.metadata().unwrap().len(), before as u64);
        if foreign == Some(149_995) {
            // Publication can precede cleanup even for a same-inode edit.
            let first = use_faults(&[(Phase::ArchivePublished, io_failure)], || {
                recover_saves(Some(profile.path()))
            });
            assert!(first[0].contains("cleanup failed"), "{first:?}");
            assert_eq!(fs::read_to_string(&path).unwrap(), expected);
        }
        let notices = recover_saves(Some(profile.path()));
        assert_eq!(notices.len(), 1, "{notices:?}");
        if foreign.is_some() {
            assert!(
                notices[0].contains("no data was overwritten"),
                "{notices:?}"
            );
            assert_eq!(archived_versions(profile.path()), [new]);
        } else {
            assert!(
                notices[0].contains("Recovered interrupted save"),
                "{notices:?}"
            );
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), expected);
        assert!(recover_saves(Some(profile.path())).is_empty());
    }
}

#[test]
fn clone_proof_uses_less_space_than_a_second_revision() {
    const SIZE: usize = 40 * 1024 * 1024;
    let profile = tempfile::tempdir().unwrap();
    let target = profile.path().join("document");
    fs::write(&target, vec![b'o'; SIZE]).unwrap();
    let result = use_faults(
        &[
            // A full new stage would exceed the post-overwrite reserve.
            (Phase::BeforeStage, disk_full),
            (Phase::BeforeWrite, io_failure),
            (Phase::Rollback, || panic!("no data was overwritten")),
        ],
        || save(&target, profile.path(), &"n".repeat(SIZE)),
    );
    assert!(result.unwrap_err().contains("restored the original"));
    // Inspect a published proof at a real interrupted overwrite.
    child(profile.path(), &target, &format!("grow-{SIZE}"));
    let recovery = profile.path().join("Recovery");
    let files: Vec<_> = recovery.read_dir().unwrap().flatten().collect();
    assert_eq!(files.len(), 2);
    let table = files
        .iter()
        .find(|e| staged_name(&e.path(), PROOF))
        .unwrap();
    assert!(table.metadata().unwrap().len() < 5 * 1024 * 1024);
    assert_eq!(table.metadata().unwrap().len(), (SIZE / 4096 * 16) as u64);
    assert!(recover_saves(Some(profile.path()))[0].contains("Restored original"));
    use_faults(&[(Phase::BeforeStage, disk_full)], || {
        save(&target, profile.path(), &"n".repeat(SIZE))
    })
    .unwrap();
    assert_eq!(fs::read(&target).unwrap(), vec![b'n'; SIZE]);
}

#[test]
fn verified_disjoint_pages_can_recover_without_a_contiguous_new_prefix() {
    const SIZE: usize = 320_003;
    let profile = tempfile::tempdir().unwrap();
    let target = profile.path().join("document");
    let original = "o".repeat(SIZE);
    fs::write(&target, &original).unwrap();
    child(profile.path(), &target, "foreign-before-write");
    let file = regular::writable(&target).unwrap();
    for offset in [4096, 12288, 299008] {
        file.write_all_at(&[b'n'; 4096], offset).unwrap();
    }
    let notices = recover_saves(Some(profile.path()));
    assert!(notices[0].contains("Restored original"), "{notices:?}");
    assert_eq!(fs::read_to_string(&target).unwrap(), original);

    // The same verification rule also guards a forward journal.
    let new = "n".repeat(SIZE - 1);
    child(
        profile.path(),
        &target,
        &format!("shrink-forward-{}", new.len()),
    );
    for offset in [4096, 12288, 299008] {
        file.write_all_at(&[b'o'; 4096], offset).unwrap();
    }
    let notices = recover_saves(Some(profile.path()));
    assert!(
        notices[0].contains("Recovered interrupted save"),
        "{notices:?}"
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), new);
    assert!(recover_saves(Some(profile.path())).is_empty());
}

struct Image {
    file: PathBuf,
    mount: PathBuf,
    mounted: bool,
}

impl Image {
    fn new(scratch: &Path, name: &str, fs: &str) -> Self {
        Self::with_size(scratch, name, fs, "64m")
    }

    fn with_size(scratch: &Path, name: &str, fs: &str, size: &str) -> Self {
        let file = scratch.join(format!("{name}.dmg"));
        let mount = scratch.join(name);
        fs::create_dir(&mount).unwrap();
        checked(&[
            "create",
            "-size",
            size,
            "-fs",
            fs,
            "-volname",
            name,
            file.to_str().unwrap(),
        ]);
        let mut image = Self {
            file,
            mount,
            mounted: false,
        };
        image.attach();
        image
    }

    fn attach(&mut self) {
        checked(&[
            "attach",
            "-nobrowse",
            "-noverify",
            "-mountpoint",
            self.mount.to_str().unwrap(),
            self.file.to_str().unwrap(),
        ]);
        self.mounted = true;
    }

    fn detach(&mut self) {
        checked(&["detach", self.mount.to_str().unwrap()]);
        self.mounted = false;
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        if self.mounted {
            let _ = Command::new("/usr/bin/hdiutil")
                .arg("detach")
                .arg(&self.mount)
                .output();
        }
    }
}

fn checked(args: &[&str]) {
    let output = Command::new("/usr/bin/hdiutil")
        .args(args)
        .output()
        .expect("hdiutil is required for native image tests");
    assert!(
        output.status.success(),
        "hdiutil {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires hdiutil and permission to attach an APFS fixture image"]
fn native_apfs_clone_saves_40_mib_with_45_mib_free() {
    const MIB: u64 = 1024 * 1024;
    let scratch = tempfile::tempdir().unwrap();
    let image = Image::with_size(scratch.path(), "tight-apfs", "APFS", "200m");
    let profile = tempfile::tempdir_in(&image.mount).unwrap();
    let target = image.mount.join("document");
    let original = vec![b'o'; (40 * MIB) as usize];
    fs::write(&target, &original).unwrap();
    atomic::sync(&File::open(&target).unwrap(), Durability::Full).unwrap();
    let available = || {
        let stats = rustix::fs::statvfs(&image.mount).unwrap();
        stats.f_bavail * stats.f_frsize
    };
    let mut filler = File::create(image.mount.join("filler")).unwrap();
    let bytes = vec![b'f'; (MIB / 4) as usize];
    while available() > 45 * MIB {
        filler.write_all(&bytes).unwrap();
        filler.sync_all().unwrap();
    }
    assert!((44 * MIB..=45 * MIB).contains(&available()));
    let revised = "n".repeat(original.len());
    let entry = Entry::of(&fs::metadata(&target).unwrap());
    save(&target, profile.path(), &revised).unwrap();
    assert_eq!(Entry::of(&fs::metadata(&target).unwrap()), entry);
    assert_eq!(fs::read_to_string(&target).unwrap(), revised);
    assert!(recover_saves(Some(profile.path())).is_empty());
}

#[test]
#[ignore = "requires hdiutil and permission to attach FAT/exFAT fixture images"]
fn native_fat_images_save_empty_files_and_reap_interrupted_stages() {
    let scratch = tempfile::tempdir().unwrap();
    for (kind, filesystem) in [("fat32", "MS-DOS FAT32"), ("exfat", "ExFAT")] {
        let image = Image::new(scratch.path(), kind, filesystem);
        let profile = tempfile::tempdir().unwrap();
        let target = image.mount.join("target");
        fs::write(&target, "").unwrap();
        save(&target, profile.path(), "complete empty-file save").unwrap();
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "complete empty-file save"
        );
        fs::write(&target, "external overwrite").unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "external overwrite");
        child(profile.path(), &target, "during-private-stage");
        assert!(recover_saves(Some(profile.path())).is_empty());
        assert_eq!(
            profile.path().join("Recovery").read_dir().unwrap().count(),
            0
        );
        assert!(image.mount.read_dir().unwrap().flatten().all(|e| {
            !e.file_name()
                .as_encoded_bytes()
                .windows(STAGE.len())
                .any(|part| part == STAGE.as_bytes())
        }));
        fs::write(&target, "").unwrap();
        child(profile.path(), &target, "forward");
        let notices = recover_saves(Some(profile.path()));
        assert!(
            notices
                .iter()
                .any(|s| s.starts_with("Recovered interrupted save:")),
            "{notices:?}"
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "y".repeat(430_000));
    }
}

#[test]
#[ignore = "requires hdiutil and permission to detach and reattach HFS+ fixture images"]
fn native_hfs_remount_recovers_a_partial_write_with_a_new_device() {
    let scratch = tempfile::tempdir().unwrap();
    let mut hfs = Image::new(scratch.path(), "original", "HFS+");
    let profile = tempfile::tempdir().unwrap();
    let target = hfs.mount.join("target");
    fs::write(&target, "x".repeat(430_000)).unwrap();
    let before = target.metadata().unwrap();
    let original = volume::inspect(&File::open(&target).unwrap());
    assert!(original.uuid.is_some());
    child(profile.path(), &target, "forward");
    hfs.detach();
    let _other = Image::new(scratch.path(), "other", "HFS+");
    hfs.attach();
    let after = target.metadata().unwrap();
    assert_ne!(
        after.dev(),
        before.dev(),
        "fixture mount did not change its device"
    );
    assert_eq!(after.ino(), before.ino());
    let notices = recover_saves(Some(profile.path()));
    assert!(
        notices[0].starts_with("Recovered interrupted save:"),
        "{notices:?}"
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "y".repeat(430_000));
}

#[test]
#[ignore = "requires hdiutil and permission to attach an HFS+ fixture image"]
fn native_hfs_concurrent_same_length_saves_do_not_lose_updates() {
    let scratch = tempfile::tempdir().unwrap();
    let image = Image::new(scratch.path(), "coarse", "HFS+");
    let path = image.mount.join("race.txt");
    fs::write(&path, "").unwrap();
    assert!(volume::inspect(&File::open(&path).unwrap()).coarse_timestamps);
    super::race::simultaneous_saves(&path, false);
}

#[test]
fn inline_rollback_recovers_exact_original_at_varied_kill_offsets() {
    let profile = tempfile::tempdir().unwrap();
    let target = profile.path().join("target");
    let original = "original ab!".repeat(1024);
    let mut seed = 0x0a574d32_u32;
    let alias = profile.path().join("alias");
    fs::write(&target, &original).unwrap();
    fs::hard_link(&target, &alias).unwrap();
    let inode = target.metadata().unwrap().ino();
    for step in 0..12 {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let offset = if step == 0 {
            0
        } else {
            1 + seed as usize % (original.len() - 1)
        };
        child(profile.path(), &target, &format!("inline-{offset}"));
        let notices = recover_saves(Some(profile.path()));
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].starts_with(if step == 0 {
                "Removed unused save rollback"
            } else {
                "Restored original after interrupted save"
            }),
            "{notices:?}"
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), original);
        assert_eq!(fs::read_to_string(&alias).unwrap(), original);
        assert_eq!(target.metadata().unwrap().ino(), inode);
        assert!(recover_saves(Some(profile.path())).is_empty());
    }
}

#[test]
fn interrupted_inline_preparation_keeps_the_original_and_allows_the_next_save() {
    let profile = tempfile::tempdir().unwrap();
    let target = profile.path().join("target");
    fs::write(&target, "old text".repeat(128)).unwrap();
    child(profile.path(), &target, "inline-preparing");
    assert_eq!(fs::read_to_string(&target).unwrap(), "old text".repeat(128));
    let saved = save(&target, profile.path(), "successful retry").unwrap();
    let notice = saved.warning.unwrap();
    let retained = notice.split_once("kept at ").unwrap().1;
    assert!(Path::new(retained).is_file());
    assert_eq!(fs::read_to_string(&target).unwrap(), "successful retry");
    assert!(recover_saves(Some(profile.path())).is_empty());
}

#[test]
fn killed_rollback_never_overwrites_a_later_same_inode_edit() {
    for size in [8192, 360_000] {
        for (point, equal_length) in [
            ("before-write", false),
            ("partial", true),
            ("before-flush", true),
        ] {
            let profile = tempfile::tempdir().unwrap();
            let directory = tempfile::tempdir().unwrap();
            let target = directory.path().join("original.save-undo");
            let original = "o".repeat(size);
            fs::write(&target, &original).unwrap();
            let identity = Entry::of(&target.metadata().unwrap());
            child(profile.path(), &target, &format!("foreign-{point}"));
            let foreign = if equal_length {
                // A first-block digest alone could misidentify this edit.
                format!("{}{}", "n".repeat(4096), "z".repeat(size - 4096))
            } else {
                "independent rewrite\n".into()
            };
            fs::write(&target, &foreign).unwrap();
            assert_eq!(Entry::of(&target.metadata().unwrap()), identity);
            let notices = recover_saves(Some(profile.path()));
            assert_eq!(notices.len(), 1, "{notices:?}");
            let mut versions = archived_versions(profile.path());
            let mut expected = vec![original];
            let inline = size <= crate::save::INLINE_LIMIT;
            if inline {
                expected.push("n".repeat(size));
            }
            versions.sort();
            expected.sort();
            assert_eq!(versions, expected);
            assert_eq!(notices[0].contains("new revision kept at"), inline);
            assert_eq!(fs::read_to_string(&target).unwrap(), foreign);
            assert!(recover_saves(Some(profile.path())).is_empty());
            assert_eq!(
                profile.path().join("Recovery").read_dir().unwrap().count(),
                if inline { 2 } else { 1 }
            );
        }
    }
}

#[test]
fn reaps_an_abandoned_clone_but_leaves_a_live_publishers_clone() {
    let profile = tempfile::tempdir().unwrap();
    let target = profile.path().join("large.txt");
    let original = "o".repeat(430_000);
    fs::write(&target, &original).unwrap();
    child(profile.path(), &target, "after-clone");
    let recovery = profile.path().join("Recovery");
    let live = recovery.join(format!("{}live", preparation_prefix(BACKUP)));
    fs::write(&live, "fixture for a still running process").unwrap();
    assert_eq!(recovery.read_dir().unwrap().count(), 2);
    assert!(recover_saves(Some(profile.path())).is_empty());
    assert!(live.is_file());
    assert_eq!(recovery.read_dir().unwrap().count(), 1);
    assert_eq!(fs::read_to_string(&target).unwrap(), original);
}

#[test]
#[ignore = "requires hdiutil and permission to attach a local HFS+ image"]
fn native_hfs_small_saves_use_inline_rollback_and_preserve_the_inode() {
    let scratch = tempfile::tempdir().unwrap();
    let image = Image::new(scratch.path(), "inline", "HFS+");
    let target = image.mount.join("txt");
    let original = "o".repeat(8192);
    fs::write(&target, &original).unwrap();
    let before = target.metadata().unwrap().ino();
    let profile = tempfile::tempdir().unwrap();
    child(profile.path(), &target, "foreign-partial");
    let messages = recover_saves(Some(profile.path()));
    assert!(messages[0].starts_with("Restored original"), "{messages:?}");
    assert_eq!(fs::read_to_string(&target).unwrap(), original);
    assert_eq!(target.metadata().unwrap().ino(), before);
    assert_eq!(
        profile.path().join("Recovery").read_dir().unwrap().count(),
        0
    );
}

#[test]
fn coarse_uncontended_saves_do_not_read_the_original_twice() {
    let profile = tempfile::tempdir().unwrap();
    let target = profile.path().join("document");
    let original = "old".repeat(128_000);
    let new = "new".repeat(128_000);
    fs::write(&target, original).unwrap();
    COARSE_VERSION_PROBE.set(true);
    let result = use_faults(
        &[
            (Phase::Backup, unsupported),
            (Phase::DiskDigest, || {
                panic!("uncontended library save should not hash separately")
            }),
        ],
        || save(&target, profile.path(), &new),
    );
    COARSE_VERSION_PROBE.set(false);
    result.unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), new);
}

#[test]
fn staged_and_inline_saves_only_flush_once_per_device() {
    let profile = tempfile::tempdir().unwrap();
    let target = profile.path().join("document");
    for (cross_device, size) in [
        (false, 1024),
        (true, 1024),
        (false, 500_000),
        (true, 500_000),
    ] {
        fs::write(&target, "old!".repeat(size / 4)).unwrap();
        // Model a non-cloning volume while recording the requested flushes.
        volume::FAT_PROBE.set(true);
        CROSS_VOLUME_PROBE.set(cross_device);
        atomic::FLUSHES.with(|trace| trace.replace(Some(Vec::new())));
        let result = save(&target, profile.path(), &"n".repeat(size));
        let trace = atomic::FLUSHES.with(|trace| trace.take().unwrap());
        CROSS_VOLUME_PROBE.set(false);
        volume::FAT_PROBE.set(false);
        result.unwrap();
        let trace = if size == 1024 && !cross_device {
            // The first save also publishes the Recovery directory itself.
            &trace[1..]
        } else {
            &trace
        };
        let mut expected = Vec::new();
        if size > 1024 {
            expected.extend([Some(Durability::Cached); 2]);
        }
        expected.extend([
            None,
            Some(if cross_device {
                Durability::Full
            } else {
                Durability::Barrier
            }),
            Some(Durability::Full),
        ]);
        assert_eq!(trace, expected, "cross={cross_device}, size={size}");
        assert_eq!(fs::read_to_string(&target).unwrap(), "n".repeat(size));
    }
}
