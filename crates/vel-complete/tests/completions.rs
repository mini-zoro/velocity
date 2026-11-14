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
    snapshot.settings.auto_complete_preserve_order = true;
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
