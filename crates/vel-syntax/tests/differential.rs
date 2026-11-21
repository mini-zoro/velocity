//! Every official line is compared, including header/assertion lines and branch rewinds.
use std::{path::Path, sync::Arc};
use vel_regex::{MultiDfa, OnigBackend};
use vel_syntax::{ParseState, SharedLineOps, SyntaxSet};

#[test]
fn official_line_ops_rewinds_and_memoized_transitions_agree() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/sublime-packages");
    let onig = Arc::new(
        SyntaxSet::load_directories(&[&root])
            .unwrap()
            .with_matcher_backend(Arc::new(OnigBackend::default())),
    );
    let backend = Arc::new(MultiDfa::default());
    let multi = Arc::new((*onig).clone().with_matcher_backend(backend.clone()));
    let mut files: Vec<_> = walkdir::WalkDir::new(&root)
        .into_iter()
        .map(Result::unwrap)
        .filter(|entry| {
            entry.file_type().is_file()
                && entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("syntax_test_")
        })
        .map(|entry| entry.into_path())
        .collect();
    files.sort();
    assert_eq!(files.len(), 242);
    // Optional cross-revision evidence, independent of process-local scope IDs.
    let mut digest = std::env::var_os("VEL_SYNTAX_CORPUS_DIGEST").map(|_| blake3::Hasher::new());
    let mut lines = 0;
    let mut rewinds = 0;
    for path in files {
        if let Some(digest) = &mut digest {
            serde_json::to_writer(&mut *digest, path.strip_prefix(&root).unwrap()).unwrap();
        }
        let text = std::fs::read_to_string(&path).unwrap().replace('\r', "");
        let syntax = text.lines().next().unwrap().split('"').nth(1).unwrap();
        let mut reference =
            ParseState::new(onig.clone(), onig.find_syntax_by_path(syntax).unwrap())
                .with_line_cache_capacity(0);
        let mut candidate =
            ParseState::new(multi.clone(), multi.find_syntax_by_path(syntax).unwrap())
                .with_line_cache_capacity(0);
        let mut memoized =
            ParseState::new(multi.clone(), multi.find_syntax_by_path(syntax).unwrap());
        let mut shared = SharedLineOps::default();
        let mut shared_wide = Vec::new();
        for (row, line) in text.split_inclusive('\n').enumerate() {
            let expected = reference.parse_line(line).unwrap();
            if let Some(digest) = &mut digest {
                serde_json::to_writer(
                    &mut *digest,
                    &(
                        &expected.ops,
                        expected.rewind.map(|rewind| rewind.from_line),
                        &expected.corrections,
                        &expected.warnings,
                    ),
                )
                .unwrap();
                digest.update(b"\n");
            }
            assert_eq!(
                candidate.parse_line(line).unwrap(),
                expected,
                "{}:{}",
                path.display(),
                row + 1
            );
            memoized.parse_line_shared(line, &mut shared).unwrap();
            let corrections: Vec<_> = shared.drain_corrections().collect();
            assert_eq!(
                (
                    shared.ops.expanded(&mut shared_wide),
                    shared.rewind(),
                    corrections.as_slice(),
                    shared.warnings()
                ),
                (
                    expected.ops.as_slice(),
                    expected.rewind,
                    expected.corrections.as_slice(),
                    expected.warnings.as_slice()
                ),
                "shared memo {}:{}",
                path.display(),
                row + 1,
            );
            lines += 1;
            rewinds += usize::from(expected.rewind.is_some());
        }
    }
    assert_eq!(lines, 193_704);
    assert!(rewinds > 0);
    assert!(backend.stats().classifications > 0);
    assert!(backend.stats().resident_bytes <= 32 * 1024 * 1024);
    if let Some(digest) = digest {
        println!("syntax-differential-digest {}", digest.finalize());
    }
}
