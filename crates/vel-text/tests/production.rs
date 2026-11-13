//! Exercises the production chunk size, in addition to the small-chunk unit model.
use proptest::prelude::*;
use unicode_segmentation::UnicodeSegmentation;
use vel_text::{Edit, Text};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(40))]
    #[test]
    fn large_unicode_batches_and_slices(repeats in 1500usize..9000, seeds in prop::collection::vec(any::<u16>(), 1..150)) {
        let source = "line 中😀 e\u{301} 👩\u{200d}💻\r\n".repeat(repeats);
        let mut text = Text::from_reader(source.as_bytes()).unwrap();
        let snapshot = text.clone();
        let points: Vec<_> = source.char_indices().map(|(b, _)| b).chain([source.len()]).collect();
        let mut at: Vec<_> = seeds.iter().map(|&n| points[n as usize % points.len()]).collect();
        at.sort_unstable();
        let edits: Vec<_> = at.iter().map(|&n| Edit { range: n..n, text: "🦀\nfoo" }).collect();
        text.apply_edits(&edits);
        let mut model = source.clone();
        for edit in edits.iter().rev() { model.insert_str(edit.range.start, edit.text); }
        prop_assert_eq!(text.to_string(), model.as_str());
        prop_assert_eq!(snapshot.to_string(), source.as_str());
        for &n in at.iter().take(20) {
            let n = text.floor_char_boundary(n);
            let view = text.slice(n..text.len_bytes());
            prop_assert_eq!(view.to_string(), &model[n..]);
            let relative = text.ceil_char_boundary(n + 15) - n;
            prop_assert_eq!(view.byte_to_utf16(relative), model[n..n + relative].encode_utf16().count());
            prop_assert_eq!(view.point_to_byte(view.byte_to_point(relative)), relative);
        }
    }
}

#[test]
fn graphemes_and_regex_use_actual_production_edges() {
    let probe = Text::from("a".repeat(20_000));
    let edge = probe.chunks().next().unwrap().len();
    let source = format!(
        "{}e\u{301}👩🏽\u{200d}💻🇺🇳{}",
        "a".repeat(edge - 1),
        "z".repeat(20_000)
    );
    let text = Text::from(source.as_str());
    let boundaries: Vec<_> = source
        .grapheme_indices(true)
        .map(|(b, _)| b)
        .chain([source.len()])
        .collect();
    for byte in source
        .char_indices()
        .map(|(b, _)| b)
        .filter(|&b| b >= edge - 5 && b < edge + 40)
    {
        assert_eq!(
            text.next_grapheme_boundary(byte),
            *boundaries.iter().find(|&&b| b > byte).unwrap()
        );
        assert_eq!(
            text.prev_grapheme_boundary(byte),
            boundaries.iter().copied().rfind(|&b| b < byte).unwrap()
        );
    }
    #[cfg(feature = "regex-cursor")]
    {
        use regex_cursor::{Input, engines::meta::Regex};
        use vel_text::ChunkCursor;
        let re = Regex::new("e\\u{301}👩🏽\\u{200d}💻🇺🇳").unwrap();
        let found = re.find(Input::new(ChunkCursor::from(&text))).unwrap();
        assert_eq!(found.start(), edge - 1);
        assert!(text.chunks().next().unwrap().len() < found.end());
    }
}

#[test]
fn dense_and_sparse_lf_indexes_cover_production_blocks() {
    for source in [
        "\n".repeat(20_000),
        format!("{}\n{}\n", "x".repeat(7000), "中".repeat(900)),
        (0..900)
            .map(|i| format!("{}\n", "a".repeat(i % 300)))
            .collect::<String>(),
    ] {
        let text = Text::from(source.as_str());
        let mut start = 0;
        for (row, line) in source.split('\n').enumerate() {
            assert_eq!(text.line_to_byte(row), start);
            assert_eq!(text.line_len_bytes(row), line.len());
            assert_eq!(text.byte_to_line(start), row);
            assert_eq!(text.line(row).to_string(), line);
            start += line.len() + 1;
        }
        assert_eq!(text.byte_to_line(source.len()), text.len_lines() - 1);
    }
}

#[test]
fn streaming_unicode_straddles_real_staging_boundary() {
    for padding in 0..4 {
        let source = format!(
            "{}中😀x\n{}",
            "a".repeat(1024 * 1024 - 2 + padding),
            "世界🦀\n".repeat(200)
        );
        let streamed = Text::from_reader(source.as_bytes()).unwrap();
        assert_eq!(streamed, Text::from(source.as_str()));
        assert_eq!(streamed.len_utf16(), source.encode_utf16().count());
    }
}

#[test]
fn parallel_and_serial_readers_agree_across_worker_boundaries() {
    use std::{
        cell::Cell,
        io::{self, Read},
        rc::Rc,
    };
    struct LocalReader<'a> {
        remaining: &'a [u8],
        calls: Rc<Cell<usize>>,
    }
    impl Read for LocalReader<'_> {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let n = self.calls.get() + 1;
            self.calls.set(n);
            if n.is_multiple_of(11) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let len = out.len().min(98_321).min(self.remaining.len());
            out[..len].copy_from_slice(&self.remaining[..len]);
            self.remaining = &self.remaining[len..];
            Ok(len)
        }
    }
    for prefix in [
        2 * 1024 * 1024 - 3,
        2 * 1024 * 1024 - 1,
        2 * 1024 * 1024 + 1,
    ] {
        let input = "x".repeat(prefix) + &"中🦀e\u{301}\n".repeat(750_000);
        let parallel = Text::from_reader(LocalReader {
            remaining: input.as_bytes(),
            calls: Rc::new(Cell::new(0)),
        })
        .unwrap();
        let serial = Text::from_reader_serial(LocalReader {
            remaining: input.as_bytes(),
            calls: Rc::new(Cell::new(0)),
        })
        .unwrap();
        assert_eq!(parallel, serial);
        assert_eq!(parallel.len_bytes(), input.len());
        assert_eq!(parallel.len_chars(), input.chars().count());
        assert_eq!(parallel.len_utf16(), input.encode_utf16().count());
        for at in [0, prefix, prefix + 3, input.len()] {
            assert_eq!(parallel.byte_to_line(at), serial.byte_to_line(at));
            assert_eq!(
                parallel.byte_to_point_utf16(at),
                serial.byte_to_point_utf16(at)
            );
        }
    }
}

#[test]
fn parallel_reader_errors_after_multiple_inflight_blocks() {
    use std::io::{self, Read};
    struct FailAfter {
        remaining: usize,
    }
    impl Read for FailAfter {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            let n = self.remaining.min(out.len());
            out[..n].fill(b'a');
            self.remaining -= n;
            Ok(n)
        }
    }
    assert_eq!(
        Text::from_reader(FailAfter {
            remaining: 9 * 1024 * 1024
        })
        .unwrap_err()
        .kind(),
        io::ErrorKind::PermissionDenied
    );
    let mut invalid = vec![b'a'; 9 * 1024 * 1024];
    invalid.push(0xff);
    type Loader = fn(&[u8]) -> io::Result<Text>;
    let loaders: [Loader; 2] = [|r| Text::from_reader(r), |r| Text::from_reader_serial(r)];
    for load in loaders {
        assert_eq!(
            load(&invalid).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            load(b"\xf0\x9f\xa6").unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
