use super::*;
use proptest::prelude::*;
use std::{collections::hash_map::DefaultHasher, io::Read};
use unicode_segmentation::UnicodeSegmentation;

fn arb_text(max: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop_oneof![
            6 => (0x20u8..0x7f).prop_map(|b| char::from(b).to_string()),
            2 => Just("\n".to_owned()), 1 => Just("\r".to_owned()),
            2 => Just("中文かな".to_owned()), 2 => Just("🦀𐐀😀".to_owned()),
            2 => Just("e\u{301}\u{308}".to_owned()),
            1 => Just("👩🏽\u{200d}💻🇺🇳".to_owned()),
            1 => any::<char>().prop_map(|c| c.to_string()),
        ],
        0..max,
    )
    .prop_map(|v| v.concat())
}

fn boundaries(s: &str) -> Vec<usize> {
    s.char_indices().map(|(b, _)| b).chain([s.len()]).collect()
}
fn hash(value: &impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}

fn check_view(view: TextSlice<'_>, s: &str) {
    assert_eq!(view.to_string(), s);
    assert_eq!(view.len_bytes(), s.len());
    assert_eq!(view.len_chars(), s.chars().count());
    assert_eq!(view.len_utf16(), s.encode_utf16().count());
    assert_eq!(
        view.len_lines(),
        s.bytes().filter(|b| *b == b'\n').count() + 1
    );
    assert_eq!(view.is_empty(), s.is_empty());
    assert!(view == s);
    assert_eq!(view.chunks().collect::<String>(), s);
    let mut chunks = view.chunks().rev().collect::<Vec<_>>();
    chunks.reverse();
    assert_eq!(chunks.concat(), s);
    assert_eq!(view.chars().collect::<String>(), s);
    assert_eq!(
        view.chars().rev().collect::<String>(),
        s.chars().rev().collect::<String>()
    );
    assert_eq!(view.bytes().collect::<Vec<_>>(), s.as_bytes());
    assert_eq!(
        view.bytes().rev().collect::<Vec<_>>(),
        s.bytes().rev().collect::<Vec<_>>()
    );
    let lines = view.lines().collect::<Vec<_>>();
    assert_eq!(
        lines.iter().map(ToString::to_string).collect::<Vec<_>>(),
        s.split('\n').collect::<Vec<_>>()
    );
    let b = boundaries(s);
    let mut utf16 = 0;
    for (ch, &byte) in b.iter().enumerate() {
        if ch > 0 {
            utf16 += s[b[ch - 1]..byte].encode_utf16().count();
        }
        // Exercise endpoints and a regular sample on larger inputs.
        if b.len() > 200 && ch % (b.len() / 100) != 0 && ch + 1 != b.len() {
            continue;
        }
        assert_eq!(view.byte_to_char(byte), ch);
        assert_eq!(view.char_to_byte(ch), byte);
        assert_eq!(view.byte_to_utf16(byte), utf16);
        assert_eq!(view.utf16_to_byte(utf16), byte);
        let row = s[..byte].bytes().filter(|b| *b == b'\n').count();
        let start = s[..byte].rfind('\n').map_or(0, |n| n + 1);
        assert_eq!(view.byte_to_line(byte), row);
        assert_eq!(view.line_to_byte(row), start);
        let p = Point {
            row,
            column: byte - start,
        };
        let p16 = PointUtf16 {
            row,
            column: s[start..byte].encode_utf16().count(),
        };
        assert_eq!(view.byte_to_point(byte), p);
        assert_eq!(view.point_to_byte(p), byte);
        assert_eq!(view.byte_to_point_utf16(byte), p16);
        assert_eq!(view.point_utf16_to_byte(p16), byte);
        assert_eq!(view.clip_point(p), p);
        assert_eq!(view.clip_point_utf16(p16), p16);
        assert_eq!(view.clip_char(ch), ch);
        assert_eq!(view.clip_byte(byte), byte);
        assert_eq!(view.clip_utf16(utf16), utf16);
        if byte < s.len() {
            assert_eq!(view.char_at(byte), s[byte..].chars().next().unwrap());
            let (chunk, at) = view.chunk_at_byte(byte);
            assert!(at <= byte && byte < at + chunk.len());
            assert_eq!(chunk, &s[at..at + chunk.len()]);
        }
    }
    for (row, line) in s.split('\n').enumerate() {
        assert_eq!(view.line_len_bytes(row), line.len());
        assert_eq!(view.line(row).to_string(), line);
        assert_eq!(lines[row].len_utf16(), line.encode_utf16().count());
        assert_eq!(view.clip_line(row), row);
    }
    for byte in 0..s.len().min(256) {
        assert_eq!(view.byte_at(byte), s.as_bytes()[byte]);
        assert_eq!(view.is_char_boundary(byte), s.is_char_boundary(byte));
        assert_eq!(view.floor_char_boundary(byte), s.floor_char_boundary(byte));
        assert_eq!(view.ceil_char_boundary(byte), s.ceil_char_boundary(byte));
    }
    assert!(!view.is_char_boundary(usize::MAX));
    assert_eq!(view.floor_char_boundary(usize::MAX), s.len());
    assert_eq!(view.ceil_char_boundary(usize::MAX), s.len());
    assert_eq!(view.clip_utf16(usize::MAX), view.len_utf16());
    assert_eq!(view.clip_char(usize::MAX), view.len_chars());
    let last = s.rsplit('\n').next().unwrap();
    assert_eq!(
        view.clip_point(Point {
            row: usize::MAX,
            column: usize::MAX
        }),
        Point {
            row: view.len_lines() - 1,
            column: last.len()
        }
    );
    assert_eq!(
        view.clip_point_utf16(PointUtf16 {
            row: usize::MAX,
            column: usize::MAX
        }),
        PointUtf16 {
            row: view.len_lines() - 1,
            column: last.encode_utf16().count()
        }
    );
    let (chunk, at) = view.chunk_at_byte(s.len());
    assert_eq!(at + chunk.len(), s.len());
    assert_eq!(chunk, &s[at..]);
    let graphemes: Vec<_> = s
        .grapheme_indices(true)
        .map(|(b, _)| b)
        .chain([s.len()])
        .collect();
    for &byte in b.iter().step_by((b.len() / 80).max(1)) {
        let next = graphemes
            .iter()
            .copied()
            .find(|b| *b > byte)
            .unwrap_or(s.len());
        let prev = graphemes.iter().copied().rfind(|b| *b < byte).unwrap_or(0);
        assert_eq!(
            view.next_grapheme_boundary(byte),
            next,
            "next at {byte} in {s:?}"
        );
        assert_eq!(
            view.prev_grapheme_boundary(byte),
            prev,
            "prev at {byte} in {s:?}"
        );
    }
    assert_eq!(hash(&view), hash(&Text::from(s)));
}

