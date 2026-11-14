use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use vel_index::{
    FileAnalysis, Index, IndexOptions, IndexedSymbol, ProjectIndex, SymbolKind, WordStatistics,
    analyze,
};
use vel_syntax::{MetadataSet, SyntaxDefinition, SyntaxSet, SyntaxSetBuilder};
use vel_text::Text;
fn set() -> Arc<SyntaxSet> {
    let mut builder = SyntaxSetBuilder::new();
    builder.add_with_path(SyntaxDefinition::load_from_str("name: Fixture\nfile_extensions: [fix]\nscope: source.fixture\ncontexts:\n  main:\n    - match: '(?<=fn )[\\p{L}_]+'\n      scope: entity.name.function.fixture\n    - match: '[\\p{L}_]+(?=\\()'\n      scope: variable.function.fixture\n",true,None).unwrap(),"Fixture.sublime-syntax");
    Arc::new(builder.build())
}
#[test]
fn definitions_references_transforms_unicode_and_replace_statistics() {
    let set = set();
    let metadata = MetadataSet::new();
    let source = "fn café\ncafé()\nlet answer = print!(answer);\nlet answer = print!(answer);\n";
    let analysis = analyze(Path::new("a.fix"), source, &set, &metadata).unwrap();
    assert_eq!(analysis.symbols.len(), 2);
    let mut index = Index::new();
    index.replace_file("a.fix", analysis);
    assert_eq!(index.lookup_definition("café")[0].column, 3);
    assert_eq!(index.lookup_definition("café")[0].range, 3..8);
    assert_eq!(index.lookup_references("café").len(), 1);
    assert_eq!(index.fuzzy_symbols("cf", 5)[0].location.name, "café");
    assert_eq!(index.word_count("answer"), 4);
    assert_eq!(index.sequence_count(&["let", "answer"]), 2);
    let words = index
        .suggest_words("pr", &["let", "answer"], 5, || false)
        .unwrap();
    assert_eq!(words[0].word, "print");
    assert_eq!(words[0].trailing, "!(");
    index.replace_file(
        "a.fix",
        analyze(Path::new("a.fix"), "fn changed\n", &set, &metadata).unwrap(),
    );
    assert!(index.lookup_definition("café").is_empty());
    assert_eq!(index.word_count("answer"), 0);
    assert_eq!(index.sequence_count(&["let", "answer"]), 0);
    assert!(index.fuzzy_symbols_cancel("", 2, || true).is_none());
    index.remove_file(Path::new("a.fix"));
    assert_eq!(index.symbol_count(), 0);
}
fn options(cache: &Path) -> IndexOptions {
    IndexOptions {
        cache_dir: Some(cache.into()),
        index_workers: 2,
        watch: false,
        debounce: Duration::from_millis(5),
        ..IndexOptions::default()
    }
}
#[test]
fn background_ignore_size_binary_persistence_overlay_staleness_and_deletion() {
    let directory = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::write(root.join("main.fix"), "fn disk\n").unwrap();
    std::fs::write(root.join("hidden.fix"), "fn ignored\n").unwrap();
    std::fs::write(root.join("skip.log"), "fn excluded\n").unwrap();
    std::fs::write(root.join("binary.fix"), b"fn nope\0").unwrap();
    std::fs::write(root.join("huge.fix"), "fn huge\n".repeat(100)).unwrap();
    std::fs::write(root.join(".gitignore"), "hidden.fix\n").unwrap();
    let mut settings = options(cache.path());
    settings.max_file_size = 100;
    let index =
        ProjectIndex::open(root, set(), Arc::new(MetadataSet::new()), settings.clone()).unwrap();
    index.flush().unwrap();
    assert_eq!(index.lookup_definition("disk").len(), 1);
    for excluded in ["ignored", "excluded", "nope", "huge"] {
        assert!(index.lookup_definition(excluded).is_empty(), "{excluded}");
    }
    index.update_buffer("main.fix", 2, Text::from("fn unsaved\n"));
    index.update_buffer("main.fix", 1, Text::from("fn stale\n"));
    index.flush().unwrap();
    assert!(index.lookup_definition("disk").is_empty());
    assert_eq!(index.lookup_definition("unsaved").len(), 1);
    assert!(index.lookup_definition("stale").is_empty());
    drop(index);
    // Cache stores disk snapshots, not the unsaved overlay. A restart restores it without parsing.
    let index = ProjectIndex::open(root, set(), Arc::new(MetadataSet::new()), settings).unwrap();
    index.flush().unwrap();
    assert_eq!(index.lookup_definition("disk").len(), 1);
    assert!(index.lookup_definition("unsaved").is_empty());
    assert!(index.status().cached_files >= 1);
    std::fs::write(root.join("main.fix"), "fn update\n").unwrap();
    index.reindex_file("main.fix");
    index.flush().unwrap();
    assert_eq!(index.lookup_definition("update").len(), 1);
    std::fs::remove_file(root.join("main.fix")).unwrap();
    index.reindex_file("main.fix");
    index.flush().unwrap();
    assert!(index.lookup_definition("update").is_empty());
    index.cancel();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !index.status().cancelled {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
}
#[test]
fn complete_journal_prefix_survives_truncated_tail() {
    let directory = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("a.fix"), "fn preserved\n").unwrap();
    let settings = options(cache.path());
    {
        let index = ProjectIndex::open(
            directory.path(),
            set(),
            Arc::new(MetadataSet::new()),
            settings.clone(),
        )
        .unwrap();
        index.flush().unwrap();
    }
    let path = std::fs::read_dir(cache.path().join("velocity/index"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&[255, 255])
        .unwrap();
    let index = ProjectIndex::open(
        directory.path(),
        set(),
        Arc::new(MetadataSet::new()),
        settings,
    )
    .unwrap();
    index.flush().unwrap();
    assert_eq!(index.lookup_definition("preserved").len(), 1);
}
#[test]
fn identical_names_in_many_files_survive_removal_and_slot_reuse() {
    let symbol = || IndexedSymbol {
        name: "same".into(),
        row: 0,
        column: 0,
        range: 0..4,
        kind: SymbolKind::Function,
        definition: true,
        reference: false,
    };
    let mut index = Index::new();
    for n in 0..100 {
        index.replace_file(
            format!("{n}.fix"),
            FileAnalysis {
                symbols: vec![symbol(); 10],
                words: WordStatistics::from_text("same same"),
            },
        );
    }
    index.remove_file(Path::new("50.fix"));
    assert_eq!(index.lookup_definition("same").len(), 990);
    assert_eq!(index.word_count("same"), 198);
    index.replace_file(
        "50.fix",
        FileAnalysis {
            symbols: vec![symbol()],
            words: WordStatistics::default(),
        },
    );
    assert_eq!(index.lookup_definition("same").len(), 991);
}

#[test]
fn ignored_files_stay_ignored_on_direct_updates_and_unsaved_edits() {
    let root = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(".gitignore"), "ignored.fix\n").unwrap();
    std::fs::write(root.path().join("ignored.fix"), "fn hidden\n").unwrap();
    let index = ProjectIndex::open(
        root.path(),
        set(),
        Arc::new(MetadataSet::new()),
        options(cache.path()),
    )
    .unwrap();
    index.flush().unwrap();
    index.reindex_file("ignored.fix");
    index.update_buffer("ignored.fix", 1, Text::from("fn overlay\n"));
    index.flush().unwrap();
    assert!(index.lookup_definition("hidden").is_empty());
    assert!(index.lookup_definition("overlay").is_empty());
    let mut settings = options(cache.path());
    settings.index_exclude_gitignore = false;
    let included =
        ProjectIndex::open(root.path(), set(), Arc::new(MetadataSet::new()), settings).unwrap();
    included.flush().unwrap();
    assert_eq!(included.lookup_definition("hidden").len(), 1);
}

