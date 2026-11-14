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
    let metadata_baseline = baseline.scope_counts.passed + baseline.scope_counts.failed > 0;
    assert!(
        actual.overall.passed >= baseline.overall.passed,
        "total pass counts regressed"
    );
    assert!(
        actual.overall.skipped <= baseline.overall.skipped,
        "new skips must be reviewed"
    );
    if metadata_baseline {
        assert_eq!(
            actual.overall.passed + actual.overall.failed,
            baseline.overall.passed + baseline.overall.failed,
            "assertion denominator changed"
        );
    }
    for expected in &baseline.files {
        let got = actual
            .files
            .iter()
            .find(|f| f.file == expected.file)
            .expect("test file disappeared");
        let previous_scope = if metadata_baseline {
            &expected.scope_counts
        } else {
            &expected.counts
        };
        assert!(
            got.scope_counts.passed >= previous_scope.passed
                && got.scope_counts.failed <= previous_scope.failed,
            "{} scope regression",
            expected.file
        );
        if metadata_baseline {
            assert!(
                got.symbol_counts.passed >= expected.symbol_counts.passed
                    && got.symbol_counts.failed <= expected.symbol_counts.failed,
                "{} symbol regression",
                expected.file
            );
            assert!(
                got.reindent_counts.passed >= expected.reindent_counts.passed
                    && got.reindent_counts.failed <= expected.reindent_counts.failed,
                "{} reindent regression",
                expected.file
            );
        }
    }
    for (name, expected) in &baseline.syntaxes {
        let got = &actual.syntaxes[name];
        assert!(
            got.passed >= expected.passed && (!metadata_baseline || got.failed <= expected.failed),
            "{name}: {got:?}, baseline {expected:?}"
        );
    }
}
