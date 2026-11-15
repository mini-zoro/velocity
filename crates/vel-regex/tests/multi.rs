use std::sync::Arc;
use vel_regex::{
    MatcherBackend, MultiDfa, MultiDfaConfig, OnigBackend, PatternSpec, Regex, SearchCache,
};

fn check(patterns: &[&str], lines: &[&str]) {
    let specs: Vec<_> = patterns
        .iter()
        .map(|p| PatternSpec {
            regex: Regex::new((*p).into()),
            allow_empty: true,
        })
        .collect();
    let backend = MultiDfa::new(MultiDfaConfig {
        verify_matches: true,
        minimum_patterns: 1,
        ..MultiDfaConfig::default()
    });
    let multi = backend.build(specs.clone());
    let onig = OnigBackend::default().build(specs);
    let mut a = SearchCache::default();
    let mut b = SearchCache::default();
    for &line in lines {
        a.reset_line();
        b.reset_line();
        for start in (0..=line.len()).filter(|&p| line.is_char_boundary(p)) {
            for excluded in [&[][..], &[0][..], &[1, 3][..]] {
                assert_eq!(
                    multi.find_earliest_excluding(line, start, &mut a, excluded),
                    onig.find_earliest_excluding(line, start, &mut b, excluded),
                    "{line:?} at {start}; {patterns:?}"
                );
            }
        }
    }
}
#[test]
fn dialect_priority_independent_guards_and_captures() {
    let lines = [
        "foo **_**\n",
        "aab b aa",
        "f() ",
        "xaaa\n",
        "foo bar\r\n",
        "\n",
        "",
        "ß SS ﬃ",
        "_²³¹¼½¾_",
        "a\u{200c}b",
        "[ a#] ff12\n",
        "a\nb\n",
    ];
    for patterns in [
        vec![
            r"\b_(?=[^\s_])(?=[^*_]*\*\*)",
            r"\b_(?=\S)(?!_)",
            r"[*_]+",
            r"\*\*",
        ],
        vec![r"a+(?=a)", r"(a+)(b?)", r"(?=b)", r"b|aa"],
        vec![r"\Afoo", r"foo\Z", r"foo\z", r"\Gfoo", r"(?<=x)a", r"a\Kb"],
        vec![r"^", r"$", r"\b\w+\b", r"\B", r"\W+"],
        vec![
            r"(?i:ss|ffi|foo)",
            r"(?i:ß)",
            r"[[:alpha:][:digit:]_]",
            r"[\h]+",
        ],
        vec![
            "(?x)[ a#]+ \\h+ # comment\n",
            r"(?m:a.b)",
            r"(a|(b))*",
            r"(?:(a)|(b))+",
        ],
        vec![r"a*+a", r"[a-z&&[^b]]+", r"a(?=b)b", r"(a)\1"],
        vec![
            r"(?P<name>a)",
            r"[ab~~bc]",
            r"[a||b]",
            r"a",
            r"b",
            r"[a--b]",
        ],
        vec![r"\n^", r"\n$", r"a\n(?=^)", r"a", r"$"],
        vec![r"(a){0}", r"z", r"x"],
        vec![r"(a)(b){0}", r"z", r"x"],
        vec![r"(a){0}(b)", r"z", r"x"],
    ] {
        check(&patterns, &lines);
    }
}
#[test]
fn short_strings_exhaustive() {
    let patterns = [
        r"(a+)(b?)",
        r"a+(?!b)",
        r"a+(?=a)",
        r"(?=b)",
        r"[ab]+\b",
        r"^\s*$",
        r"a|ab",
        r".*?",
    ];
    let mut strings = vec![String::new()];
    for _ in 0..5 {
        let previous = strings.clone();
        for s in previous {
            for ch in ['a', 'b', ' ', '\n'] {
                strings.push(format!("{s}{ch}"));
            }
        }
    }
    let refs: Vec<_> = strings.iter().map(String::as_str).collect();
    check(&patterns, &refs);
}
#[test]
fn lazy_sharing_budget_eviction_and_thread_safety() {
    let backend = Arc::new(MultiDfa::new(MultiDfaConfig {
        memory_limit: 12_000,
        minimum_patterns: 1,
        ..MultiDfaConfig::default()
    }));
    let specs = || {
        vec![PatternSpec {
            regex: Regex::new("abc|def".into()),
            allow_empty: false,
        }]
    };
    let a = backend.build(specs());
    let _b = backend.build(specs());
    assert_eq!(backend.stats().compilations, 0);
    assert_eq!(backend.stats().pattern_sets, 1);
    let mut cache = SearchCache::default();
    assert!(a.find_earliest("abc", 0, &mut cache).is_some());
    let mut sets = vec![a];
    for i in 0..20 {
        let set = backend.build(vec![PatternSpec {
            regex: Regex::new(format!("abc|def{i}")),
            allow_empty: false,
        }]);
        cache.reset_line();
        assert!(set.find_earliest("abc", 0, &mut cache).is_some());
        assert!(backend.stats().resident_bytes <= 12_000);
        sets.push(set);
    }
    assert!(backend.stats().evictions > 0);
    std::thread::scope(|scope| {
        for set in &sets {
            scope.spawn(move || {
                assert!(
                    set.find_earliest("abc", 0, &mut SearchCache::default())
                        .is_some()
                );
            });
        }
    });
}