#[test]
fn replacement_index_matches_a_simple_file_model() {
    use std::collections::BTreeMap;
    let mut model: BTreeMap<String, FileAnalysis> = BTreeMap::new();
    let mut index = Index::new();
    let mut seed = 19u64;
    for turn in 0..400 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let file = format!("{}.fix", seed % 13);
        if turn % 9 == 0 {
            index.remove_file(Path::new(&file));
            model.remove(&file);
        } else {
            let mut analysis = FileAnalysis::default();
            for n in 0..(seed % 7 + 1) as u32 {
                let name = format!("name_{}", (seed + n as u64) % 23);
                analysis.symbols.push(IndexedSymbol {
                    name: name.clone(),
                    row: n,
                    column: n % 3,
                    range: n * 10..n * 10 + 4,
                    kind: SymbolKind::Variable,
                    definition: n % 2 == 0,
                    reference: n % 3 == 0,
                });
                *analysis.words.words.entry(name).or_default() += 1;
            }
            index.replace_file(file.clone(), analysis.clone());
            model.insert(file, analysis);
        }
        for n in 0..23 {
            let name = format!("name_{n}");
            let mut defs = 0;
            let mut refs = 0;
            let mut words = 0;
            for analysis in model.values() {
                defs += analysis
                    .symbols
                    .iter()
                    .filter(|s| s.name == name && s.definition)
                    .count();
                refs += analysis
                    .symbols
                    .iter()
                    .filter(|s| s.name == name && s.reference)
                    .count();
                words += analysis.words.words.get(&name).copied().unwrap_or(0);
            }
            assert_eq!(index.lookup_definition(&name).len(), defs);
            assert_eq!(index.lookup_references(&name).len(), refs);
            assert_eq!(index.word_count(&name), words);
        }
    }
}

