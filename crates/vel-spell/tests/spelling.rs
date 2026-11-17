use std::{sync::Arc, time::Duration};
use vel_buffer::{Change, ChangeId, ChangeSet};
use vel_color_scheme::ColorScheme;
use vel_highlight::{DocumentHighlighter, ScopeSnapshot};
use vel_scope::Selector;
use vel_spell::{DocumentSpelling, SpellChecker};
use vel_syntax::{SyntaxDefinition, SyntaxSetBuilder};
use vel_text::Text;
fn scopes(text: Text, version: u64) -> ScopeSnapshot {
    let mut builder = SyntaxSetBuilder::new();
    builder.add(SyntaxDefinition::load_from_str("name: Fixture\nscope: source.fixture\ncontexts:\n  main:\n    - match: '//.*'\n      scope: comment.line.fixture\n    - match: '\"[^\"]*\"'\n      scope: string.quoted.fixture\n",true,None).unwrap());
    let set = Arc::new(builder.build());
    let mut highlighter = DocumentHighlighter::new(
        text.clone(),
        ChangeId(version),
        set.clone(),
        &set.syntaxes()[0],
        Arc::new(ColorScheme::velocity_dark()),
    );
    highlighter.finish().unwrap();
    highlighter
        .request_lines(ChangeId(version), 0..text.len_lines(), Duration::ZERO)
        .unwrap()
        .scopes
}
#[test]
fn actual_english_affixes_suggestions_user_words_and_tokenization() {
    let mut checker = SpellChecker::english().unwrap();
    for word in [
        "hello", "world", "running", "dogs", "isn't", "Hello", "HELLO",
    ] {
        assert!(checker.check(word), "{word}");
    }
    assert!(!checker.check("mispelled"));
    assert!(
        checker
            .suggestions("mispelled", 10)
            .iter()
            .any(|s| s == "misspelled")
    );
    let text = Text::from(
        "hello badSpeling spell_chek HTTPServer WRNOG user42 https://zzzz.invalid/baaad www.zzz.invalid foo@zz.invalid isn't",
    );
    let errors = checker
        .check_words(&text, std::slice::from_ref(&(0..text.len_bytes())))
        .unwrap();
    assert_eq!(
        errors.iter().map(|e| e.word.as_str()).collect::<Vec<_>>(),
        ["Speling", "chek", "WRNOG"]
    );
    checker.set_ignored_words(["speling", "http"]);
    checker.add_word("chek");
    checker.add_word("wrnog");
    assert!(
        checker
            .check_words(&text, std::slice::from_ref(&(0..text.len_bytes())))
            .unwrap()
            .is_empty()
    );
    assert!(
        checker
            .check_words(&Text::from("世"), std::slice::from_ref(&(1..2)))
            .is_err()
    );
}
#[test]
fn scope_filters_and_incremental_cache_track_exact_lines() {
    let mut checker = SpellChecker::english().unwrap();
    let mut text = Text::from("mispelled code\n// mispelled word\n\"speling\"\n");
    let selector = Selector::parse("comment, string").unwrap();
    let initial = scopes(text.clone(), 0);
    let mut document = DocumentSpelling::new(text.clone(), ChangeId(0), selector.clone());
    let expected = checker.check_scoped(&initial, &selector).unwrap();
    assert_eq!(
        expected.iter().map(|e| e.word.as_str()).collect::<Vec<_>>(),
        ["mispelled", "speling"]
    );
    assert_eq!(
        document.check_lines(&checker, &initial, 0..4).unwrap(),
        expected
    );
    assert_eq!(document.checked_lines(), 4);
    document.check_lines(&checker, &initial, 0..4).unwrap();
    assert_eq!(document.checked_lines(), 0);
    let at = text.line_to_byte(1) + 3;
    text.replace(at..at + 9, "misspelled");
    let changes = ChangeSet::new(vec![Change {
        old_range: at..at + 9,
        new_len: 10,
    }]);
    document
        .apply_change(text.clone(), ChangeId(1), &changes)
        .unwrap();
    assert!(document.check_lines(&checker, &initial, 0..4).is_err());
    let updated = scopes(text.clone(), 1);
    let errors = document.check_lines(&checker, &updated, 0..4).unwrap();
    assert_eq!(errors.len(), 1);
    assert_eq!(document.checked_lines(), 1);
    checker.add_word("speling");
    assert!(
        document
            .check_lines(&checker, &updated, 0..4)
            .unwrap()
            .is_empty()
    );
    assert_eq!(document.checked_lines(), 4);
    let at = text.line_to_byte(1);
    text.insert(at, "// new line\n");
    document
        .apply_change(
            text.clone(),
            ChangeId(2),
            &ChangeSet::new(vec![Change {
                old_range: at..at,
                new_len: 12,
            }]),
        )
        .unwrap();
    let updated = scopes(text, 2);
    assert!(
        document
            .check_lines(&checker, &updated, 0..5)
            .unwrap()
            .is_empty()
    );
    assert_eq!(document.checked_lines(), 2);
    let mut unavailable = updated.clone();
    for line in &mut unavailable.lines {
        line.exact = false;
    }
    assert!(
        checker
            .check_scoped(&unavailable, &selector)
            .unwrap()
            .is_empty()
    );
}
#[test]
fn scope_edges_never_create_partial_words() {
    let checker = SpellChecker::english().unwrap();
    let text = Text::from("misspelled");
    assert!(
        checker
            .check_words(&text, std::slice::from_ref(&(2..5)))
            .unwrap()
            .is_empty()
    );
}
#[test]
fn package_pairs_load_and_cannot_escape_packages() {
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("User");
    std::fs::create_dir(&user).unwrap();
    std::fs::write(user.join("tiny.aff"), "SET UTF-8\n").unwrap();
    std::fs::write(user.join("tiny.dic"), "1\nvelocity\n").unwrap();
    let dictionaries = SpellChecker::package_dictionaries(dir.path()).unwrap();
    assert_eq!(dictionaries.len(), 1);
    let checker = SpellChecker::from_package(dir.path(), "Packages/User/tiny.dic").unwrap();
    assert!(checker.check("velocity"));
    assert!(SpellChecker::from_package(dir.path(), "Packages/User/../../outside.dic").is_err());
}

