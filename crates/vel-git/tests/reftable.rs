use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};
use vel_git::{Badge, DiffTarget, Head, Repository};

fn git(root: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid");
    cmd
}

fn run(root: &Path, args: &[&str]) -> String {
    let output = git(root).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn git_generated_reftables_cover_head_branch_status_and_worktrees() {
    for hash in ["sha1", "sha256"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir(&root).unwrap();
        let format = format!("--object-format={hash}");
        let init = git(&root)
            .args([
                "init",
                "-q",
                "--ref-format=reftable",
                "-b",
                "zz-lane",
                &format,
            ])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&init.stderr);
        if !init.status.success()
            && (stderr.contains("unknown option") || stderr.contains("unknown ref storage format"))
        {
            eprintln!(
                "{} does not support --ref-format=reftable",
                run(&root, &["--version"])
            );
            return;
        }
        assert!(init.status.success(), "{stderr}");
        let mut repo = Repository::discover(&root).unwrap();
        assert_eq!(
            repo.refresh().unwrap().head,
            Head::Branch {
                name: "zz-lane".into(),
                unborn: true
            }
        );
        assert_eq!(
            repo.blob(Path::new("file.txt"), DiffTarget::Head).unwrap(),
            None
        );

        fs::write(root.join("file.txt"), "first\n").unwrap();
        run(&root, &["add", "file.txt"]);
        let tree = run(&root, &["write-tree"]);
        let id = run(&root, &["commit-tree", &tree, "-m", "fixture"]);
        run(&root, &["update-ref", "refs/heads/zz-lane", &id]);
        // Pack enough independently generated refs to exercise later reference blocks.
        let mut writer = git(&root)
            .args(["update-ref", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let mut input = writer.stdin.take().unwrap();
        for index in 0..256 {
            writeln!(input, "create refs/heads/test-{index:04} {id}").unwrap();
        }
        drop(input);
        assert!(writer.wait().unwrap().success());
        run(&root, &["pack-refs", "--all"]);
        run(&root, &["config", "branch.zz-lane.remote", "."]);
        run(
            &root,
            &["config", "branch.zz-lane.merge", "refs/heads/test-0000"],
        );

        fs::write(root.join("file.txt"), "staged\n").unwrap();
        run(&root, &["add", "file.txt"]);
        fs::write(root.join("new.txt"), "untracked").unwrap();
        let status = repo.refresh().unwrap();
        assert_eq!(
            status.head,
            Head::Branch {
                name: "zz-lane".into(),
                unborn: false
            }
        );
        assert_eq!(
            status.badge(Path::new("file.txt")),
            Some(Badge::StagedModification)
        );
        assert_eq!(status.badge(Path::new("new.txt")), Some(Badge::Untracked));
        assert_eq!(
            status.ahead_behind,
            Some(vel_git::AheadBehind {
                ahead: 0,
                behind: 0
            })
        );
        assert_eq!(
            repo.blob(Path::new("file.txt"), DiffTarget::Head)
                .unwrap()
                .unwrap(),
            b"first\n"
        );

        let linked = dir.path().join("linked");
        run(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                linked.to_str().unwrap(),
                &id,
            ],
        );
        let mut secondary = Repository::discover(&linked).unwrap();
        assert_eq!(
            secondary.refresh().unwrap().head,
            Head::Detached(id.clone())
        );
        assert!(secondary.snapshot().unwrap().files.is_empty());

        run(&root, &["update-ref", "--no-deref", "HEAD", &id]);
        assert_eq!(repo.refresh().unwrap().head, Head::Detached(id.clone()));
        run(&root, &["update-ref", "-d", "refs/heads/zz-lane"]);
        run(&root, &["symbolic-ref", "HEAD", "refs/heads/zz-lane"]);
        assert_eq!(
            repo.refresh().unwrap().head,
            Head::Branch {
                name: "zz-lane".into(),
                unborn: true
            }
        );
    }
}
