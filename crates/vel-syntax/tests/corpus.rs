//! The pinned official corpus is the behavioral referee; failures cannot be silently skipped.
use std::path::Path;
use vel_syntax::{
    SyntaxSet,
    syntest::{self, SuiteResult},
};
#[test]
fn official_assertions_never_regress() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/sublime-packages");
    assert!(
        root.is_dir(),
        "initialize the pinned sublime-packages submodule before testing"
    );
    let set = SyntaxSet::load_directories(&[&root]).unwrap();
    assert_eq!(set.syntaxes().len(), 202);
    assert!(set.warnings().is_empty(), "{:?}", set.warnings());
    let actual = syntest::run(&set, &root, None);
    let baseline: SuiteResult = serde_json::from_str(include_str!("baseline.json")).unwrap();
    assert_eq!(actual.test_files, 242);
    assert!(
        actual.files.iter().all(|f| f.error.is_none()),
        "parser/header errors: {:?}",
        actual
            .files
            .iter()
            .filter(|f| f.error.is_some())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        actual.overall.skipped, baseline.overall.skipped,
        "new skips must be reviewed"
    );
    assert_eq!(
        actual.overall.passed + actual.overall.failed,
        baseline.overall.passed + baseline.overall.failed,
        "assertion denominator changed"
    );
    assert!(
        actual.overall.rate() >= baseline.overall.rate(),
        "overall regression: {} < {}",
        actual.overall.rate(),
        baseline.overall.rate()
    );
    for (name, expected) in &baseline.syntaxes {
        let got = &actual.syntaxes[name];
        assert!(
            got.passed >= expected.passed && got.failed <= expected.failed,
            "{name}: {got:?}, baseline {expected:?}"
        );
    }
}
