use super::*;
use regex_cursor::Cursor;

/// Independent cursor, so the test does not depend on the rope's leaf sizes.
struct Split<'a> {
    data: &'a str,
    starts: Vec<usize>,
    index: usize,
}

impl<'a> Split<'a> {
    fn new(data: &'a str, mut width: impl FnMut() -> usize) -> Self {
        let mut starts = vec![0];
        let mut at = 0;
        while at < data.len() {
            at = data.ceil_char_boundary((at + width()).min(data.len()));
            starts.push(at);
        }
        Self {
            data,
            starts,
            index: 0,
        }
    }
}

impl Cursor for Split<'_> {
    fn chunk(&self) -> &[u8] {
        &self.data.as_bytes()[self.offset()
            ..self
                .starts
                .get(self.index + 1)
                .copied()
                .unwrap_or(self.data.len())]
    }
    fn advance(&mut self) -> bool {
        if self.index + 2 < self.starts.len() {
            self.index += 1;
            true
        } else {
            false
        }
    }
    fn backtrack(&mut self) -> bool {
        if self.index > 0 {
            self.index -= 1;
            true
        } else {
            false
        }
    }
    fn total_bytes(&self) -> Option<usize> {
        Some(self.data.len())
    }
    fn offset(&self) -> usize {
        self.starts[self.index]
    }
}

fn compare(
    got: impl IntoIterator<Item = Range<usize>>,
    want: &[Range<usize>],
    context: impl std::fmt::Debug,
) {
    let got: Vec<_> = got.into_iter().collect();
    if got != want {
        let i = got
            .iter()
            .zip(want)
            .position(|(a, b)| a != b)
            .unwrap_or(got.len().min(want.len()));
        panic!(
            "{context:?}: match {i}: got {:?}, expected {:?} ({} vs {} total)",
            got.get(i),
            want.get(i),
            got.len(),
            want.len()
        );
    }
}

#[test]
fn greedy_regex_keeps_its_match_end_at_a_chunk_boundary() {
    for (query, unit) in [("t ?", "t t tx "), ("    ?", "    x     x ")] {
        for pad in 0..unit.len() {
            let source = "x".repeat(pad) + &unit.repeat(1600);
            let text = Text::from(source.as_str());
            let options = Options {
                regex: true,
                ..Options::default()
            };
            let search = Search::new(query, options).unwrap();
            let want: Vec<_> = regex::Regex::new(query)
                .unwrap()
                .find_iter(&source)
                .map(|m| m.range())
                .collect();
            compare(search.find(&text, &[]), &want, (query, pad, "find"));
            compare(
                search.replacements(&text, "$&", &[]).into_iter().map(|r| {
                    assert_eq!(r.text, source[r.range.clone()]);
                    r.range
                }),
                &want,
                (query, pad, "replace"),
            );
        }
    }
}

#[test]
fn rope_search_agrees_with_contiguous_over_random_chunkings() {
    let mut seed = 0x6C62_272E_07BB_0142_u64;
    let mut random = |limit: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % limit as u64) as usize
    };
    let tokens = [
        "kKsS ",
        "Straße ",
        "Kſ",
        "Σσς ",
        "Ａａ ",
        "İıIi ",
        "Жж ",
        "猫e\u{301}🦀",
        "t     ",
        " abc\r\n",
    ];
    let queries = [
        ("k", false, false),
        ("s", false, false),
        ("sk", false, false),
        ("ß", false, false),
        ("Σ", false, true),
        ("Ａ", false, false),
        ("i", false, true),
        ("猫e\u{301}🦀", false, false),
        ("t ?", true, false),
        ("    ?", true, false),
        (r"(?P<fold>[ks])(?:\p{Greek}|\s)*", true, false),
        (r"\b|^", true, false),
        (r"abc\r?\n", true, false),
    ];
    let mut trials = 0;
    for round in 0..8 {
        let n = if round == 0 { 1200 } else { 45 + random(80) };
        let mut source = String::new();
        for _ in 0..n {
            source.push_str(tokens[random(tokens.len())]);
        }
        let mut rope = Text::from(source.as_str());
        let at = source.floor_char_boundary(random(source.len()));
        source.insert_str(at, "ksKſ");
        rope.insert(at, "ksKſ");
        for (query, regex, whole_word) in queries {
            for case_sensitive in [false, true] {
                let options = Options {
                    regex,
                    case_sensitive,
                    whole_word,
                    ..Options::default()
                };
                let search = Search::new(query, options).unwrap();
                let src = if regex {
                    query.into()
                } else {
                    regex::escape(query)
                };
                let src = if whole_word {
                    format!(r"\b(?:{src})\b")
                } else {
                    src
                };
                let re = regex::RegexBuilder::new(&src)
                    .multi_line(true)
                    .case_insensitive(!case_sensitive)
                    .build()
                    .unwrap();
                let want: Vec<_> = re.find_iter(&source).map(|m| m.range()).collect();
                compare(
                    search.find(&rope, &[]),
                    &want,
                    (round, options, query, "edited rope"),
                );
                for size in (1..=64).chain([4096]) {
                    let chunks = Split::new(&source, || size);
                    compare(
                        search.matches_in(chunks),
                        &want,
                        (round, options, query, size),
                    );
                    trials += 1;
                }
                for _ in 0..2 {
                    let chunks = Split::new(&source, || 1 + random(64));
                    compare(
                        search.matches_in(chunks),
                        &want,
                        (round, options, query, "random widths"),
                    );
                    trials += 1;
                }
            }
        }
    }
    println!("checked {trials} cursor/query/flag combinations plus edited ropes");
}

#[test]
#[ignore = "compares the same large synthetic scans with and without automatic prefilters"]
fn benchmark_rope_prefilter() {
    use std::{hint::black_box, time::Instant};
    for size in [10_000_000, 100_000_000] {
        let line = "a".repeat(994) + " Q12\n";
        let source = line.repeat(size / line.len()) + "\nKey Needle 12345";
        let text = Text::from(source.as_str());
        for (label, query) in [
            ("literal", "Key Needle"),
            ("regex", r"Key\s+Needle\s+[0-9]{5}"),
        ] {
            for case_sensitive in [false, true] {
                let pattern = format!(
                    "(?m){}(?:{query})",
                    if case_sensitive { "" } else { "(?i)" }
                );
                for prefilter in [true, false] {
                    let regex = Regex::builder()
                        .configure(Regex::config().auto_prefilter(prefilter))
                        .build(&pattern)
                        .unwrap();
                    let mut samples = Vec::new();
                    for _ in 0..5 {
                        let now = Instant::now();
                        assert_eq!(
                            black_box(
                                regex
                                    .find_iter(Input::new(ChunkCursor::from(&text)))
                                    .count()
                            ),
                            1
                        );
                        samples.push(now.elapsed());
                    }
                    samples.sort();
                    println!(
                        "PREFILTER bytes={} kind={label} case={case_sensitive} prefilter={prefilter} median={:?}",
                        text.len_bytes(),
                        samples[2]
                    );
                }
            }
        }
    }
}