// Deterministic seeds make a failed transaction directly reproducible. Unlike
// the sampled large-view helper above, this oracle checks every coordinate in
// every generated document after every batch, including retained snapshots.
fn check_batch_model(text: &Text, model: &str) {
    tree::assert_invariants(&text.root);
    assert_eq!(text.root.summary, tree::Summary::of(model));
    assert_eq!(text.to_string(), model);
    assert_eq!(text.chunks().collect::<String>(), model);
    assert_eq!(
        text.chunks()
            .rev()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<String>(),
        model
    );
    let mut row = 0;
    let mut line_start = 0;
    let mut units = 0;
    let mut line_units = 0;
    for (scalar, (byte, c)) in model
        .char_indices()
        .map(|(b, c)| (b, Some(c)))
        .chain([(model.len(), None)])
        .enumerate()
    {
        assert_eq!(text.byte_to_char(byte), scalar);
        assert_eq!(text.char_to_byte(scalar), byte);
        assert_eq!(text.byte_to_utf16(byte), units);
        assert_eq!(text.utf16_to_byte(units), byte);
        assert_eq!(text.byte_to_line(byte), row);
        assert_eq!(text.line_to_byte(row), line_start);
        assert_eq!(
            text.byte_to_point(byte),
            Point {
                row,
                column: byte - line_start
            }
        );
        assert_eq!(
            text.point_to_byte(Point {
                row,
                column: byte - line_start
            }),
            byte
        );
        assert_eq!(
            text.byte_to_point_utf16(byte),
            PointUtf16 {
                row,
                column: units - line_units
            }
        );
        assert_eq!(
            text.point_utf16_to_byte(PointUtf16 {
                row,
                column: units - line_units
            }),
            byte
        );
        if let Some(c) = c {
            assert_eq!(text.char_at(byte), c);
            units += c.len_utf16();
            if c == '\n' {
                row += 1;
                line_start = byte + 1;
                line_units = units;
            }
        }
    }
    for byte in 0..=model.len() {
        assert_eq!(text.is_char_boundary(byte), model.is_char_boundary(byte));
        assert_eq!(
            text.floor_char_boundary(byte),
            model.floor_char_boundary(byte)
        );
        assert_eq!(
            text.ceil_char_boundary(byte),
            model.ceil_char_boundary(byte)
        );
        let (chunk, start) = text.chunk_at_byte(byte);
        assert_eq!(chunk, &model[start..start + chunk.len()]);
        assert!(start <= byte && byte <= start + chunk.len());
        if byte < model.len() {
            assert_eq!(text.byte_at(byte), model.as_bytes()[byte]);
        }
    }
    for (row, line) in model.split('\n').enumerate() {
        assert_eq!(text.line(row).to_string(), line);
        assert_eq!(text.line_len_bytes(row), line.len());
    }
}

