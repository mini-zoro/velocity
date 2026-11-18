use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use vel_index::{IndexOptions, ProjectIndex};
use vel_syntax::{MetadataSet, SyntaxDefinition, SyntaxSetBuilder};

#[test]
#[ignore = "paired cold symbol-index timing on a 51,000-file fixture"]
fn round20_index_cold_timing() {
    let fixture = tempfile::tempdir().unwrap();
    let root = std::env::var_os("VELOCITY_INDEX_FIXTURE")
        .map_or_else(|| fixture.path().join("repo"), PathBuf::from);
    if !root.exists() {
        fs::create_dir_all(root.join(".git/info")).unwrap();
        fs::write(root.join(".gitignore"), "*.generated.fix\nignored/\n").unwrap();
        fs::write(root.join(".git/info/exclude"), "*.scratch.fix\n").unwrap();
        for i in 0..10_000 {
            let dir = root.join(format!("src/p{}/d{i}", i / 1000));
            fs::create_dir_all(&dir).unwrap();
            for j in 0..(5 + usize::from(i < 1000)) {
                fs::write(
                    dir.join(format!("unit{j}.fix")),
                    format!("fn symbol_{i}_{j}\ncall()\n"),
                )
                .unwrap();
            }
        }
    }
    let mut builder = SyntaxSetBuilder::new();
    builder.add_with_path(SyntaxDefinition::load_from_str("name: Fixture\nfile_extensions: [fix]\nscope: source.fixture\ncontexts:\n  main:\n    - match: 'symbol_[0-9]+_[0-9]+'\n      scope: entity.name.function.fixture\n", true, None).unwrap(), "Fixture.sublime-syntax");
    let opts = IndexOptions {
        cache_dir: Some(fixture.path().join("cache")),
        index_workers: 4,
        watch: std::env::var_os("VELOCITY_INDEX_WATCH").as_deref() != Some("0".as_ref()),
        ..IndexOptions::default()
    };
    let start = Instant::now();
    let index = ProjectIndex::open(
        &root,
        Arc::new(builder.build()),
        Arc::new(MetadataSet::new()),
        opts,
    )
    .unwrap();
    while index.status().active {
        assert!(
            start.elapsed() < Duration::from_secs(400),
            "index did not become idle"
        );
        std::thread::sleep(Duration::from_millis(15));
    }
    index.flush().unwrap();
    let status = index.status();
    eprintln!(
        "cold index (watch={}): {:.3}s, {} indexed, {} skipped",
        std::env::var_os("VELOCITY_INDEX_WATCH").as_deref() != Some("0".as_ref()),
        start.elapsed().as_secs_f64(),
        status.indexed_files,
        status.skipped_files
    );
    assert!(status.indexed_files >= 51_000);
    assert_eq!(index.lookup_definition("symbol_9980_3").len(), 1);
}
