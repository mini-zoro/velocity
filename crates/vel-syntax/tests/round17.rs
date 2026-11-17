//! Headless regressions through the same resource/cache loader used by the editor.
use std::sync::Arc;
use vel_syntax::{ParseState, ScopeStack, SyntaxSet};

fn stacks(set: Arc<SyntaxSet>, syntax: &str, text: &str) -> (Vec<String>, Vec<String>) {
    let mut state = ParseState::new(set.clone(), set.find_syntax_by_name(syntax).unwrap());
    let mut stack = ScopeStack::new();
    let mut result = Vec::new();
    let mut warnings = Vec::new();
    for line in text.split_inclusive('\n') {
        let output = state.parse_line(line).unwrap();
        warnings.extend(output.warnings);
        let mut ops = output.ops.into_iter().peekable();
        for at in 0..line.len() {
            while ops.peek().is_some_and(|(byte, _)| *byte <= at) {
                stack.apply(&ops.next().unwrap().1).unwrap();
            }
            result.push(
                stack
                    .as_slice()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
        for (_, op) in ops {
            stack.apply(&op).unwrap();
        }
    }
    (result, warnings)
}

#[test]
fn extends_keeps_parent_child_embeds_anonymous_contexts_and_prototypes_distinct() {
    let parent = r#"
name: Parent
scope: source.parent
version: 2
contexts:
  main:
    - match: p
      push:
        - meta_scope: meta.parent
        - match: '!'
          pop: true
      with_prototype:
        - match: Q
          scope: keyword.parent
    - match: '\['
      embed: scope:source.inner
      embed_scope: meta.parent.embed
      escape: '\]'
"#;
    let child = r#"
name: Child
scope: source.child
version: 2
extends: Packages/Fixture/Parent.sublime-syntax
contexts:
  main:
    - meta_prepend: true
    - match: c
      push:
        - meta_scope: meta.child
        - match: '!'
          pop: true
      with_prototype:
        - match: Q
          scope: keyword.child
    - match: '\{'
      embed: scope:source.inner
      embed_scope: meta.child.embed
      escape: '\}'
"#;
    let set = Arc::new(
        SyntaxSet::load_roots(
            &[],
            &[
                ("Fixture/Parent.sublime-syntax", parent),
                ("Fixture/Child.sublime-syntax", child),
                (
                    "Fixture/Inner.sublime-syntax",
                    "name: Inner\nscope: source.inner\ncontexts: {main: []}",
                ),
                ("Broken/Bad.sublime-syntax", "scope: 9\ncontexts: []"),
            ],
        )
        .unwrap(),
    );
    assert!(
        set.warnings()
            .iter()
            .any(|w| w.contains("Bad.sublime-syntax"))
    );
    let (actual, _) = stacks(set, "Child", "pQ! cQ! [x] {x}\n");
    assert!(
        actual[1].contains("meta.parent keyword.parent"),
        "{}",
        actual[1]
    );
    assert!(!actual[1].contains("meta.child"));
    assert!(
        actual[5].contains("meta.child keyword.child"),
        "{}",
        actual[5]
    );
    assert!(actual[9].contains("meta.parent.embed"), "{}", actual[9]);
    assert!(actual[13].contains("meta.child.embed"), "{}", actual[13]);
}

#[test]
fn deep_pushes_keep_closers_and_non_pushing_rules_at_the_limit() {
    let source = r#"
name: Deep
scope: source.deep
contexts:
  main:
    - match: '\['
      push: nested
    - match: Z
      scope: keyword.done
  nested:
    - meta_scope: meta.array
    - match: '\['
      push: nested
    - match: '\]'
      pop: true
    - match: x
      scope: constant.inside
"#;
    for depth in [99, 900, 1200] {
        let set = Arc::new(
            SyntaxSet::load_roots(&[], &[("Fixture/Deep.sublime-syntax", source)]).unwrap(),
        );
        let text = format!("{}x{}Z\n", "[".repeat(depth), "]".repeat(depth));
        let (actual, warnings) = stacks(set, "Deep", &text);
        assert!(actual[depth].contains("constant.inside"));
        assert_eq!(actual[text.len() - 2], "source.deep keyword.done");
        assert_eq!(warnings.is_empty(), depth < 1024);
    }
}

#[test]
fn unresolved_references_are_warned_at_link_time_and_escape_recovers() {
    let source = r#"
name: Missing
scope: source.missing
contexts:
  main:
    - include: absent_include
    - match: '\['
      embed: scope:source.absent
      escape: '\]'
    - match: X
      push: absent_push
    - match: Y
      set: absent_set
    - match: Z
      scope: keyword.done
"#;
    let set = Arc::new(
        SyntaxSet::load_roots(&[], &[("Fixture/Missing.sublime-syntax", source)]).unwrap(),
    );
    for name in [
        "absent_include",
        "source.absent",
        "absent_push",
        "absent_set",
    ] {
        assert!(
            set.warnings().iter().any(|w| w.contains(name)),
            "{name}: {:?}",
            set.warnings()
        );
    }
    let (actual, _) = stacks(set.clone(), "Missing", "[inside]\nZ\n");
    assert_eq!(actual[9], "source.missing keyword.done");
    stacks(set.clone(), "Missing", "X rest\nnext line\n");
    stacks(set, "Missing", "Y rest\nnext line\n");
}

#[test]
fn malformed_syntax_table_is_isolated_and_cached_without_panics() {
    let snippets = [
        "push: [[]]",
        "push: [[[]]]",
        "push: [1]",
        "push: {}",
        "set: false",
        "embed: []\n      escape: 'x'",
        "embed: x",
        "embed: x\n      escape: []",
        "pop: -1",
        "clear_scopes: -1",
        "with_prototype: false",
        "captures: []",
        "scope: [bad]",
        "branch: {}",
        "branch: [x]",
        "match: []",
    ];
    let cache = std::env::temp_dir().join(format!("vel-r17-malformed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache);
    let mut files = vec![(
        "Good/Good.sublime-syntax".to_string(),
        "name: Good\nscope: source.good\ncontexts: {main: []}".to_string(),
    )];
    for (i, snippet) in snippets.iter().enumerate() {
        files.push((format!("Broken/Bad{i}.sublime-syntax"),format!("name: Bad{i}\nscope: source.bad\ncontexts:\n  main:\n    - match: x\n      {snippet}\n")));
    }
    let root = cache.join("source");
    for (path, source) in &files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    for expected in [vel_syntax::CacheStatus::Yaml, vel_syntax::CacheStatus::Hit] {
        let (set, status) = SyntaxSet::load_cached_roots(&[&root], &[], &cache).unwrap();
        assert_eq!(status, expected);
        assert!(set.find_syntax_by_name("Good").is_some());
        // Structural errors still isolate their file; optional mistyped keys now
        // leave their syntax usable, with a warning for the discarded key only.
        assert_eq!(set.syntaxes().len(), 10, "{:?}", set.warnings());
        assert_eq!(set.warnings().len(), snippets.len());
        for index in [3, 4, 5, 8, 9, 10, 11, 12, 13] {
            let name = format!("Bad{index}");
            assert!(set.find_syntax_by_name(&name).is_some(), "{name}");
        }
    }
    std::fs::remove_dir_all(cache).unwrap();
}

#[test]
fn authoring_diagnostics_see_final_inherited_variables_and_leave_regexes_lazy() {
    let parent = "name: Parent\nscope: source.parent\nvariables: {parent: z}\ncontexts: {main: []}";
    let child = r#"
name: Child
scope: source.child
extends: Packages/Fixture/Parent.sublime-syntax
variables: {indirect: '{{unknown_indirect}}'}
contexts:
  main:
    - match: '{{parent}}'
    - match: '{{missing}}'
    - match: '('
    - match: '['
    - match: '*'
    - match: '[z-a]'
    - match: 'a{3,2}'
    - match: Z
      scope: keyword.good
"#;
    let set = Arc::new(
        SyntaxSet::load_roots(
            &[],
            &[
                ("Fixture/Parent.sublime-syntax", parent),
                ("Fixture/Child.sublime-syntax", child),
            ],
        )
        .unwrap(),
    );
    for expected in ["unknown_indirect", "missing", "invalid regex"] {
        assert!(
            set.warnings().iter().any(|w| w.contains(expected)),
            "{:?}",
            set.warnings()
        );
    }
    assert!(
        !set.warnings()
            .iter()
            .any(|w| w.contains("unknown variable {{parent}}"))
    );
    assert!(set.syntaxes().iter().all(|s| !s.contexts_are_loaded()));
}

#[test]
fn clear_scopes_multi_pop_and_set_agree_with_a_frame_stack_model() {
    for version in [1, 2] {
        for clear in [1, 2, 3] {
            for pops in [1, 2, 3] {
                let source = format!(
                    r#"
name: Model
scope: source.model
version: {version}
contexts:
  main:
    - meta_scope: meta.main
    - match: a
      push: a
  a:
    - meta_scope: meta.a
    - match: b
      push: b
  b:
    - meta_scope: meta.b
    - match: c
      push: c
  c:
    - clear_scopes: {clear}
    - meta_scope: meta.c
    - match: s
      pop: {pops}
      set: e
    - match: p
      pop: {pops}
  e:
    - meta_scope: meta.e
"#
                );
                let set = Arc::new(
                    SyntaxSet::load_roots(&[], &[("Fixture/Model.sublime-syntax", &source)])
                        .unwrap(),
                );
                // Model: pop c (its own scopes, restore its clear), then lower frames in
                // LIFO order, then push e. The number cleared cannot change the survivor.
                let mut expected = vec!["source.model", "meta.main", "meta.a", "meta.b"];
                expected.truncate(expected.len() - (pops - 1));
                for (op, set_target) in [('s', true), ('p', false)] {
                    let (actual, _) = stacks(set.clone(), "Model", &format!("abc{op}X\n"));
                    let mut result = expected.clone();
                    if set_target {
                        result.push("meta.e");
                    }
                    assert_eq!(
                        actual[4],
                        result.join(" "),
                        "v{version} clear={clear} pop={pops} {op}"
                    );
                }
            }
        }
    }
}

#[test]
fn compatible_yaml_scalars_load_and_only_discarded_items_warn() {
    let fixtures = [
        (
            "Quoted",
            "scope: source.quoted\ncontexts: {main: [{match: '(x)', captures: {'1': keyword.capture}}]}",
            "source.quoted keyword.capture",
            vec![],
        ),
        (
            "Empty",
            "scope:\ncontexts: {main: [{match: x, scope: keyword.capture}]}",
            "keyword.capture",
            vec![],
        ),
        (
            "Extensions",
            "scope: source.ext\nfile_extensions: [man, 1, 2, 3]\ncontexts: {main: [{match: x, scope: keyword.capture}]}",
            "source.ext keyword.capture",
            vec!["man", "1", "2", "3"],
        ),
        (
            "Partial",
            "scope: source.partial\nfile_extensions: [1, {}]\ncontexts: {main: [{match: '(x)', captures: {'1': keyword.capture, nope: invalid}, pop: -1}]}",
            "source.partial keyword.capture",
            vec!["1"],
        ),
    ];
    let cache = std::env::temp_dir().join(format!("vel-r17-scalars-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache);
    std::fs::create_dir_all(&cache).unwrap();
    let files: Vec<_> = fixtures
        .iter()
        .map(|(name, source, _, _)| {
            (
                format!("Packages/User/{name}.sublime-syntax"),
                format!("name: {name}\n{source}"),
            )
        })
        .collect();
    for status in [vel_syntax::CacheStatus::Yaml, vel_syntax::CacheStatus::Hit] {
        let (set, actual) = SyntaxSet::load_cached_roots(
            &[],
            &files
                .iter()
                .map(|(p, s)| (p.as_str(), s.as_str()))
                .collect::<Vec<_>>(),
            cache.as_path(),
        )
        .unwrap();
        assert_eq!(actual, status);
        assert_eq!(set.warnings().len(), 3, "{:?}", set.warnings());
        assert!(
            set.warnings()
                .iter()
                .all(|w| w.contains("Packages/User/Partial.sublime-syntax"))
        );
        let set = Arc::new(set);
        for (name, _, expected, extensions) in &fixtures {
            assert_eq!(
                &set.find_syntax_by_name(name).unwrap().file_extensions,
                extensions
            );
            let (actual, _) = stacks(set.clone(), name, "x\n");
            assert_eq!(actual[0].trim(), *expected, "{name}");
        }
    }
    std::fs::remove_dir_all(cache).unwrap();
}

#[test]
fn missing_embedded_syntax_uses_plain_text_but_missing_context_stays_empty() {
    let source = r#"
name: Embed
scope: source.embed
contexts:
  main:
    - match: A
      embed: scope:source.absent
      escape: '!'
    - match: B
      embed: Packages/User/Absent.sublime-syntax
      escape: '!'
    - match: C
      embed: scope:source.present#absent
      escape: '!'
    - match: D
      embed: Packages/User/Present.sublime-syntax#absent
      escape: '!'
    - match: Z
      scope: keyword.after
"#;
    let set = Arc::new(
        SyntaxSet::load_roots(
            &[],
            &[
                ("User/Embed.sublime-syntax", source),
                (
                    "User/Plain.sublime-syntax",
                    "name: Plain Text\nscope: text.plain\ncontexts: {main: []}",
                ),
                (
                    "User/Present.sublime-syntax",
                    "name: Present\nscope: source.present\ncontexts: {main: []}",
                ),
            ],
        )
        .unwrap(),
    );
    assert_eq!(set.warnings().len(), 4);
    for (opening, plain) in [('A', true), ('B', true), ('C', false), ('D', false)] {
        let (actual, _) = stacks(set.clone(), "Embed", &format!("{opening}inside!Z\n"));
        assert_eq!(
            actual[1].contains("text.plain"),
            plain,
            "{opening}: {actual:?}"
        );
        assert_eq!(actual[8], "source.embed keyword.after");
    }
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/sublime-packages");
    let bundled = Arc::new(SyntaxSet::load_directories(&[root]).unwrap());
    let text = "x = <<~'HTML'\nhello\nHTML\nputs 'after'\n";
    let (actual, _) = stacks(bundled, "Ruby", text);
    assert!(actual[text.find("hello").unwrap()].contains("text.plain"));
    assert!(!actual[text.find("puts").unwrap()].contains("text.plain"));
}