#[test]
fn batch_string_model_and_immutable_snapshots_over_5000_seeds() {
    const TOKENS: &[&str] = &[
        "",
        "x",
        "let x = 1; ",
        " ",
        "\t",
        "\n",
        "\r\n",
        "\r",
        "你好",
        "🦀",
        "e\u{301}",
        "\u{301}",
        "🇺🇳",
    ];
    for seed in (0..5000_u64).chain([1000493]) {
        let mut state = seed + 1;
        let mut next = |n: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as usize % n
        };
        let mut model = (0..32)
            .map(|_| TOKENS[next(TOKENS.len())])
            .collect::<String>();
        let mut text = Text::from(model.as_str());
        let mut snapshots = Vec::new();
        for batch in 0..40 {
            let before = text.clone();
            let old = model.clone();
            let points = boundaries(&model);
            let mut owned = Vec::new();
            let mut end = 0;
            // Some entire batches are empty. Adjacent ranges and repeated zero
            // length positions deliberately survive sorting and overlap removal.
            for _ in 0..if batch % 7 == 0 { 0 } else { next(12) + 1 } {
                let a = next(points.len());
                let b = if batch % 3 == 0 {
                    a
                } else {
                    next(points.len())
                };
                owned.push((
                    points[a.min(b)]..points[a.max(b)],
                    TOKENS[next(TOKENS.len())],
                ));
            }
            owned.sort_by_key(|(r, _)| (r.start, r.end));
            owned.retain(|(r, _)| {
                if r.start >= end {
                    end = r.end;
                    true
                } else {
                    false
                }
            });
            let edits = owned
                .iter()
                .map(|(r, s)| Edit {
                    range: r.clone(),
                    text: s,
                })
                .collect::<Vec<_>>();
            text.apply_edits(&edits);
            for e in edits.iter().rev() {
                model.replace_range(e.range.clone(), e.text);
            }
            check_batch_model(&text, &model);
            check_batch_model(&before, &old);
            let points = boundaries(&model);
            for _ in 0..3 {
                let a = next(points.len());
                let b = next(points.len());
                let range = points[a.min(b)]..points[a.max(b)];
                check_view(text.slice(range.clone()), &model[range]);
            }
            if batch % 6 == 0 {
                snapshots.push((before, old));
            }
            for (snapshot, old) in &snapshots {
                assert_eq!(snapshot.to_string(), *old, "seed {seed}, batch {batch}");
                assert_eq!(snapshot.root.summary, tree::Summary::of(old));
            }
        }
        // True overlaps are invalid, even if a preceding edit was valid. The
        // entire call must fail atomically; adjacent edits above must succeed.
        if !model.is_empty() {
            let end = model.chars().next().unwrap().len_utf8();
            let before = text.clone();
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| text.apply_edits(&[
                    Edit {
                        range: 0..end,
                        text: "replacement"
                    },
                    Edit {
                        range: 0..0,
                        text: "overlap"
                    },
                ])))
                .is_err()
            );
            check_batch_model(&text, &model);
            assert_eq!(text, before);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(72))]
    #[test]
    fn read_operations_match_string(s in arb_text(900), a in any::<usize>(), b in any::<usize>()) {
        let text = Text::from_str(&s);
        tree::assert_invariants(&text.root);
        check_view(text.as_slice(), &s);
        assert!(text.estimated_storage_bytes() >= text.len_bytes());
        assert_eq!(text.estimated_storage_bytes(), text.clone().estimated_storage_bytes());
        let view = text.as_slice();
        for byte in boundaries(&s).into_iter().step_by((text.len_chars() / 40).max(1)) {
            assert_eq!(text.byte_to_char(byte), view.byte_to_char(byte));
            assert_eq!(text.char_to_byte(text.byte_to_char(byte)), byte);
            assert_eq!(text.byte_to_utf16(byte), view.byte_to_utf16(byte));
            assert_eq!(text.utf16_to_byte(text.byte_to_utf16(byte)), byte);
            assert_eq!(text.byte_to_line(byte), view.byte_to_line(byte));
            assert_eq!(text.line_to_byte(text.byte_to_line(byte)), view.line_to_byte(view.byte_to_line(byte)));
            assert_eq!(text.byte_to_point(byte), view.byte_to_point(byte));
            assert_eq!(text.point_to_byte(text.byte_to_point(byte)), byte);
            assert_eq!(text.byte_to_point_utf16(byte), view.byte_to_point_utf16(byte));
            assert_eq!(text.point_utf16_to_byte(text.byte_to_point_utf16(byte)), byte);
            assert_eq!(text.clip_byte(byte + 1), view.clip_byte(byte + 1));
            assert_eq!(text.clip_utf16(byte), view.clip_utf16(byte));
            assert_eq!(text.floor_char_boundary(byte + 1), view.floor_char_boundary(byte + 1));
            assert_eq!(text.ceil_char_boundary(byte + 1), view.ceil_char_boundary(byte + 1));
            assert_eq!(text.next_grapheme_boundary(byte), view.next_grapheme_boundary(byte));
            assert_eq!(text.prev_grapheme_boundary(byte), view.prev_grapheme_boundary(byte));
        }
        assert_eq!(text.lines().map(|l| l.to_string()).collect::<Vec<_>>(), s.split('\n').collect::<Vec<_>>());
        assert_eq!(text.chunks().collect::<String>(), s);
        assert_eq!(text.chars().collect::<String>(), s);
        assert_eq!(text.bytes().collect::<Vec<_>>(), s.as_bytes());
        let boundaries = boundaries(&s);
        let a = boundaries[a % boundaries.len()]; let b = boundaries[b % boundaries.len()];
        let range = a.min(b)..a.max(b);
        let view = text.slice(range.clone());
        check_view(view, &s[range]);
        check_view(view.slice(0..view.len_bytes()), &view.to_string());
        assert_eq!(Text::from(s.clone()), text);
        assert_eq!(s.parse::<Text>().unwrap(), text);
    }

    #[test]
    fn edits_match_string(s in arb_text(700), ops in prop::collection::vec((any::<u16>(), any::<u16>(), arb_text(35), 0u8..4), 1..32)) {
        let mut model = s.clone(); let mut text = Text::from(&*s);
        for (a, b, replacement, kind) in ops {
            let points = boundaries(&model);
            let a = points[a as usize % points.len()]; let b = points[b as usize % points.len()];
            let range = a.min(b)..a.max(b);
            let snapshot = text.clone(); let before = model.clone();
            match kind {
                0 => { text.replace(range.clone(), &replacement); model.replace_range(range, &replacement); }
                1 => { text.insert(a, &replacement); model.insert_str(a, &replacement); }
                2 => { text.delete(range.clone()); model.replace_range(range, ""); }
                _ => {
                    let edits = [Edit { range: 0..range.start, text: "🦀\n" }, Edit { range: range.end..model.len(), text: &replacement }];
                    text.apply_edits(&edits);
                    for e in edits.iter().rev() { model.replace_range(e.range.clone(), e.text); }
                }
            }
            tree::assert_invariants(&text.root);
            tree::assert_invariants(&snapshot.root);
            assert!(text.estimated_storage_bytes() >= text.len_bytes());
            assert_eq!(text.to_string(), model);
            assert_eq!(snapshot.to_string(), before);
            assert_eq!(text.len_utf16(), model.encode_utf16().count());
            assert_eq!(text.len_chars(), model.chars().count());
        }
        check_view(text.as_slice(), &model);
    }

    #[test]
    fn batches_equal_reverse_sequential(s in arb_text(1700), data in prop::collection::vec((any::<u16>(), any::<u16>(), arb_text(12)), 0..70)) {
        let points = boundaries(&s);
        let mut ranges = data.iter().map(|(a,b,t)| {
            let a = points[*a as usize % points.len()]; let b = points[*b as usize % points.len()];
            Edit { range: a.min(b)..a.max(b), text: t }
        }).collect::<Vec<_>>();
        ranges.sort_by_key(|e| (e.range.start, e.range.end));
        let mut end = 0;
        ranges.retain(|e| if e.range.start >= end { end = e.range.end; true } else { false });
        let mut model = s.clone(); let mut sequential = Text::from(&*s); let mut batch = sequential.clone();
        for e in ranges.iter().rev() { model.replace_range(e.range.clone(), e.text); sequential.replace(e.range.clone(), e.text); tree::assert_invariants(&sequential.root); }
        batch.apply_edits(&ranges);
        tree::assert_invariants(&batch.root);
        assert_eq!(batch, sequential);
        assert_eq!(batch.to_string(), model);
    }

    #[test]
    fn streaming_load_matches(s in arb_text(800), width in 1usize..257) {
        struct Short<'a> { bytes: &'a [u8], width: usize, interrupt: bool }
        impl Read for Short<'_> {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                self.interrupt = !self.interrupt;
                if self.interrupt { return Err(io::ErrorKind::Interrupted.into()); }
                let n = buf.len().min(self.width).min(self.bytes.len());
                buf[..n].copy_from_slice(&self.bytes[..n]); self.bytes = &self.bytes[n..]; Ok(n)
            }
        }
        let t = Text::from_reader(Short { bytes: s.as_bytes(), width, interrupt: false }).unwrap();
        tree::assert_invariants(&t.root);
        assert_eq!(t, Text::from(&*s));
    }

    #[test]
    fn streaming_utf8_errors_match_std(s in arb_text(200), at in any::<usize>(), byte in any::<u8>()) {
        let mut bytes = s.into_bytes();
        if bytes.is_empty() { bytes.push(byte); } else { let i = at % bytes.len(); bytes[i] = byte; }
        match std::str::from_utf8(&bytes) {
            Ok(s) => { let t = Text::from_reader(bytes.as_slice()).unwrap(); assert_eq!(t.to_string(), s); tree::assert_invariants(&t.root); }
            Err(_) => assert_eq!(Text::from_reader(bytes.as_slice()).unwrap_err().kind(), io::ErrorKind::InvalidData),
        }
    }
}