#[test]
fn notify_changes_or_reported_watcher_fallback_update_the_index() {
    let root = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("hot.fix"), "fn original\n").unwrap();
    let mut settings = options(cache.path());
    settings.watch = true;
    let index =
        ProjectIndex::open(root.path(), set(), Arc::new(MetadataSet::new()), settings).unwrap();
    index.flush().unwrap();
    let fallback = index
        .status()
        .logs
        .iter()
        .any(|line| line.contains("watcher unavailable"));
    std::fs::write(root.path().join("hot.fix"), "fn notified\n").unwrap();
    if fallback {
        index.reindex_file("hot.fix");
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while index.lookup_definition("notified").is_empty() {
        assert!(
            Instant::now() < deadline,
            "watcher update timed out: {:?}",
            index.status()
        );
        std::thread::park_timeout(Duration::from_millis(10));
    }
    assert!(index.lookup_definition("original").is_empty());
}

#[test]
fn a_project_local_cache_is_excluded_from_its_own_watcher_and_walker() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("main.fix"), "fn safe\n").unwrap();
    let mut settings = options(&root.path().join("cache"));
    settings.watch = true;
    let index =
        ProjectIndex::open(root.path(), set(), Arc::new(MetadataSet::new()), settings).unwrap();
    index.flush().unwrap();
    assert_eq!(index.status().total_files, 1);
    assert_eq!(index.lookup_definition("safe").len(), 1);
    index.update_buffer(root.path().join("main.fix"), 1, Text::from("fn overlay\n"));
    index.flush().unwrap();
    assert_eq!(index.lookup_definition("overlay").len(), 1);
}

#[test]
fn shared_highlighter_analysis_matches_disk_extraction_and_keeps_unsaved_precedence() {
    use vel_index::analyze_scopes;
    let set = set();
    let text = Text::from("fn shared\nshared()\n");
    let metadata = Arc::new(MetadataSet::new());
    let mut highlighter = vel_highlight::DocumentHighlighter::new(
        text.clone(),
        Default::default(),
        set.clone(),
        &set.syntaxes()[0],
        Arc::new(vel_color_scheme::ColorScheme::velocity_dark()),
    )
    .with_metadata(metadata.clone());
    highlighter.finish().unwrap();
    let scopes = highlighter.scope_snapshot(0..text.len_lines());
    let symbols = highlighter.symbols().unwrap();
    let shared = analyze_scopes(&scopes, &symbols).unwrap();
    let disk = analyze(Path::new("main.fix"), &text.to_string(), &set, &metadata).unwrap();
    assert_eq!(
        bincode::serialize(&shared).unwrap(),
        bincode::serialize(&disk).unwrap()
    );
    assert!(analyze_scopes(&highlighter.scope_snapshot(0..1), &symbols).is_err());
    let root = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("main.fix"), "fn disk\n").unwrap();
    let index = ProjectIndex::open(root.path(), set, metadata, options(cache.path())).unwrap();
    index.flush().unwrap();
    index.update_buffer_analysis("main.fix", 1, text, shared);
    index.reindex_file("main.fix");
    index.flush().unwrap();
    assert_eq!(index.lookup_definition("shared").len(), 1);
    assert_eq!(index.lookup_references("shared").len(), 1);
    assert!(index.lookup_definition("disk").is_empty());
}

#[test]
fn publishing_prepared_analysis_does_not_start_a_second_parser() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use vel_regex::{MatcherBackend, OnigBackend, PatternSet, PatternSpec};
    #[derive(Debug)]
    struct Counting(Arc<AtomicUsize>);
    impl MatcherBackend for Counting {
        fn build(&self, patterns: Vec<PatternSpec>) -> PatternSet {
            self.0.fetch_add(1, Ordering::SeqCst);
            OnigBackend::default().build(patterns)
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let syntax = Arc::new(
        Arc::try_unwrap(set())
            .unwrap()
            .with_matcher_backend(Arc::new(Counting(calls.clone()))),
    );
    let root = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let index = ProjectIndex::open(
        root.path(),
        syntax,
        Arc::new(MetadataSet::new()),
        options(cache.path()),
    )
    .unwrap();
    index.flush().unwrap();
    let analysis = FileAnalysis {
        symbols: vec![IndexedSymbol {
            name: "shared".into(),
            row: 0,
            column: 3,
            range: 3..9,
            kind: SymbolKind::Function,
            definition: true,
            reference: false,
        }],
        words: WordStatistics::from_text("fn shared"),
    };
    index.update_buffer_analysis("main.fix", 1, Text::from("fn shared\n"), analysis);
    index.flush().unwrap();
    assert_eq!(index.lookup_definition("shared").len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
