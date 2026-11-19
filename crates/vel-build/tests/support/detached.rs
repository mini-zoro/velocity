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
my $deadline = time + 8;
while (-e "$dir/lease" && time < $deadline) {
    print "DAEMON\n" if $continuous;
    sleep 0.05;
}
mark("done");
"#,
        )
        .unwrap();
        Self(root)
    }

    pub fn root(&self) -> &Path {
        self.0.path()
    }

    pub fn script(&self, continuous: bool, after: &str) -> String {
        let dir = self.root().to_str().unwrap().replace('\'', "'\\''");
        format!(
            "/usr/bin/perl '{dir}/detach.pl' '{dir}' {} &\n{after}\n",
            u8::from(continuous),
        )
    }
}