#[test]
fn empty_cr_newline_and_clipping() {
    for s in ["", "\n", "\n\n", "a\r\nb\r", "😀", "a\n🦀\n", "no newline"] {
        check_view(Text::from(s).as_slice(), s);
    }
    let t = Text::from("😀a\n");
    assert_eq!(t.clip_utf16(1), 0);
    assert_eq!(
        t.clip_point_utf16(PointUtf16 { row: 0, column: 1 }),
        PointUtf16::default()
    );
    assert_eq!(Text::new(), Text::default());
}

#[test]
fn invalid_inputs_panic_before_mutation() {
    let text = Text::from("a😀\n");
    for f in [
        |t: &Text| t.byte_to_char(2),
        |t: &Text| t.byte_to_utf16(2),
        |t: &Text| t.byte_to_line(2),
        |t: &Text| t.char_to_byte(5),
        |t: &Text| t.utf16_to_byte(2),
        |t: &Text| t.line_to_byte(2),
        |t: &Text| t.char_at(6) as usize,
        |t: &Text| t.byte_at(6) as usize,
        |t: &Text| t.point_to_byte(Point { row: 0, column: 6 }),
        |t: &Text| t.point_utf16_to_byte(PointUtf16 { row: 0, column: 2 }),
        |t: &Text| t.next_grapheme_boundary(2),
        |t: &Text| t.prev_grapheme_boundary(7),
        |t: &Text| t.slice(2..5).len_bytes(),
        |t: &Text| t.chunk_at_byte(7).1,
        |t: &Text| {
            t.point_to_byte(Point {
                row: usize::MAX,
                column: 0,
            })
        },
        |t: &Text| {
            t.point_utf16_to_byte(PointUtf16 {
                row: 0,
                column: usize::MAX,
            })
        },
    ] {
        assert!(std::panic::catch_unwind(|| f(&text)).is_err());
    }
    for edits in [
        vec![Edit {
            range: 2..5,
            text: "",
        }],
        vec![
            Edit {
                range: 0..1,
                text: "ok",
            },
            Edit {
                range: 6..7,
                text: "",
            },
        ],
        vec![Edit {
            range: Range { start: 3, end: 1 },
            text: "",
        }],
        vec![
            Edit {
                range: 1..5,
                text: "",
            },
            Edit {
                range: 1..1,
                text: "x",
            },
        ],
        vec![
            Edit {
                range: 5..5,
                text: "",
            },
            Edit {
                range: 0..0,
                text: "x",
            },
        ],
    ] {
        let mut t = text.clone();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| t.apply_edits(&edits)))
                .is_err()
        );
        assert_eq!(t, text);
    }
    for bad in [
        vec![0xff],
        vec![0xc0, 0xaf],
        vec![0xed, 0xa0, 0x80],
        vec![0xf4, 0x90, 0x80, 0x80],
        vec![0xe4, 0xb8],
    ] {
        assert_eq!(
            Text::from_reader(&*bad).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
    struct Fail;
    impl Read for Fail {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::ErrorKind::PermissionDenied.into())
        }
    }
    assert_eq!(
        Text::from_reader(Fail).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
}

