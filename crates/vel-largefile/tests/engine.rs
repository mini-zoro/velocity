use std::{
    fs::{self, File},
    io::{self, Read, Write},
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
use vel_largefile::*;
use vel_text::Text;

/// Hang guard: stops a wait that would otherwise never end. It is not a latency
/// bound; the test checks the condition it waits for.
const HANG_GUARD: std::time::Duration = std::time::Duration::from_secs(120);

struct Short<'a> {
    bytes: &'a [u8],
    limit: usize,
    interrupted: bool,
}
impl Read for Short<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if !self.interrupted {
            self.interrupted = true;
            return Err(io::ErrorKind::Interrupted.into());
        }
        let n = out.len().min(self.limit).min(self.bytes.len());
        out[..n].copy_from_slice(&self.bytes[..n]);
        self.bytes = &self.bytes[n..];
        Ok(n)
    }
}
fn options() -> LoadOptions {
    LoadOptions {
        chunk_bytes: 17,
        publish_bytes: 31,
        publish_interval: Duration::from_secs(3600),
    }
}
#[test]
fn prefixes_are_valid_monotonic_and_bom_and_seams_are_correct() {
    let source = format!("\u{feff}{}", "中🦀e\u{301}\r\nnext\r".repeat(1000));
    for limit in [1, 2, 3, 7, 17, 8192] {
        let mut snapshots = Vec::new();
        let loaded = stream_utf8(
            Short {
                bytes: source.as_bytes(),
                limit,
                interrupted: false,
            },
            Some(source.len() as u64),
            &options(),
            &Cancellation::default(),
            |s| snapshots.push(s),
        )
        .unwrap();
        let model = &source[3..];
        assert_eq!(loaded.text.to_string(), model);
        assert!(loaded.bom);
        assert_eq!(loaded.progress.read, source.len() as u64);
        assert_eq!(loaded.metrics.bytes, model.len() as u64);
        assert_eq!(loaded.metrics.lines, model.split('\n').count() as u64);
        assert_eq!(
            loaded.metrics.longest_line_bytes,
            model.split('\n').map(str::len).max().unwrap() as u64
        );
        let mut last = 0;
        for snapshot in snapshots {
            assert!(snapshot.text.len_bytes() >= last);
            last = snapshot.text.len_bytes();
            assert_eq!(snapshot.text.to_string(), &model[..last]);
        }
        assert_eq!(last, model.len());
    }
}
#[test]
fn fallback_errors_empty_input_and_cancellation_are_explicit() {
    for bytes in [
        &b"\xff\xfea\0"[..],
        b"\xfe\xff\0a",
        b"abc\xff",
        b"x\xf0\x9f",
    ] {
        assert!(matches!(
            stream_utf8(bytes, None, &options(), &Cancellation::default(), |_| {}),
            Err(LoadError::DecodeRequired)
        ));
    }
    assert!(matches!(
        stream_utf8(
            &b"abc\0def"[..],
            None,
            &options(),
            &Cancellation::default(),
            |_| {}
        ),
        Err(LoadError::Binary)
    ));
    let cancel = Cancellation::default();
    let result = stream_utf8(
        &b"abc".repeat(10_000)[..],
        None,
        &options(),
        &cancel,
        |_| cancel.cancel(),
    );
    assert!(matches!(result, Err(LoadError::Cancelled)));
    let mut count = 0;
    let empty = stream_utf8(&b""[..], None, &options(), &Cancellation::default(), |_| {
        count += 1
    })
    .unwrap();
    assert!(empty.text.is_empty());
    assert_eq!(count, 1);
    assert_eq!(empty.metrics.lines, 1);
    let mut invalid = b"x".repeat(3 * 1024 * 1024);
    invalid.extend_from_slice(b"\xff");
    let mut count = 0;
    assert!(matches!(
        stream_utf8(
            &invalid[..],
            None,
            &options(),
            &Cancellation::default(),
            |_| count += 1
        ),
        Err(LoadError::DecodeRequired)
    ));
    assert!(count > 0); // Provisional prefixes must be discarded on a later failure.
    struct Failing {
        left: usize,
    }
    impl Read for Failing {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            if self.left == 0 {
                return Err(io::ErrorKind::Other.into());
            }
            let n = out.len().min(self.left);
            out[..n].fill(b'a');
            self.left -= n;
            Ok(n)
        }
    }
    assert!(matches!(
        stream_utf8(
            Failing { left: 9000 },
            None,
            &options(),
            &Cancellation::default(),
            |_| {}
        ),
        Err(LoadError::Io(_))
    ));
}
static SERIAL: AtomicUsize = AtomicUsize::new(0);
fn path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "velocity-r11-test-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ))
}
#[test]
fn task_mailbox_coalesces_and_hex_maps_sparse_files() {
    let path = path();
    {
        let mut file = File::create(&path).unwrap();
        file.write_all(&b"text\n".repeat(10000)).unwrap();
    }
    let task = load_utf8(&path, options()).unwrap();
    let loaded = task.result.recv_timeout(HANG_GUARD).unwrap().unwrap();
    let last = task.try_snapshot().unwrap();
    assert_eq!(last.text, loaded.text);
    assert!(task.try_snapshot().is_none());
    assert_eq!(task.progress().read, 50_000);
    drop(task);
    drop(loaded);
    drop(last);
    {
        let mut file = File::create(&path).unwrap();
        file.write_all(b"A\0\xff hello world!?").unwrap();
        file.set_len(4_000_000_003).unwrap();
    }
    // SAFETY: the test owns this uniquely named file; no writer exists while mapped.
    let hex = unsafe { HexView::open(&path) }.unwrap();
    assert_eq!(hex.len_rows(), 250_000_001);
    let first = hex.row(0).unwrap();
    assert_eq!(&first.ascii[..4], b"A.. ");
    let last = hex.row(hex.len_rows() - 1).unwrap();
    assert_eq!(last.offset, 4_000_000_000);
    assert_eq!(last.bytes, [0, 0, 0]);
    assert!(hex.row(u64::MAX).is_none());
    drop(hex);
    fs::write(&path, []).unwrap();
    // SAFETY: the empty test file remains unchanged until this view is dropped.
    let hex = unsafe { HexView::open(&path) }.unwrap();
    assert!(hex.is_empty());
    assert!(hex.row(0).is_none());
    drop(hex);
    fs::remove_file(path).unwrap();
}
#[test]
fn policy_thresholds_are_inclusive_and_overflow_safe() {
    let p = LargeDocumentPolicy::default();
    assert_eq!(
        p.evaluate(DocumentMetrics {
            bytes: 10,
            lines: 1,
            longest_line_bytes: 10
        })
        .highlighting,
        HighlightMode::Full
    );
    // Huge documents keep every expensive feature off, wrapping included.
    for m in [
        DocumentMetrics {
            bytes: p.viewport_bytes,
            lines: 1,
            longest_line_bytes: 1,
        },
        DocumentMetrics {
            bytes: 1,
            lines: p.viewport_lines,
            longest_line_bytes: 1,
        },
    ] {
        let f = p.evaluate(m);
        assert_eq!(f.highlighting, HighlightMode::Viewport);
        assert!(!f.indexing && !f.word_wrap && !f.spell_check && !f.diff_against_disk);
    }
    // A lone long line in a modest document still wraps: `word_wrap: auto` exists to wrap
    // lines wider than the viewport, and an unwrapped 100k-column line shows nothing.
    let f = p.evaluate(DocumentMetrics {
        bytes: 1,
        lines: 1,
        longest_line_bytes: p.long_line_bytes,
    });
    assert_eq!(f.highlighting, HighlightMode::Viewport);
    assert!(f.word_wrap);
    assert!(!f.indexing && !f.spell_check && !f.diff_against_disk);
    let f = p.evaluate(DocumentMetrics {
        bytes: u64::MAX,
        lines: u64::MAX,
        longest_line_bytes: u64::MAX,
    });
    assert!(f.minimap_stride > 1);
    assert_eq!(detect_content(b"\xff\xfe\0\0"), ContentKind::DecodeRequired);
    assert_eq!(detect_content(b"abc\xf0\x9f"), ContentKind::Utf8);
}
fn check_line(index: &LongLineIndex, source: &str, tab: usize) {
    let mut boundaries: Vec<_> = source.grapheme_indices(true).map(|(i, _)| i).collect();
    boundaries.push(source.len());
    assert_eq!(index.len_bytes(), source.len());
    assert_eq!(index.len_graphemes(), boundaries.len() - 1);
    let mut column = 0;
    for (g, pair) in boundaries.windows(2).enumerate() {
        let sampled = g % (boundaries.len() / 128).max(1) == 0 || g + 2 == boundaries.len();
        if sampled {
            assert_eq!(index.grapheme_to_byte(g), pair[0]);
            assert_eq!(index.byte_to_grapheme(pair[0]), g);
            assert_eq!(index.byte_to_column(pair[0]), column);
            assert_eq!(index.next_grapheme_boundary(pair[0]), pair[1]);
            assert_eq!(index.prev_grapheme_boundary(pair[1]), pair[0]);
        }
        let cluster = &source[pair[0]..pair[1]];
        let next = if cluster == "\t" {
            column + tab - column % tab
        } else {
            column + cluster.width()
        };
        if sampled {
            for c in column..next {
                assert_eq!(index.column_to_byte(c), pair[0]);
            }
        }
        column = next;
    }
    assert_eq!(index.display_width(), column);
    assert_eq!(index.column_to_byte(column), source.len());
    assert_eq!(
        index.visible_window(column + 100, 200),
        source.len()..source.len()
    );
}
#[test]
fn unicode_columns_tabs_clusters_and_edits_match_a_flat_model() {
    let mut source = "abc\t中e\u{301}👩🏽\u{200d}💻🇺🇳! \t".repeat(400);
    let mut index = LongLineIndex::new(Text::from(format!("header\n{source}\nlast")), 1, 4);
    check_line(&index, &source, 4);
    let mut seed = 123u64;
    for i in 0..70 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let at = source.floor_char_boundary(seed as usize % (source.len() + 1));
        let end = source.ceil_char_boundary((at + i % 7).min(source.len()));
        let inserted = ["x", "\u{301}", "🇺", "\u{200d}", "\t", "中", ""][i % 7];
        source.replace_range(at..end, inserted);
        index.replace(at..end, inserted);
        assert_eq!(index.text().line(1).to_string(), source);
        check_line(&index, &source, 4);
    }
}
#[test]
fn word_jumps_and_ri_propagation_cross_many_blocks() {
    let source = format!("{} {}!!", "a".repeat(500_000), "中".repeat(5000));
    let mut index = LongLineIndex::new(Text::from(source.as_str()), 0, 4);
    assert_eq!(index.next_word_boundary(100), 500_000);
    assert_eq!(index.prev_word_boundary(499_999), 0);
    assert_eq!(
        index.prev_word_boundary(index.len_bytes()),
        index.len_bytes() - 2
    );
    index.replace(250_000..250_000, " ");
    assert_eq!(index.next_word_boundary(100), 250_000);
    let mut source = "🇺".repeat(6000);
    let mut index = LongLineIndex::new(Text::from(source.as_str()), 0, 8);
    source.insert(4, '🇳');
    index.replace(4..4, "🇳");
    check_line(&index, &source, 8);
    index.replace(0..index.len_bytes(), "");
    check_line(&index, "", 8);
    index.replace(0..0, "hello");
    check_line(&index, "hello", 8);
}

#[test]
fn a_giant_combining_cluster_has_constant_size_query_metadata() {
    let source = format!("{}e{}z", "a".repeat(4095), "\u{301}".repeat(100_000));
    let index = LongLineIndex::new(Text::from(source.as_str()), 0, 4);
    assert_eq!(index.len_graphemes(), 4097);
    assert_eq!(index.byte_to_grapheme(4100), 4095);
    assert_eq!(index.byte_to_column(4100), 4095);
    assert_eq!(index.grapheme_to_byte(4096), source.len() - 1);
    assert_eq!(index.visible_window(4095, 1), 4095..source.len() - 1);
}
