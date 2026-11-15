use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use vel_git::{Badge, DiffTarget, GitService, Head, Repository};
fn git(path: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-b", "main"]);
    fs::create_dir(dir.path().join("src")).unwrap();
    for file in [
        "src/modified",
        "src/missing",
        "src/staged_mod",
        "src/staged_del",
        "src/conflict",
        "literal[1].rs",
    ] {
        fs::write(dir.path().join(file), "base\n").unwrap();
    }
    fs::write(dir.path().join(".gitignore"), "ignored/\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "fixture"]);
    dir
}
#[test]
fn badges_both_layers_blobs_folders_and_literal_incremental_refresh() {
    let dir = fixture();
    let p = dir.path();
    fs::write(p.join("src/modified"), "working\n").unwrap();
    fs::remove_file(p.join("src/missing")).unwrap();
    fs::write(p.join("src/staged_mod"), "staged\n").unwrap();
    git(p, &["add", "src/staged_mod"]);
    fs::write(p.join("src/staged_mod"), "working2\n").unwrap();
    fs::write(p.join("src/new"), "new\n").unwrap();
    git(p, &["add", "src/new"]);
    git(p, &["rm", "src/staged_del"]);
    fs::write(p.join("src/untracked"), "untracked\n").unwrap();
    fs::create_dir(p.join("ignored")).unwrap();
    fs::write(p.join("ignored/file"), "ignored\n").unwrap();
    let index_before = fs::read(p.join(".git/index")).unwrap();
    let mut repo = Repository::discover(p.join("src")).unwrap();
    let status = repo.refresh().unwrap();
    for (path, badge) in [
        ("src/modified", Badge::Modified),
        ("src/missing", Badge::Missing),
        ("src/staged_mod", Badge::StagedModification),
        ("src/new", Badge::StagedAddition),
        ("src/staged_del", Badge::StagedDeletion),
        ("src/untracked", Badge::Untracked),
        ("ignored/file", Badge::Ignored),
    ] {
        assert_eq!(status.badge(Path::new(path)), Some(badge), "{path}");
    }
    assert_eq!(status.files[Path::new("src/staged_mod")].statuses.len(), 2);
    assert_eq!(status.folders[Path::new("src")], Badge::StagedDeletion);
    assert_eq!(
        repo.blob(Path::new("src/staged_mod"), DiffTarget::Head)
            .unwrap(),
        Some(b"base\n".to_vec())
    );
    assert_eq!(
        repo.blob(Path::new("src/staged_mod"), DiffTarget::Index)
            .unwrap(),
        Some(b"staged\n".to_vec())
    );
    assert_eq!(
        repo.blob(Path::new("src/new"), DiffTarget::Head).unwrap(),
        None
    );
    assert_eq!(fs::read(p.join(".git/index")).unwrap(), index_before);
    fs::write(p.join("literal[1].rs"), "changed\n").unwrap();
    repo.refresh_paths(&["literal[1].rs".into()]).unwrap();
    assert_eq!(
        repo.snapshot().unwrap().badge(Path::new("literal[1].rs")),
        Some(Badge::Modified)
    );
    assert_eq!(
        repo.snapshot().unwrap().badge(Path::new("src/staged_del")),
        Some(Badge::StagedDeletion)
    );
    fs::write(p.join("literal[1].rs"), "base\n").unwrap();
    repo.refresh_paths(&["literal[1].rs".into()]).unwrap();
    assert_eq!(
        repo.snapshot().unwrap().badge(Path::new("literal[1].rs")),
        None
    );
    assert!(repo.refresh_paths(&["../outside".into()]).is_err());
}
#[test]
fn unmerged_index_unborn_detached_worktrees_and_cheap_ahead_behind() {
    let dir = fixture();
    let p = dir.path();
    let id = git(p, &["rev-parse", "HEAD:src/conflict"]);
    let mut child = Command::new("git")
        .arg("-C")
        .arg(p)
        .args(["update-index", "--index-info"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    write!(child.stdin.take().unwrap(),"0 {}\tsrc/conflict\n100644 {id} 1\tsrc/conflict\n100644 {id} 2\tsrc/conflict\n100644 {id} 3\tsrc/conflict\n","0".repeat(40)).unwrap();
    assert!(child.wait().unwrap().success());
    let mut repo = Repository::discover(p).unwrap();
    assert_eq!(
        repo.refresh().unwrap().badge(Path::new("src/conflict")),
        Some(Badge::Unmerged)
    );
    assert!(
        repo.blob(Path::new("src/conflict"), DiffTarget::Index)
            .is_err()
    );
    git(p, &["reset", "--hard", "HEAD"]);
    git(p, &["branch", "upstream"]);
    git(p, &["branch", "--set-upstream-to=upstream", "main"]);
    assert_eq!(repo.refresh().unwrap().ahead_behind.unwrap().ahead, 0);
    fs::write(p.join("new_commit"), "new").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-m", "ahead"]);
    assert_eq!(repo.refresh().unwrap().ahead_behind.unwrap().ahead, 1);
    let parent = tempfile::tempdir().unwrap();
    let worktree = parent.path().join("linked");
    git(
        p,
        &[
            "worktree",
            "add",
            "--detach",
            worktree.to_str().unwrap(),
            "HEAD",
        ],
    );
    let mut linked = Repository::discover(&worktree).unwrap();
    assert!(matches!(linked.refresh().unwrap().head, Head::Detached(_)));
    assert_ne!(linked.git_dir(), worktree.join(".git"));
    let unborn = tempfile::tempdir().unwrap();
    git(unborn.path(), &["init", "-b", "unborn"]);
    let mut repo = Repository::discover(unborn.path()).unwrap();
    assert!(matches!(
        repo.refresh().unwrap().head,
        Head::Branch { unborn: true, .. }
    ));
    assert_eq!(
        repo.blob(Path::new("missing"), DiffTarget::Head).unwrap(),
        None
    );
}
fn wait_for(service: &GitService, condition: impl Fn(&vel_git::StatusSnapshot) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(error) = service.try_error() {
            panic!("worker: {error}");
        }
        if let Some(snapshot) = service.try_snapshot()
            && condition(&snapshot)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "worker timeout: {:?}",
            service.try_snapshot()
        );
        thread::sleep(Duration::from_millis(20));
    }
}
#[test]
fn worker_watches_worktree_and_index_and_coalesces_paths() {
    let dir = fixture();
    let p = dir.path();
    let service = GitService::with_watch_mode(
        p.into(),
        vel_git::WatchMode::Polling(Duration::from_millis(50)),
    )
    .unwrap();
    wait_for(&service, |_| true);
    fs::write(p.join("src/modified"), "changed\n").unwrap();
    wait_for(&service, |s| {
        s.badge(Path::new("src/modified")) == Some(Badge::Modified)
    });
    git(p, &["add", "src/modified"]);
    wait_for(&service, |s| {
        s.badge(Path::new("src/modified")) == Some(Badge::StagedModification)
    });
    fs::write(p.join("src/a"), "a").unwrap();
    fs::write(p.join("src/b"), "b").unwrap();
    for _ in 0..10 {
        service.refresh_paths(["src/a".into()]).unwrap();
        service.refresh_paths(["src/b".into()]).unwrap();
    }
    wait_for(&service, |s| {
        s.badge(Path::new("src/a")) == Some(Badge::Untracked)
            && s.badge(Path::new("src/b")) == Some(Badge::Untracked)
    });
}

#[cfg(unix)]
#[test]
fn tracked_symlink_changes_stay_repository_relative() {
    let dir = fixture();
    let p = dir.path();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("first"), "target").unwrap();
    fs::write(outside.path().join("second"), "target").unwrap();
    std::os::unix::fs::symlink(outside.path().join("first"), p.join("link")).unwrap();
    git(p, &["add", "link"]);
    git(p, &["commit", "-m", "symlink fixture"]);
    let service = GitService::with_watch_mode(
        p.into(),
        vel_git::WatchMode::Polling(Duration::from_millis(50)),
    )
    .unwrap();
    wait_for(&service, |_| true);
    fs::remove_file(p.join("link")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("second"), p.join("link")).unwrap();
    // Explicit paths also exercise the native-event adapter's public refresh contract.
    service.refresh_paths(["link".into()]).unwrap();
    wait_for(&service, |s| {
        s.badge(Path::new("link")) == Some(Badge::Modified)
    });
}