#[test]
fn deep_tree_rebalances_and_collapses() {
    let s = "a😀中\n".repeat(25_000);
    let mut text = Text::from(&*s);
    assert!(text.root.height >= 3);
    let snapshot = text.clone();
    for _ in 0..120 {
        let end = text.floor_char_boundary(text.len_bytes().saturating_sub(900));
        text.delete(17..end.max(17));
        tree::assert_invariants(&text.root);
        if text.len_bytes() < 1000 {
            break;
        }
    }
    text.delete(0..text.len_bytes());
    tree::assert_invariants(&text.root);
    assert_eq!(text.root.height, 0);
    text.insert(0, &s);
    tree::assert_invariants(&text.root);
    assert_eq!(text, snapshot);
    let len = text.len_bytes();
    text.apply_edits(&[
        Edit {
            range: 0..len,
            text: "A",
        },
        Edit {
            range: len..len,
            text: "B",
        },
        Edit {
            range: len..len,
            text: "C",
        },
    ]);
    assert_eq!(text, "ABC");
    tree::assert_invariants(&text.root);
}

#[test]
fn grapheme_context_crosses_many_chunks() {
    for unit in ["\u{301}", "🇦", "\u{600}", "👩\u{200d}", "क\u{94d}"] {
        let s = format!("a{}z", unit.repeat(350));
        let t = Text::from(&*s);
        check_view(t.as_slice(), &s);
        let cut = t.char_to_byte(12);
        let end = t.char_to_byte(t.len_chars() - 12);
        check_view(t.slice(cut..end), &s[cut..end]);
    }
}

