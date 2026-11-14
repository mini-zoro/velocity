//! Small independent grammars exercise semantics which corpus totals can hide.
use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};
use vel_syntax::{ParseState, ScopeStack, SyntaxDefinition, SyntaxSet, SyntaxSetBuilder};
fn set(yaml: &str) -> Arc<SyntaxSet> {
    let mut b = SyntaxSetBuilder::new();
    b.add_with_path(
        SyntaxDefinition::load_from_str(yaml, true, None).unwrap(),
        "Packages/Fixture/Fixture.sublime-syntax",
    );
    Arc::new(b.build())
}
fn at(set: &Arc<SyntaxSet>, line: &str, column: usize) -> String {
    let syntax = &set.syntaxes()[0];
    let mut state = ParseState::new(set.clone(), syntax);
    let output = state.parse_line(line).unwrap();
    let mut stack = ScopeStack::new();
    for (at, op) in output.ops {
        if at > column {
            break;
        }
        stack.apply(&op).unwrap();
    }
    stack
        .as_slice()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}
#[test]
fn dynamic_escape_is_escaped_and_clones_converge() {
    let set = set(
        "name: Fixture\nscope: source.fixture\nversion: 2\ncontexts:\n  main:\n    - match: 'BEGIN (.+)$'\n      push:\n        - meta_scope: string.fixture\n        - match: '^\\1$'\n          pop: true\n",
    );
    let mut state = ParseState::new(set.clone(), &set.syntaxes()[0]);
    state.parse_line("BEGIN a.b\n").unwrap();
    let before = state.clone();
    state.parse_line("axb\n").unwrap();
    assert_eq!(state, before, "capture dots must be literal");
    state.parse_line("a.b\n").unwrap();
    assert_ne!(state, before);
    let mut initial = ParseState::new(set.clone(), &set.syntaxes()[0]);
    initial.parse_line("other\n").unwrap();
    assert_eq!(
        state, initial,
        "absolute line counters must not prevent convergence"
    );
    fn hash(s: &ParseState) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        s.hash(&mut h);
        h.finish()
    }
    assert_eq!(hash(&state), hash(&initial));
}
#[test]
fn cross_line_branch_returns_rewind_and_corrected_ops() {
    let set = set(
        "name: Fixture\nscope: source.fixture\nversion: 2\ncontexts:\n  main:\n    - match: '(?=a)'\n      branch_point: reading\n      branch: [first, second]\n  first:\n    - meta_scope: first.fixture\n    - match: z\n      fail: reading\n  second:\n    - meta_scope: second.fixture\n    - match: z\n      pop: true\n",
    );
    let mut state = ParseState::new(set.clone(), &set.syntaxes()[0]);
    state.parse_line("a\n").unwrap();
    assert!(state.is_speculative());
    let output = state.parse_line("z\n").unwrap();
    assert_eq!(output.rewind.unwrap().from_line, 0);
    assert!(output.corrections.iter().any(|(n, ops)| {
        *n == 0
            && ops
                .iter()
                .any(|(_, op)| format!("{op:?}").contains("second.fixture"))
    }));
}
#[test]
fn broken_regex_is_local_and_zero_width_set_cycle_terminates() {
    let set = set(
        "name: Fixture\nscope: source.fixture\ncontexts:\n  main:\n    - match: '('\n      scope: broken.fixture\n    - match: x\n      scope: good.fixture\n",
    );
    assert!(at(&set, "x\n", 0).contains("good.fixture"));
    let cyclic = super_set_cycle();
    let mut state = ParseState::new(cyclic.clone(), &cyclic.syntaxes()[0]);
    let result = state.parse_line("中\n").unwrap();
    assert!(!result.warnings.is_empty());
}
fn super_set_cycle() -> Arc<SyntaxSet> {
    set(
        "name: Fixture\nscope: source.fixture\ncontexts:\n  main:\n    - match: ''\n      set: second\n  second:\n    - match: ''\n      set: main\n",
    )
}
#[test]
fn recursive_variables_are_recoverable() {
    let error=SyntaxDefinition::load_from_str("name: Bad\nscope: source.bad\nvariables:\n  a: '{{b}}'\n  b: '{{a}}'\ncontexts:\n  main: []\n",true,None).unwrap_err();
    assert!(error.to_string().contains("recursive syntax variable"));
}
#[test]
fn pure_assertions_unicode_and_past_eol_columns() {
    let set = set(
        "name: Fixture\nscope: source.fixture\ncontexts:\n  main:\n    - match: abc\n      scope: keyword.fixture\n    - match: '//.*'\n      scope: comment.fixture\n",
    );
    let text = "// SYNTAX TEST \"Packages/Fixture/Fixture.sublime-syntax\"\n   abc 中\n// ^^^ keyword.fixture\n// <- source.fixture\n//     ^ -comment\nx // ^ this is source, not an assertion\n//              ^ comment.fixture\n";
    let result = vel_syntax::syntest::evaluate(&set, text, "fixture");
    assert_eq!(result.counts.passed, 4, "{result:?}");
    assert_eq!(result.counts.failed, 0, "{result:?}");
}
#[test]
fn content_cache_invalidates_and_recovers_from_corruption() {
    let root =
        std::env::temp_dir().join(format!("velocity-syntax-cache-test-{}", std::process::id()));
    let sources = root.join("src");
    let cache = root.join("cache");
    std::fs::create_dir_all(&sources).unwrap();
    let path = sources.join("Fixture.sublime-syntax");
    std::fs::write(
        &path,
        "name: Before\nscope: source.before\ncontexts: {main: []}\n",
    )
    .unwrap();
    let (_, status) = SyntaxSet::load_cached(&sources, &cache).unwrap();
    assert_eq!(status, vel_syntax::CacheStatus::Yaml);
    let (_, status) = SyntaxSet::load_cached(&sources, &cache).unwrap();
    assert_eq!(status, vel_syntax::CacheStatus::Hit);
    for entry in std::fs::read_dir(&cache).unwrap() {
        std::fs::write(entry.unwrap().path(), b"corrupt").unwrap();
    }
    let (_, status) = SyntaxSet::load_cached(&sources, &cache).unwrap();
    assert_eq!(status, vel_syntax::CacheStatus::Yaml);
    std::fs::write(
        &path,
        "name: After!\nscope: source.after!\ncontexts: {main: []}\n",
    )
    .unwrap();
    let (loaded, status) = SyntaxSet::load_cached(&sources, &cache).unwrap();
    assert_eq!(status, vel_syntax::CacheStatus::Yaml);
    assert!(loaded.find_syntax_by_name("After!").is_some());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn matcher_factory_is_used_by_the_interpreter() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use vel_regex::{MatcherBackend, OnigBackend, PatternSet, PatternSpec};
    #[derive(Debug)]
    struct Counting(Arc<AtomicUsize>);
    impl MatcherBackend for Counting {
        fn build(&self, patterns: Vec<PatternSpec>) -> PatternSet {
            self.0.fetch_add(1, Ordering::Relaxed);
            OnigBackend::default().build(patterns)
        }
    }
    let original = set(
        "name: Fixture\nscope: source.fixture\ncontexts:\n  main:\n    - match: x\n      scope: winner.fixture\n",
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let changed = Arc::new(
        (*original)
            .clone()
            .with_matcher_backend(Arc::new(Counting(calls.clone()))),
    );
    assert!(at(&changed, "x\n", 0).contains("winner.fixture"));
    assert!(calls.load(Ordering::Relaxed) > 0);
}

#[test]
fn inherited_variables_expand_first_line_patterns_after_merging() {
    let parent=SyntaxDefinition::load_from_str("name: Parent\nscope: source.parent\nversion: 2\nvariables:\n  executable: parent\ncontexts:\n  main: []\n",true,None).unwrap();
    let child=SyntaxDefinition::load_from_str("name: Child\nscope: source.child\nversion: 2\nextends: Packages/Fixture/Parent.sublime-syntax\nfirst_line_match: '^#!{{executable}}$'\n",true,None).unwrap();
    let mut builder = SyntaxSetBuilder::new();
    builder.add_with_path(parent, "Packages/Fixture/Parent.sublime-syntax");
    builder.add_with_path(child, "Packages/Fixture/Child.sublime-syntax");
    let set = builder.build();
    assert_eq!(
        set.find_syntax_by_first_line("#!parent").unwrap().name,
        "Child"
    );
    assert!(set.find_syntax_by_first_line("#!other").is_none());
}

#[test]
fn external_embed_is_resolved_only_when_entered() {
    let host=SyntaxDefinition::load_from_str("name: Host\nscope: source.host\nversion: 2\ncontexts:\n  main:\n    - match: '<'\n      embed: Packages/Fixture/Guest.sublime-syntax\n      embed_scope: source.embedded.host\n      escape: '>'\n",true,None).unwrap();
    let guest=SyntaxDefinition::load_from_str("name: Display Name Differs\nscope: source.guest\nversion: 2\ncontexts:\n  main:\n    - match: x\n      scope: keyword.guest\n",true,None).unwrap();
    let mut builder = SyntaxSetBuilder::new();
    builder.add_with_path(host, "Packages/Fixture/Host.sublime-syntax");
    builder.add_with_path(guest, "Packages/Fixture/Guest.sublime-syntax");
    let set = Arc::new(builder.build());
    assert!(!set.syntaxes()[1].contexts_are_loaded());
    let mut state = ParseState::new(set.clone(), &set.syntaxes()[0]);
    state.parse_line("ordinary text\n").unwrap();
    assert!(!set.syntaxes()[1].contexts_are_loaded());
    assert!(at(&set, "<x>\n", 1).contains("source.embedded.host keyword.guest"));
    assert!(set.syntaxes()[1].contexts_are_loaded());
    assert!(set.find_unlinked_contexts().is_empty());
}

#[test]
fn branch_retry_keeps_trigger_capture_bindings() {
    let set = set(
        "name: Fixture\nscope: source.fixture\nversion: 2\ncontexts:\n  main:\n    - match: '(a)'\n      branch_point: reading\n      branch: [first, second]\n  first:\n    - match: z\n      fail: reading\n  second:\n    - match: '\\1'\n      scope: captured.fixture\n      pop: true\n    - match: z\n      pop: true\n",
    );
    let actual = at(&set, "aaz\n", 1);
    assert!(actual.contains("captured.fixture"), "{actual}");
}
#[test]
fn dynamic_capture_whitespace_and_multi_digit_groups_are_literal() {
    let set = set(
        "name: Fixture\nscope: source.fixture\ncontexts:\n  main:\n    - match: 'BEGIN (.+)$'\n      push:\n        - match: '(?x)^\\1$'\n          pop: true\n    - match: '(a)()()()()()()()()(z)'\n      push:\n        - match: '^\\10$'\n          pop: true\n",
    );
    let mut state = ParseState::new(set.clone(), &set.syntaxes()[0]);
    state.parse_line("BEGIN a b\n").unwrap();
    let before = state.clone();
    state.parse_line("ab\n").unwrap();
    assert_eq!(state, before);
    state.parse_line("a b\n").unwrap();
    assert_ne!(state, before);
    let settled = state.clone();
    state.parse_line("az\n").unwrap();
    assert_ne!(state, settled);
    state.parse_line("z\n").unwrap();
    assert_eq!(state, settled);
}