#[test]
fn caller_supplied_single_pattern_limits_are_respected() {
    let specs = vec![PatternSpec {
        regex: Regex::with_backend(
            r"(?:a+)+$".into(),
            Arc::new(OnigBackend {
                retry_limit: 20,
                stack_limit: 1024,
            }),
        ),
        allow_empty: true,
    }];
    let multi = MultiDfa::default().build(specs.clone());
    let reference = OnigBackend::default().build(specs);
    let line = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa!";
    assert_eq!(
        multi.find_earliest(line, 0, &mut SearchCache::default()),
        reference.find_earliest(line, 0, &mut SearchCache::default())
    );
}

#[test]
fn literal_prefilters_and_invalid_sources_agree_with_raw_onig() {
    for source in [
        r"(?x)[ a]",
        r"(?x)[ \t]",
        r"[ab~~bc]",
        r"[a||b]",
        r"[a--b]",
        r"(?P<name>a)",
    ] {
        let regex = Regex::new(source.into());
        match onig::Regex::new(source) {
            Ok(reference) => {
                for line in ["a", "b", "c", " ", "|", "~", "-", "\t", ""] {
                    assert_eq!(
                        regex.is_match(line),
                        reference.is_match(line),
                        "{source:?} on {line:?}"
                    );
                }
            }
            Err(_) => assert!(regex.compile_error().is_some(), "{source}"),
        }
    }
}

#[test]
fn necessary_literals_in_forward_assertions_and_concats_preserve_native_searches() {
    let patterns = [
        r"(?=\s*(?:::|!?[a-z]+|\$|<)+(<.*?>)?\s+for\s+)",
        r"(?=\w+\s*=>)",
        r"(?:[a-z]+)(?=\s*\()",
        r"(?=(?:[a-z]+|[0-9]+)\s*=>)",
        r"(?=a)b|c",
        r"a|b(?=c)",
        r"(?=ab)?c",
        r"(?=a)*b",
        r"(?!for)\w+",
        r"(?<=for )word",
        r"(?<=x)abc",
        r"a(?=b)c",
        r"(?=ab)(?=a)b",
        r"(?x:(?= \s* for [ ] ))",
        r"(?i)(?=for)",
        r"(?i:ß)",
        r"(?:a+)?for",
        r"[a-z]+for",
        r"(?=(?:[a-z]+)?for)",
        r"(?=\p{L}+for)",
        r"(?=a(?=b))",
        r"(?=a(?!b))",
        r"(?=(?:x|y))z",
        r"(?=x)|y",
        r"(a)\1",
    ];
    let lines = [
        "",
        "a",
        "ab",
        "abc",
        "ac",
        "b",
        "c",
        "x",
        "y",
        "z",
        "xaaabc",
        "identifier",
        "for",
        " for ",
        "abcd for ",
        "for word",
        "abc =>",
        "123 =>",
        "abc (",
        "abcfor",
        "FOR",
        "ss",
        "éfor",
        "你好for",
        "a\nb",
        "abc\n",
    ];
    for source in patterns {
        let regex = Regex::new(source.into());
        let Ok(raw) = onig::Regex::new(source) else {
            assert!(regex.compile_error().is_some(), "{source}");
            continue;
        };
        for line in lines {
            for start in (0..=line.len()).filter(|&at| line.is_char_boundary(at)) {
                let mut actual = vel_regex::Region::new();
                let mut expected = onig::Region::new();
                let found = raw.search_with_options(
                    line,
                    start,
                    line.len(),
                    onig::SearchOptions::SEARCH_OPTION_NONE,
                    Some(&mut expected),
                );
                assert_eq!(
                    regex.search(line, start, line.len(), Some(&mut actual), true),
                    found.is_some(),
                    "{source} at {start} on {line:?}"
                );
                if found.is_some() {
                    assert_eq!(actual.len(), expected.len(), "{source}");
                    for group in 0..actual.len() {
                        assert_eq!(
                            actual.pos(group),
                            expected.pos(group),
                            "{source} group {group}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn consuming_guard_locator_does_not_override_nonempty_policy() {
    let patterns = [r"(?=a)", r"a", r"b", r"c"]
        .into_iter()
        .map(|source| PatternSpec {
            regex: Regex::new(source.into()),
            allow_empty: false,
        })
        .collect::<Vec<_>>();
    let candidate = MultiDfa::default().build(patterns.clone());
    let reference = OnigBackend::default().build(patterns);
    assert_eq!(
        candidate.find_earliest("a", 0, &mut SearchCache::default()),
        reference.find_earliest("a", 0, &mut SearchCache::default())
    );
}

#[test]
fn prefix_filter_preserves_onig_search_to_semantics() {
    for source in ["abc", "ab+", "a.*c", "a", "[ab]{3}", "[ab][bc]"] {
        let regex = Regex::new(source.into());
        let raw = onig::Regex::new(source).unwrap();
        for end in 0..=4 {
            let mut captures = vel_regex::Region::new();
            let mut expected = onig::Region::new();
            let found = raw.search_with_options(
                "abcd",
                0,
                end,
                onig::SearchOptions::SEARCH_OPTION_NONE,
                Some(&mut expected),
            );
            assert_eq!(
                regex.search("abcd", 0, end, Some(&mut captures), true),
                found.is_some(),
                "{source}: {end}"
            );
            if found.is_some() {
                assert_eq!(captures.pos(0), expected.pos(0));
            }
        }
    }
}