#[test]
fn iterators_mix_directions_and_hash_ignores_partition() {
    let s = "hello中😀\n".repeat(300);
    let text = Text::from(&*s);
    let mut chunks = text.chunks();
    let mut head = String::new();
    let mut tail = Vec::new();
    while let Some(c) = chunks.next() {
        head.push_str(c);
        if let Some(c) = chunks.next_back() {
            tail.push(c);
        }
    }
    tail.reverse();
    head.push_str(&tail.concat());
    assert_eq!(head, s);
    assert_eq!(chunks.next(), None);
    assert_eq!(chunks.next_back(), None);
    let mut different = Text::new();
    for c in s.chars() {
        different.insert(different.len_bytes(), c.encode_utf8(&mut [0; 4]));
    }
    tree::assert_invariants(&different.root);
    assert_eq!(text, different);
    assert_eq!(hash(&text), hash(&different));
    assert!(format!("{text:?}").len() < 400);
    let mut lines = text.lines();
    while lines.next().is_some() {}
    assert!(lines.next().is_none());
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Text>();
    send_sync::<TextSlice<'_>>();
}

#[test]
fn unequal_unicode_partitions_do_not_panic_during_equality() {
    let a = Text::from("éa");
    let mut b = Text::from("ae");
    b.replace(1..2, "é");
    assert_eq!(a.len_bytes(), b.len_bytes());
    assert_eq!(a.len_chars(), b.len_chars());
    assert_ne!(a, b);
    assert_ne!(b, a);
    tree::assert_invariants(&b.root);
}

