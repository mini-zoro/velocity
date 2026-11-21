use super::*;
use std::cell::Cell;

fn regex(pattern: &str) -> Search {
    Search::new(
        pattern,
        Options {
            case_sensitive: true,
            regex: true,
            ..Options::default()
        },
    )
    .unwrap_or_else(|error| panic!("{pattern}: {error}"))
}

#[test]
fn accepted_patterns_stay_on_the_rope() {
    for query in [
        "^",
        "$",
        r"(?s).+",
        r"\b\w+\b",
        r"(?P<name>a+)",
        "a++a",
        "(?R)foo",
    ] {
        assert!(!regex(query).uses_perl(), "{query}");
    }
    assert!(
        !Search::new("(?=b)", Options::default())
            .unwrap()
            .uses_perl()
    );
    assert_eq!(
        regex("a++a").find(&Text::from("aaa"), &[]),
        std::slice::from_ref(&(0..3))
    );
}

#[test]
fn perl_constructs() {
    for (query, input, expected) in [
        (r"a(?=b)", "ab ac", &[(0, 1)][..]),
        (r"a(?!b)", "ab ac", &[(3, 4)]),
        (r"(?<=a)b", "ab cb", &[(1, 2)]),
        (r"(?<!a)b", "ab cb", &[(4, 5)]),
        (r"(\w+):\1", "xy:xy no:yes", &[(0, 5)]),
        (r"(?<n>\w+):\k<n>", "猫:猫 犬:鳥", &[(0, 7)]),
        (r"(?>a+)a|b", "aaab", &[(3, 4)]),
        (r"(?=a)a++a|b", "aaab", &[(3, 4)]),
        (r"(?=a)a{1,3}+a|b", "aaab", &[(3, 4)]),
        (r"(?=a)a*+a|b", "aaab", &[(3, 4)]),
        (r"(?=a)a?+a|b", "ab", &[(1, 2)]),
        (r"\G\w", "é猫 z", &[(0, 2), (2, 5)]),
        (r"(a)?b(?(1)c|d)", "abc bd abd", &[(0, 3), (4, 6), (8, 10)]),
        (r"(?<nest>\((?:[^()]|(?&nest))*\))", "(a(b)c)", &[(0, 7)]),
        (r"(\((?:[^()]|(?1))*\))", "(a(b)c)", &[(0, 7)]),
        // The rope engine recognizes bare (?R) as its CRLF flag.
        (r"(?=)\((?:[^()]|(?R))*\)", "(a(b)c)", &[(0, 7)]),
    ] {
        let search = regex(query);
        assert!(search.uses_perl(), "{query}");
        let hits = search.find_cancel(&Text::from(input), &[], || false);
        assert_eq!(
            hits.unwrap().unwrap(),
            expected.iter().map(|&(a, b)| a..b).collect::<Vec<_>>(),
            "{query}"
        );
    }
}

#[test]
fn black_box_replacements_and_line_anchors() {
    for (query, replacement, expected) in [
        ("(?=b)", "-", "a-bc\ndef\n"),
        ("^", "> ", "> abc\n> def\n> "),
        ("(?=)^", "> ", "> abc\n> def\n> "),
        (r"$(?=)", "!", "abc!\ndef!\n!"),
        (r"(?-m:^)(?=)", ">", ">abc\ndef\n"),
        (r"(?-m:^)|(?m:^)$(?=)", ">", ">abc\ndef\n>"),
    ] {
        let mut buffer = Buffer::new("abc\ndef\n");
        let view = buffer.new_view();
        let edits = regex(query)
            .try_replacements(buffer.text(), replacement, &[])
            .unwrap();
        apply(&mut buffer, view, &edits);
        assert_eq!(buffer.text().to_string(), expected, "{query}");
        assert!(buffer.undo(view));
        assert_eq!(buffer.text().to_string(), "abc\ndef\n");
    }
    let text = Text::from("x\nabc");
    assert!(regex(r"x.(?=a)").find(&text, &[]).is_empty());
    assert_eq!(
        regex(r"(?s:x.)(?=a)").find(&text, &[]),
        std::slice::from_ref(&(0..2))
    );
}

#[test]
fn scope_and_options_keep_full_assertion_context() {
    let input = Text::from("abcxd");
    assert_eq!(
        regex(r"a.*(?=c|d)").find(&input, std::slice::from_ref(&(0..2))),
        std::slice::from_ref(&(0..2))
    );
    assert_eq!(
        regex(r"(?<=a)b(?=c)").find(&input, std::slice::from_ref(&(1..2))),
        std::slice::from_ref(&(1..2))
    );
    assert!(
        regex(r"b(?!c)")
            .find(&input, std::slice::from_ref(&(1..2)))
            .is_empty()
    );
    assert_eq!(
        regex(r"(?=c)").find(&input, std::slice::from_ref(&(0..2))),
        std::slice::from_ref(&(2..2))
    );
    let text = Text::from("É é éx é\u{200d}");
    for (case_sensitive, expected) in [(true, &[(3, 5)][..]), (false, &[(0, 2), (3, 5)][..])] {
        let search = Search::new(
            r"(?=é)é",
            Options {
                regex: true,
                whole_word: true,
                case_sensitive,
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(
            search.find(&text, &[]),
            expected.iter().map(|&(a, b)| a..b).collect::<Vec<_>>()
        );
    }
}

#[test]
fn utf8_zero_width_navigation_includes_eof_once() {
    let input = Text::from("é猫\n");
    let search = regex(r"(?=)");
    let hits = search.find(&input, &[]);
    assert_eq!(hits, [0..0, 2..2, 5..5, 6..6]);
    assert_eq!(search.next(&hits, 0, Some(&(0..0)), false), Some(2..2));
    assert_eq!(search.next(&hits, 0, Some(&(0..0)), true), Some(6..6));
    assert_eq!(
        search.find(&Text::from(""), &[]),
        std::slice::from_ref(&(0..0))
    );
    // Captures outside a zero-width match still participate.
    assert_eq!(
        regex(r"(?=(猫))")
            .try_replacements(&input, "<$1>", &[])
            .unwrap()[0]
            .text,
        "<猫>"
    );
}

#[test]
fn malformed_patterns_still_fail() {
    let options = regex("a").options;
    for pattern in ["[", r"\q", "(?=b", "(?>", r"(a)\2", r"(?<n>a)\k<m>"] {
        assert!(Search::new(pattern, options).is_err(), "{pattern}");
    }
}

#[test]
fn perl_copy_and_scan_are_cancellable_and_bounded() {
    let text = Text::from("a".repeat(32_768).as_str());
    let search = regex(r"(?=a)");
    let cancelled = Cell::new(0);
    assert!(
        search
            .find_cancel(&text, &[], || {
                cancelled.set(cancelled.get() + 1);
                cancelled.get() == 2
            })
            .unwrap()
            .is_none()
    );
    let cancelled = Cell::new(0);
    assert!(
        perl::find(
            search.perl().unwrap(),
            "abc",
            std::slice::from_ref(&(0..3)),
            &|| {
                cancelled.set(cancelled.get() + 1);
                cancelled.get() >= 2
            }
        )
        .unwrap()
        .is_none()
    );
    assert!(perl::check_size(perl::MAX_INPUT as u64 + 1).is_err());
}
