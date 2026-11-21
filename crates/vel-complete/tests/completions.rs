use std::{
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};
use vel_buffer::ChangeId;
use vel_complete::{
    BufferWords, CompletionEngine, CompletionFormat, CompletionService, Invocation, KindId,
    ViewSnapshot,
};
use vel_highlight::ScopeProvider;
use vel_index::{FileAnalysis, Index, WordStatistics};
use vel_scope::Scope;
use vel_snippet::SnippetFile;
use vel_text::Text;
struct Scopes {
    version: ChangeId,
    scopes: Vec<Scope>,
}
impl ScopeProvider for Scopes {
    fn version(&self) -> ChangeId {
        self.version
    }
    fn scopes_at(&self, _: usize) -> Option<&[Scope]> {
        Some(&self.scopes)
    }
}
fn scopes(version: u64) -> Arc<Scopes> {
    Arc::new(Scopes {
        version: ChangeId(version),
        scopes: vec![Scope::new("source.fixture").unwrap()],
    })
}
fn view(text: &str) -> ViewSnapshot {
    ViewSnapshot::new(1, ChangeId(7), Text::from(text))
}
#[test]
fn r36_tab_delimits_annotation_not_inserted_text() {
    let mut engine = CompletionEngine::new();
    engine
        .add_completions(
            r#"{
      "scope":"source.fixture",
      "completions":["example\tlegacy", {"trigger":"exampleRich\tlegacy","contents":"inserted"}]
    }"#,
        )
        .unwrap();
    let list = engine.query(&view("exa"), &[3], &*scopes(7));
    for (trigger, inserted) in [("example", "example"), ("exampleRich", "inserted")] {
        let item = list
            .items
            .iter()
            .find(|item| item.trigger == trigger)
            .unwrap();
        assert_eq!(item.annotation, "legacy");
        assert_eq!(item.completion, inserted);
    }
}
#[test]
fn all_sources_ranking_exact_snippets_rich_items_and_scope_filtering() {
    let mut engine = CompletionEngine::new();
    engine.add_completions(r#"{ // resource comment
      "scope":"source.fixture", "completions":["prefixOther",{"trigger":"prefixRich\tlegacy","contents":"${1:rich}","annotation":"shown","kind":["function","m","Method"],"details":"<b>details</b>"},],
    }"#).unwrap();
    engine
        .add_completions(r#"{"scope":"text.html","completions":["prefixHidden"]}"#)
        .unwrap();
    engine.add_snippet(SnippetFile::parse("<snippet><content><![CDATA[insert ${1:field}$0]]></content><tabTrigger>pre</tabTrigger><scope>source.fixture</scope></snippet>").unwrap()).unwrap();
    engine.set_open_buffer(2, Arc::new(BufferWords::new(&Text::from("prefixOpen"))));
    let mut index = Index::new();
    index.replace_file(
        "a.fix",
        FileAnalysis {
            symbols: Vec::new(),
            words: WordStatistics::from_text("prefixProject prefixProject"),
        },
    );
    engine.set_index(Arc::new(RwLock::new(index)));
    let list = engine.query(&view("prefixLocal pre"), &[15], &*scopes(7));
    assert_eq!(list.items[0].trigger, "pre");
    assert_eq!(list.items[0].completion_format, CompletionFormat::Snippet);
    for expected in [
        "prefixOther",
        "prefixRich",
        "prefixOpen",
        "prefixLocal",
        "prefixProject",
    ] {
        assert!(
            list.items.iter().any(|item| item.trigger == expected),
            "{expected}: {:?}",
            list.items
        );
    }
    assert!(!list.items.iter().any(|item| item.trigger == "prefixHidden"));
    let rich = list
        .items
        .iter()
        .find(|item| item.trigger == "prefixRich")
        .unwrap();
    assert_eq!(rich.annotation, "shown");
    assert_eq!(rich.kind.category, KindId::Function);
    assert_eq!(rich.kind.letter, "m");
    assert_eq!(rich.details, "<b>details</b>");
    assert!(
        engine
            .query(&view("pre"), &[3], &*scopes(6))
            .items
            .is_empty()
    );
}
#[test]
fn triggers_settings_history_preserve_order_and_multicarets() {
    let mut engine = CompletionEngine::new();
    engine
        .add_completions(r#"{"completions":["abcTwo","abcOne"]}"#)
        .unwrap();
    let mut snapshot = view("abc abc");
    let scope = scopes(7);
    let mut list = engine.query(&snapshot, &[3, 7], &*scope);
    assert_eq!(list.replace_ranges, vec![0..3, 4..7]);
    let item = list
        .items
        .iter()
        .find(|item| item.trigger == "abcOne")
        .unwrap();
    engine.record_commit(item);
    list = engine.query(&snapshot, &[3, 7], &*scope);
    assert_eq!(list.items[0].trigger, "abcOne");
    snapshot.settings.auto_complete_use_history = false;
    snapshot.settings.auto_complete_preserve_order = vel_complete::PreserveOrder::Strict;
    assert_eq!(
        engine.query(&snapshot, &[3], &*scope).items[0].trigger,
        "abcTwo"
    );
    snapshot.invocation = Invocation::Typing {
        character: 'c',
        idle: Duration::ZERO,
    };
    assert!(engine.query(&snapshot, &[3], &*scope).items.is_empty());
    snapshot.invocation = Invocation::Typing {
        character: 'c',
        idle: Duration::from_secs(1),
    };
    snapshot.settings.auto_complete = false;
    assert!(engine.query(&snapshot, &[3], &*scope).items.is_empty());
    snapshot.settings.auto_complete = true;
    snapshot.settings.auto_complete_size_limit = 1;
    assert!(engine.query(&snapshot, &[3], &*scope).items.is_empty());
    snapshot.settings.auto_complete_size_limit = usize::MAX;
    snapshot.settings.auto_complete_commit_on_tab = true;
    snapshot.fields_active = true;
    assert!(engine.query(&snapshot, &[3], &*scope).items.is_empty());
    snapshot.settings.auto_complete_with_fields = true;
    assert!(!engine.query(&snapshot, &[3], &*scope).items.is_empty());
    snapshot.settings.auto_complete_selector = "text.html".into();
    assert!(engine.query(&snapshot, &[3], &*scope).items.is_empty());
    snapshot
        .settings
        .auto_complete_triggers
        .push(vel_complete::CompletionTrigger {
            selector: "source".into(),
            characters: "c".into(),
            rhs_empty: false,
        });
    assert!(!engine.query(&snapshot, &[3], &*scope).items.is_empty());
}
#[test]
fn contextual_punctuation_and_source_switches() {
    let mut index = Index::new();
    index.replace_file(
        "context.fix",
        FileAnalysis {
            symbols: Vec::new(),
            words: WordStatistics::from_text(
                "let value println!(value)\nlet value println!(value)",
            ),
        },
    );
    let mut engine = CompletionEngine::new();
    engine.set_index(Arc::new(RwLock::new(index)));
    let mut snapshot = view("let value pri");
    snapshot.settings.auto_complete_trailing_symbols = true;
    let list = engine.query(&snapshot, &[13], &*scopes(7));
    assert_eq!(list.items[0].trigger, "println");
    assert_eq!(list.items[0].completion, "println!(");
    snapshot.settings.auto_complete_use_index = false;
    assert!(engine.query(&snapshot, &[13], &*scopes(7)).items.is_empty());
}
#[test]
fn async_latest_wins_and_cancel_rejects_already_queued_output() {
    let engine = CompletionEngine::new();
    engine.set_open_buffer(
        2,
        Arc::new(BufferWords::new(&Text::from("candidate one two"))),
    );
    let service = CompletionService::new(Arc::new(engine)).unwrap();
    for _ in 0..50 {
        service.query(view("c"), vec![1], scopes(7));
    }
    let generation = service.query(view("ca"), vec![2], scopes(7));
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(result) = service.poll() {
            assert_eq!(result.generation, generation);
            assert_eq!(result.items[0].trigger, "candidate");
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    service.query(view("ca"), vec![2], scopes(7));
    service.cancel();
    assert!(service.poll().is_none());
}

#[test]
fn punctuation_tab_triggers_and_existing_context_punctuation() {
    let mut engine = CompletionEngine::new();
    engine.add_snippet(SnippetFile::parse("<snippet><content><![CDATA[println!($0)]]></content><tabTrigger>print!</tabTrigger><scope>source</scope></snippet>").unwrap()).unwrap();
    let mut snapshot = view("print!");
    snapshot.invocation = Invocation::Tab;
    let list = engine.query(&snapshot, &[6], &*scopes(7));
    assert_eq!(list.items[0].trigger, "print!");
    assert_eq!(list.replace_ranges.len(), 1);
    assert_eq!(list.replace_ranges[0], 0..6);
    let mut index = Index::new();
    index.replace_file(
        "context.fix",
        FileAnalysis {
            symbols: Vec::new(),
            words: WordStatistics::from_text("let value println!(value)"),
        },
    );
    engine.set_index(Arc::new(RwLock::new(index)));
    let snapshot = view("let value pri!(already)");
    let list = engine.query(&snapshot, &[13], &*scopes(7));
    assert_eq!(
        list.items
            .iter()
            .find(|item| item.trigger == "println")
            .unwrap()
            .completion,
        "println"
    );
}
#[test]
fn multi_caret_project_intersection_is_selected_before_limiting() {
    let mut index = Index::new();
    let mut words = WordStatistics::default();
    for n in 0..100 {
        words.words.insert(format!("aaa_{n:03}"), 100);
    }
    words.words.insert("a_z_result".into(), 1);
    index.replace_file(
        "words.fix",
        FileAnalysis {
            symbols: Vec::new(),
            words,
        },
    );
    let mut engine = CompletionEngine::new();
    engine.set_index(Arc::new(RwLock::new(index)));
    let snapshot = view("a z");
    let list = engine.query(&snapshot, &[1, 3], &*scopes(7));
    assert_eq!(list.items[0].trigger, "a_z_result");
}

#[test]
fn malformed_resources_return_errors_instead_of_partial_publication_or_panics() {
    let mut engine = CompletionEngine::new();
    for invalid in [
        "/* broken é",
        r#"{"completions":[{"trigger":"x","contents":"x","kind":["function",7,"Method"]}]}"#,
        r#"{"completions":[{"trigger":"x","contents":"${1:broken"}]}"#,
    ] {
        assert!(engine.add_completions(invalid).is_err(), "{invalid}");
    }
    assert!(engine.query(&view("x"), &[1], &*scopes(7)).items.is_empty());
}

#[test]
fn explicit_trigger_selector_and_rhs_table() {
    let mut engine = CompletionEngine::new();
    engine
        .add_completions(r#"{"completions":["alpha"]}"#)
        .unwrap();
    for (selector, characters, scope, character, rhs, flag, want) in [
        (
            "punctuation.accessor",
            "",
            "punctuation.accessor",
            '.',
            " right",
            true,
            true,
        ),
        (
            "punctuation.accessor",
            "",
            "punctuation.accessor",
            '.',
            "\tright",
            true,
            true,
        ),
        ("", "<", "source.fixture", '<', "", false, true),
        ("text.html", "<", "text.html", '<', "", false, true),
        ("text.html", "<", "source.fixture", '<', "", false, false),
        (
            "punctuation.accessor",
            "",
            "punctuation.accessor",
            '.',
            "",
            true,
            true,
        ),
        (
            "punctuation.accessor",
            "",
            "punctuation.accessor",
            ':',
            "",
            true,
            true,
        ),
        (
            "punctuation.accessor",
            "",
            "punctuation.accessor",
            '.',
            "right",
            true,
            false,
        ),
        (
            "punctuation.accessor",
            "",
            "punctuation.accessor",
            '.',
            "right",
            false,
            true,
        ),
        (
            "source - comment",
            "!",
            "source.fixture",
            '!',
            "",
            false,
            true,
        ),
        (
            "source - comment",
            "!",
            "source.fixture",
            '?',
            "",
            false,
            false,
        ),
        (
            "source - comment",
            "!",
            "comment.line",
            '!',
            "",
            false,
            false,
        ),
    ] {
        let mut v = view(&format!("{character}{rhs}"));
        v.invocation = Invocation::Typing {
            character,
            idle: Duration::from_secs(1),
        };
        v.settings.auto_complete_selector = "text.never".into();
        v.settings.auto_complete_triggers = vec![vel_complete::CompletionTrigger {
            selector: selector.into(),
            characters: characters.into(),
            rhs_empty: flag,
        }];
        let scopes = Scopes {
            version: v.version,
            scopes: vec![Scope::new(scope).unwrap()],
        };
        let list = engine.query(&v, &[character.len_utf8()], &scopes);
        assert_eq!(
            !list.items.is_empty(),
            want,
            "{selector}/{characters}/{scope}/{rhs}/{flag}"
        );
    }
}
#[test]
fn inferred_suffixes_respect_both_settings_and_preserve_authored_contents() {
    let mut index = Index::new();
    index.replace_file(
        "fixture",
        FileAnalysis {
            symbols: Vec::new(),
            words: WordStatistics::from_text(
                "let value println!(value)\nlet value println!(value)",
            ),
        },
    );
    let mut engine = CompletionEngine::new();
    engine.set_index(Arc::new(RwLock::new(index)));
    for symbols in [false, true] {
        let mut v = view("let value pri");
        v.settings.auto_complete_trailing_symbols = symbols;
        let list = engine.query(&v, &[13], &*scopes(7));
        let item = list.items.iter().find(|i| i.trigger == "println").unwrap();
        assert_eq!(
            item.completion,
            if symbols { "println!(" } else { "println" }
        );
    }
    for spaces in [false, true] {
        let mut v = view("alpha beta\nalpha beta\nalpha beta\nal");
        v.settings.auto_complete_trailing_spaces = spaces;
        let list = engine.query(&v, &[35], &*scopes(7));
        let item = list.items.iter().find(|i| i.trigger == "alpha").unwrap();
        assert_eq!(item.completion, if spaces { "alpha " } else { "alpha" });
    }
    engine
        .add_completions(
            r#"{"completions":[{"trigger":"literal","contents":"literal() ","kind":"namespace"}]}"#,
        )
        .unwrap();
    let mut v = view("lit");
    v.settings.auto_complete_trailing_symbols = false;
    v.settings.auto_complete_trailing_spaces = false;
    let list = engine.query(&v, &[3], &*scopes(7));
    assert_eq!(list.items[0].completion, "literal() ");
    assert_eq!(list.items[0].kind.letter, "a");
}
#[test]
fn preserve_order_documented_values_and_history_switch() {
    let mut engine = CompletionEngine::new();
    engine
        .add_completions(r#"{"completions":["axbyc","abc"]}"#)
        .unwrap();
    for (policy, expected) in [("none", "abc"), ("some", "abc"), ("strict", "axbyc")] {
        let mut v = view("ab");
        v.settings.auto_complete_preserve_order = vel_complete::PreserveOrder::from_setting(policy);
        let list = engine.query(&v, &[2], &*scopes(7));
        assert_eq!(list.items[0].trigger, expected, "{policy}");
    }
    let mut engine = CompletionEngine::new();
    engine
        .add_completions(r#"{"completions":["abcA","abcB"]}"#)
        .unwrap();
    let mut v = view("abc");
    let list = engine.query(&v, &[3], &*scopes(7));
    engine.record_commit(list.items.iter().find(|i| i.trigger == "abcB").unwrap());
    for history in [false, true] {
        v.settings.auto_complete_use_history = history;
        let list = engine.query(&v, &[3], &*scopes(7));
        assert_eq!(list.items[0].trigger, if history { "abcB" } else { "abcA" });
    }
}

#[test]
fn automatic_snippet_inclusion_and_default_selector_table() {
    let mut engine = CompletionEngine::new();
    engine.add_snippet(SnippetFile::parse("<snippet><content>alert</content><tabTrigger>alert</tabTrigger><scope>source.fixture</scope></snippet>").unwrap()).unwrap();
    for include in [false, true] {
        let mut v = view("a");
        v.settings.auto_complete_include_snippets_when_typing = include;
        v.invocation = Invocation::Typing {
            character: 'a',
            idle: Duration::from_secs(1),
        };
        assert_eq!(
            !engine.query(&v, &[1], &*scopes(7)).items.is_empty(),
            include
        );
        v.invocation = Invocation::Manual;
        assert!(!engine.query(&v, &[1], &*scopes(7)).items.is_empty());
    }
    let mut engine = CompletionEngine::new();
    engine
        .add_completions(r#"{"completions":["alpha"]}"#)
        .unwrap();
    for (inner, want) in [
        ("string.quoted.double", true),
        ("string.quoted.single", true),
        ("string.quoted.double.block", false),
        ("string.quoted.single.block", false),
        ("string.unquoted.heredoc", false),
        ("comment.line", false),
    ] {
        let mut v = view("a");
        v.invocation = Invocation::Typing {
            character: 'a',
            idle: Duration::from_secs(1),
        };
        let stack = Scopes {
            version: v.version,
            scopes: vec![
                Scope::new("source.fixture").unwrap(),
                Scope::new(inner).unwrap(),
            ],
        };
        assert_eq!(
            !engine.query(&v, &[1], &stack).items.is_empty(),
            want,
            "{inner}"
        );
        v.settings.auto_complete_selector = "source - string - comment".into();
        assert!(engine.query(&v, &[1], &stack).items.is_empty());
    }
}
#[test]
fn partial_order_uses_likelihood_while_normal_order_uses_textual_match() {
    let mut engine = CompletionEngine::new();
    engine
        .add_completions(r#"{"completions":["abcA"]}"#)
        .unwrap();
    let mut v = view(&format!("{}abc", "abcB ".repeat(4096)));
    for (policy, want) in [
        (vel_complete::PreserveOrder::None, "abcA"),
        (vel_complete::PreserveOrder::Some, "abcB"),
    ] {
        v.settings.auto_complete_preserve_order = policy;
        assert_eq!(
            engine.query(&v, &[v.text.len_bytes()], &*scopes(7)).items[0].trigger,
            want
        );
    }
}

#[test]
fn an_inactive_trigger_adds_no_scope_lookups_to_normal_typing() {
    struct CountScopes {
        calls: std::sync::atomic::AtomicUsize,
        stack: Vec<Scope>,
    }
    impl ScopeProvider for CountScopes {
        fn version(&self) -> ChangeId {
            ChangeId(7)
        }
        fn scopes_at(&self, _: usize) -> Option<&[Scope]> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Some(&self.stack)
        }
    }
    let mut engine = CompletionEngine::new();
    engine
        .add_completions(r#"{"completions":["alpha"]}"#)
        .unwrap();
    let scope = CountScopes {
        calls: std::sync::atomic::AtomicUsize::new(0),
        stack: vec![Scope::new("source.fixture").unwrap()],
    };
    let mut v = view("a");
    v.invocation = Invocation::Typing {
        character: 'a',
        idle: Duration::from_secs(1),
    };
    v.settings.auto_complete_triggers = vec![vel_complete::CompletionTrigger {
        selector: "source".into(),
        characters: "<".into(),
        rhs_empty: true,
    }];
    assert!(!engine.query(&v, &[1], &scope).items.is_empty());
    assert_eq!(
        scope.calls.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "ordinary completion's existing scope lookup is reused"
    );
}

#[test]
#[ignore = "worker timing at 2000 words × 2000 carets; run explicitly"]
fn many_word_many_caret_suffix_query_timing() {
    let mut source: String = (0..2000)
        .map(|i| format!("alpha_{i:04} value\n").repeat(3))
        .collect();
    let mut carets = Vec::new();
    for _ in 0..2000 {
        source.push_str("al\n");
        carets.push(source.len() - 1);
    }
    let mut v = view(&source);
    let engine = CompletionEngine::new();
    for spaces in [false, true] {
        v.settings.auto_complete_trailing_spaces = spaces;
        let begin = std::time::Instant::now();
        let list = engine.query(&v, &carets, &*scopes(7));
        let ms = begin.elapsed().as_secs_f64() * 1000.;
        assert_eq!(list.items.len(), v.settings.max_results);
        assert!(
            list.items
                .iter()
                .all(|item| item.completion.ends_with(' ') == spaces)
        );
        println!("R18_SUFFIX_TIMING spaces={spaces} words=2000 carets=2000 query_ms={ms:.3}");
    }
}