#[cfg(feature = "regex-cursor")]
#[test]
fn regex_crosses_chunk_edge_and_slice_cursor_backtracks() {
    use regex_cursor::{Cursor, Input, engines::meta::Regex};
    let edge = tree::CHUNK * 2 / 3;
    let s = format!(
        "{}世界🦀end{}",
        "x".repeat(edge - 3),
        "z".repeat(2 * tree::CHUNK)
    );
    let text = Text::from(&*s);
    let re = Regex::new("世界🦀end").unwrap();
    let m = re.find(Input::new(ChunkCursor::from(&text))).unwrap();
    assert_eq!(m.range(), edge - 3..edge - 3 + "世界🦀end".len());
    let slice = text.slice(1..s.len() - 1);
    assert_eq!(
        re.find(Input::new(ChunkCursor::new(slice)))
            .unwrap()
            .start(),
        m.start() - 1
    );
    let mut c = ChunkCursor::new(slice);
    assert!(!c.backtrack());
    let mut bytes = c.chunk().to_vec();
    while c.advance() {
        bytes.extend_from_slice(c.chunk());
    }
    let last = (c.offset(), c.chunk().to_vec());
    assert!(!c.advance());
    assert_eq!(last, (c.offset(), c.chunk().to_vec()));
    assert_eq!(bytes, s.as_bytes()[1..s.len() - 1]);
    while c.backtrack() {}
    assert_eq!(c.offset(), 0);
    assert!(!c.backtrack());
    assert_eq!(c.total_bytes(), Some(slice.len_bytes()));
    assert!(c.utf8_aware());
    let empty = text.slice(3..3);
    let mut c = ChunkCursor::new(empty);
    assert!(c.chunk().is_empty());
    assert!(!c.advance());
    assert!(!c.backtrack());
    let mut patched = Text::from("foobarbaz");
    patched.insert(2, "中😀");
    assert_eq!(patched.chunks().count(), 3);
    let re = Regex::new("o中😀o").unwrap();
    let found = re.find(Input::new(ChunkCursor::from(&patched))).unwrap();
    assert_eq!(found.range(), 1..10);
    tree::assert_invariants(&patched.root);
}

#[test]
fn progressive_roots_share_valid_trees_and_stop_on_callback_errors() {
    let source = "abc中😀\n".repeat(600_000);
    let mut saved = Vec::new();
    let text = Text::from_reader_snapshots(
        source.as_bytes(),
        2 * 1024 * 1024,
        std::time::Duration::from_secs(10),
        |snapshot| {
            tree::assert_invariants(&snapshot.root);
            assert_eq!(snapshot.to_string(), &source[..snapshot.len_bytes()]);
            saved.push(snapshot);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(text.to_string(), source);
    assert!(saved.len() >= 3);
    for snapshot in saved {
        tree::assert_invariants(&snapshot.root);
    }
    let mut count = 0;
    let error =
        Text::from_reader_snapshots(source.as_bytes(), 1, std::time::Duration::ZERO, |_| {
            count += 1;
            Err(io::Error::other("stop publication"))
        })
        .unwrap_err();
    assert_eq!(count, 1);
    assert_eq!(error.to_string(), "stop publication");
}
