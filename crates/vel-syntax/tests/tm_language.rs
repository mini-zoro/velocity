//! Original fixtures exercise TextMate conversion independently of YAML rules.
use std::sync::Arc;
use vel_syntax::{ParseState, ScopeStack, SyntaxDefinition, SyntaxSet, SyntaxSetBuilder};
fn set(grammars: &[&str]) -> Arc<SyntaxSet> {
    let mut b = SyntaxSetBuilder::new();
    for (i, s) in grammars.iter().enumerate() {
        b.add_with_path(
            SyntaxDefinition::load_tm_language(s).unwrap(),
            format!("Fixture{i}.tmLanguage.json"),
        );
    }
    Arc::new(b.build())
}
fn scopes(set: &Arc<SyntaxSet>, syntax: usize, text: &str) -> Vec<(usize, String)> {
    let mut parser = ParseState::new(set.clone(), &set.syntaxes()[syntax]);
    let mut stack = ScopeStack::new();
    let mut result = Vec::new();
    let mut base = 0;
    for line in text.split_inclusive('\n') {
        let out = parser.parse_line(line).unwrap();
        let mut ops = out.ops.iter().peekable();
        for (offset, _) in line.char_indices() {
            while let Some((at, op)) = ops.peek()
                && *at <= offset
            {
                stack.apply(op).unwrap();
                ops.next();
            }
            result.push((
                base + offset,
                stack
                    .as_slice()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" "),
            ));
        }
        for (_, op) in ops {
            stack.apply(op).unwrap();
        }
        base += line.len();
    }
    result
}
fn at(spans: &[(usize, String)], p: usize) -> &str {
    &spans.iter().find(|(at, _)| *at == p).unwrap().1
}
#[test]
fn match_repository_captures_and_metadata() {
    let g = r##"{"name":"Fixture","scopeName":"source.fixture","fileTypes":["fix"],"firstLineMatch":"^#!fixture","patterns":[{"include":"#word"}],"repository":{"word":{"match":"(one)(two)","name":"keyword.fixture","captures":{"1":{"name":"left.fixture"},"2":{"name":"right.fixture"}}}}}"##;
    let set = set(&[g]);
    assert!(set.find_syntax_by_extension("fix").is_some());
    let s = scopes(&set, 0, "onetwo\n");
    assert!(at(&s, 0).ends_with("keyword.fixture left.fixture"));
    assert!(at(&s, 3).ends_with("keyword.fixture right.fixture"));
}
#[test]
fn begin_end_backrefs_scope_and_capture_fallback() {
    let g = r#"{"scopeName":"source.fixture","patterns":[{"begin":"BEGIN (.+)","end":"^\\1$","name":"string.fixture","contentName":"body.fixture","beginCaptures":{"1":{"name":"label.fixture"}},"endCaptures":{"0":{"name":"end.fixture"}}}]}"#;
    let s = scopes(&set(&[g]), 0, "BEGIN a.b\naxb\na.b\nz");
    assert!(at(&s, 6).contains("label.fixture"));
    assert!(!at(&s, 6).contains("body.fixture"));
    assert!(at(&s, 10).contains("body.fixture"));
    assert!(at(&s, 14).contains("end.fixture"));
    assert!(!at(&s, 14).contains("body.fixture"));
    assert_eq!(at(&s, 18), "source.fixture");
}
#[test]
fn xml_plist_cdata_and_json_are_equivalent() {
    let xml = r#"<?xml version="1.0"?><plist version="1.0"><dict><key>name</key><string>Fixture</string><key>scopeName</key><string>source.fixture</string><key>patterns</key><array><dict><key>match</key><string><![CDATA[<&>]]></string><key>name</key><string>keyword.fixture</string></dict></array></dict></plist>"#;
    let json = r#"{"name":"Fixture","scopeName":"source.fixture","patterns":[{"match":"<&>","name":"keyword.fixture"}]}"#;
    assert_eq!(
        scopes(&set(&[xml]), 0, "<&>"),
        scopes(&set(&[json]), 0, "<&>")
    );
}
#[test]
fn end_priority_can_be_applied_last() {
    for last in [false, true] {
        let g = format!(
            r#"{{"scopeName":"source.fixture","patterns":[{{"begin":"\\[","end":"x|\\]","name":"region.fixture","applyEndPatternLast":{last},"patterns":[{{"match":"x","name":"keyword.fixture"}}]}}]}}"#
        );
        let s = scopes(&set(&[&g]), 0, "[x]z");
        assert_eq!(at(&s, 1).contains("keyword.fixture"), last);
        assert!(!at(&s, 3).contains("region.fixture"));
    }
}
#[test]
fn self_base_cross_scope_and_external_repository() {
    let host = r##"{"scopeName":"source.host","patterns":[{"match":"h","name":"host.token"},{"begin":"<","end":">","patterns":[{"include":"source.guest#entry"}]}]}"##;
    let guest = r##"{"scopeName":"source.guest","patterns":[{"match":"g","name":"guest.token"}],"repository":{"entry":{"patterns":[{"include":"$base"},{"include":"$self"}]}}}"##;
    let s = set(&[host, guest]);
    let spans = scopes(&s, 0, "<hg>g");
    assert!(at(&spans, 1).contains("host.token"));
    assert!(at(&spans, 2).contains("guest.token"));
    assert!(!at(&spans, 4).contains("guest.token"));
    let guest_spans = scopes(&s, 1, "g");
    assert!(at(&guest_spans, 0).contains("guest.token"));
}
#[test]
fn nested_repository_shadowing_and_recursive_include() {
    let g = r##"{"scopeName":"source.fixture","patterns":[{"include":"#entry"},{"match":"z","name":"outer.token"}],"repository":{"entry":{"patterns":[{"begin":"\\(","end":"\\)","patterns":[{"include":"#entry"},{"include":"#inner"}],"repository":{"inner":{"match":"z","name":"inner.token"}}}]}}}"##;
    let s = scopes(&set(&[g]), 0, "((z))z");
    assert!(at(&s, 2).contains("inner.token"));
    assert!(at(&s, 5).contains("outer.token"));
}
#[test]
fn while_matches_each_line_and_unwinds_nested_contexts() {
    let g = r#"{"scopeName":"source.fixture","patterns":[{"begin":"^>(x)","while":"^>\\1","name":"quote.fixture","whileCaptures":{"0":{"name":"prefix.fixture"}},"patterns":[{"begin":"\\[","end":"\\]","name":"inner.fixture"}]}]}"#;
    let s = scopes(&set(&[g]), 0, ">x[\n>xbody\nplain\n");
    assert!(at(&s, 4).contains("prefix.fixture"));
    assert!(at(&s, 6).contains("inner.fixture"));
    assert!(!at(&s, 11).contains("quote.fixture"));
    assert!(!at(&s, 11).contains("inner.fixture"));
}
#[test]
fn unsupported_constructs_and_invalid_input_are_diagnostic() {
    for g in [
        r#"{"scopeName":"source.fixture","injections":{}}"#,
        r#"{"scopeName":"source.fixture","patterns":[{"match":"x","name":"word.$1"}]}"#,
        r#"{"scopeName":"source.fixture","patterns":[{"match":"(x)","captures":{"1":{"patterns":[]}}}]}"#,
        "<plist>",
        "{}",
    ] {
        assert!(SyntaxDefinition::load_tm_language(g).is_err(), "{g}");
    }
}
#[test]
fn folder_loading_and_cache_invalidation_cover_both_extensions() {
    let root = std::env::temp_dir().join(format!("vel-tm-{}", std::process::id()));
    let defs = root.join("definitions");
    let cache = root.join("cache");
    std::fs::create_dir_all(&defs).unwrap();
    let json = r#"{"scopeName":"source.fixture","patterns":[{"match":"x","name":"one.fixture"}]}"#;
    std::fs::write(defs.join("Fixture.tmLanguage.json"), json).unwrap();
    let xml = r#"<plist version="1.0"><dict><key>name</key><string>Other</string><key>scopeName</key><string>source.other</string><key>patterns</key><array><dict><key>match</key><string>x</string><key>name</key><string>one.other</string></dict></array></dict></plist>"#;
    std::fs::write(defs.join("Other.tmLanguage"), xml).unwrap();
    let (s, kind) = SyntaxSet::load_cached(&defs, &cache).unwrap();
    assert_eq!(kind, vel_syntax::CacheStatus::Yaml);
    assert!(s.warnings().is_empty());
    let (s, kind) = SyntaxSet::load_cached(&defs, &cache).unwrap();
    assert_eq!(kind, vel_syntax::CacheStatus::Hit);
    assert!(at(&scopes(&Arc::new(s), 0, "x"), 0).contains("one.fixture"));
    std::fs::write(
        defs.join("Fixture.tmLanguage.json"),
        json.replace("one.fixture", "two.fixture"),
    )
    .unwrap();
    assert_eq!(
        SyntaxSet::load_cached(&defs, &cache).unwrap().1,
        vel_syntax::CacheStatus::Yaml
    );
    assert_eq!(
        SyntaxSet::load_cached(&defs, &cache).unwrap().1,
        vel_syntax::CacheStatus::Hit
    );
    std::fs::write(
        defs.join("Other.tmLanguage"),
        xml.replace("one.other", "two.other"),
    )
    .unwrap();
    let (s, kind) = SyntaxSet::load_cached(&defs, &cache).unwrap();
    assert_eq!(kind, vel_syntax::CacheStatus::Yaml);
    assert_eq!(s.syntaxes().len(), 2);
    assert!(at(&scopes(&Arc::new(s), 1, "x"), 0).contains("two.other"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn public_mit_grammars_fetched_at_test_time_skip_offline() {
    // VS Code 1.105.0 is MIT; download its notice alongside the grammar sources.
    // The fetched assets stay in memory and are never vendored into this crate.
    fn fetch(path: &str) -> Option<String> {
        let url = format!("https://raw.githubusercontent.com/microsoft/vscode/1.105.0/{path}");
        let output = std::process::Command::new("curl")
            .args([
                "--fail",
                "--silent",
                "--show-error",
                "--connect-timeout",
                "3",
                "--max-time",
                "8",
                &url,
            ])
            .output();
        let Ok(output) = output else {
            eprintln!("SKIP public grammars: curl is unavailable");
            return None;
        };
        if !output.status.success() {
            assert_ne!(
                output.status.code(),
                Some(22),
                "public fixture missing: {path}"
            );
            eprintln!(
                "SKIP public grammars: offline ({})",
                String::from_utf8_lossy(&output.stderr)
            );
            return None;
        }
        Some(String::from_utf8(output.stdout).expect("UTF-8 public grammar"))
    }
    let Some(license) = fetch("LICENSE.txt") else {
        return;
    };
    assert!(license.contains("MIT License"));
    for (path, sample, scope, token, expected) in [
        (
            "extensions/json/syntaxes/JSON.tmLanguage.json",
            r#"{"key": true}"#,
            "source.json",
            "true",
            "constant.language.json",
        ),
        (
            "extensions/diff/syntaxes/diff.tmLanguage.json",
            "--- a/file\n+++ b/file\n@@ -1 +1 @@\n-removed\n+added\n",
            "source.diff",
            "added",
            "markup.inserted.diff",
        ),
        (
            "extensions/log/syntaxes/log.tmLanguage.json",
            "2026-11-15 12:00:00 [error] failed\n",
            "text.log",
            "error",
            "log.error",
        ),
    ] {
        let Some(grammar) = fetch(path) else {
            return;
        };
        let grammar = set(&[&grammar]);
        let spans = scopes(&grammar, 0, sample);
        assert!(
            spans.iter().all(|(_, stack)| stack.contains(scope)),
            "{path}"
        );
        assert!(
            spans
                .iter()
                .any(|(_, stack)| stack.split_whitespace().count() > 1),
            "grammar did not tokenize: {path}"
        );
        assert!(
            at(&spans, sample.find(token).unwrap())
                .split_whitespace()
                .any(|scope| scope == expected),
            "missing {expected} for {token} in {path}"
        );
        eprintln!("public grammar verified: {path}");
    }
}

#[test]
fn base_identity_survives_setting_to_shared_embedded_context() {
    let mut builder = SyntaxSetBuilder::new();
    for (name, token) in [("A", "a"), ("B", "b")] {
        builder.add(SyntaxDefinition::load_from_str(&format!(
            "name: {name}\nscope: source.{name}\ncontexts:\n  main:\n    - match: x\n      set: scope:source.guest\n    - match: {token}\n      scope: token.{name}\n"
        ), true, None).unwrap());
    }
    builder.add(
        SyntaxDefinition::load_tm_language(
            r#"{"name":"Guest","scopeName":"source.guest","patterns":[{"include":"$base"}]}"#,
        )
        .unwrap(),
    );
    let set = Arc::new(builder.build());
    let mut a = ParseState::new(set.clone(), set.find_syntax_by_name("A").unwrap());
    let mut b = ParseState::new(set.clone(), set.find_syntax_by_name("B").unwrap());
    a.parse_line("x\n").unwrap();
    b.parse_line("x\n").unwrap();
    assert_ne!(a, b, "the immutable document base is semantic state");
    assert!(
        scopes(&set, 0, "x\na")
            .iter()
            .any(|(_, stack)| stack.contains("token.A"))
    );
    assert!(
        scopes(&set, 1, "x\nb")
            .iter()
            .any(|(_, stack)| stack.contains("token.B"))
    );
}