#[test]
fn switching_user_lexicons_invalidates_cached_results() {
    let text = Text::from("// speling mispelled\n");
    let scopes = scopes(text.clone(), 0);
    let mut first = SpellChecker::english().unwrap();
    first.set_ignored_words(["speling"]);
    let mut second = SpellChecker::english().unwrap();
    second.set_ignored_words(["mispelled"]);
    let mut document =
        DocumentSpelling::new(text, ChangeId(0), Selector::parse("comment").unwrap());
    assert_eq!(
        document.check_lines(&first, &scopes, 0..1).unwrap()[0].word,
        "mispelled"
    );
    assert_eq!(
        document.check_lines(&second, &scopes, 0..1).unwrap()[0].word,
        "speling"
    );
}
#[test]
fn system_dictionary_discovery_loading_and_directory_boundary() {
    let root = std::env::temp_dir().join(format!(
        "velocity-r16-spelling-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("review.aff"), "SET UTF-8\n").unwrap();
    std::fs::write(root.join("review.dic"), "1\nvelocity\n").unwrap();
    std::fs::write(root.join("orphan.dic"), "1\norphan\n").unwrap();
    assert_eq!(
        SpellChecker::system_dictionaries(&root).unwrap(),
        [root.join("review.dic")]
    );
    assert!(
        SpellChecker::system_dictionaries(&root.join("absent"))
            .unwrap()
            .is_empty()
    );
    let checker = SpellChecker::from_system_directory(&root, &root.join("review.dic")).unwrap();
    assert!(checker.check("velocity"));
    assert!(!checker.check("mispelled"));
    assert!(
        SpellChecker::from_system_directory(&root.join("absent"), &root.join("review.dic"))
            .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn optional_system_directory_skips_missing_broken_and_non_directory_entries() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Spelling");
    assert!(SpellChecker::system_dictionaries(&root).unwrap().is_empty());
    std::fs::write(&root, "not a directory").unwrap();
    assert!(SpellChecker::system_dictionaries(&root).unwrap().is_empty());
    std::fs::remove_file(&root).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("broken.dic"), "1\nword\n").unwrap();
    std::fs::create_dir(root.join("directory.dic")).unwrap();
    std::fs::write(root.join("valid.aff"), "SET UTF-8\n").unwrap();
    std::fs::write(root.join("valid.dic"), "1\nword\n").unwrap();
    assert_eq!(
        SpellChecker::system_dictionaries(&root).unwrap(),
        [root.join("valid.dic")]
    );
}
