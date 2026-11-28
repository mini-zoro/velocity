use std::{fs, path::Path};

/// A detached process can't be reaped by the test. Give it a private lease
/// instead of signaling a PID that could have been recycled.
pub struct DetachedFixture(tempfile::TempDir);

impl DetachedFixture {
    pub fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("lease"), "").unwrap();
        fs::write(
            root.path().join("detach.pl"),
            r#"use POSIX qw(setsid);
use Time::HiRes qw(time sleep);
my ($dir, $continuous) = @ARGV;
setsid() >= 0 or die "setsid: $!";
sub mark {
    my ($name) = @_;
    open my $file, ">", "$dir/$name.tmp" or return;
    close $file;
    rename "$dir/$name.tmp", "$dir/$name";
}
mark("ready");
$| = 1;
# Outlives every relaxed bound the tests use (30 s), so "returned before the holder let
# go" stays a real check on a loaded machine; the lease normally ends it much sooner.
my $deadline = time + 60;
while (-e "$dir/lease" && time < $deadline) {
    print "DAEMON\n" if $continuous;
    sleep 0.05;
}
mark("done");
"#,
        )
        .unwrap();
        // A file, not inline shell: build commands expand `$name` like Sublime, so the
        // command line itself must not contain `$`. The 120 s cap is a hang guard only;
        // the caller relies on `ready`, so the wait must not give up early on a slow
        // machine.
        fs::write(
            root.path().join("wait-ready.sh"),
            "i=0\nwhile [ ! -e \"$1/ready\" ] && [ \"$i\" -lt 12000 ]; do sleep 0.01; i=$((i+1)); done\n",
        )
        .unwrap();
        Self(root)
    }

    pub fn root(&self) -> &Path {
        self.0.path()
    }

    /// Starts the detached holder, waits until it has left the caller's process group
    /// (it marks `ready` after `setsid`), then runs `after`. Without the wait, a slow
    /// machine (CI) could reach `after` and finish before Perl detached, so the caller's
    /// process-group cleanup killed the holder and the pipe closed early.
    pub fn script(&self, continuous: bool, after: &str) -> String {
        let dir = self.root().to_str().unwrap().replace('\'', "'\\''");
        format!(
            "/usr/bin/perl '{dir}/detach.pl' '{dir}' {} &\n/bin/sh '{dir}/wait-ready.sh' '{dir}'\n{after}\n",
            u8::from(continuous),
        )
    }
}
