# Spec: `vel-text` — the text storage crate

Status: P0 / lane B. Owner: core-engine lane. Location: `crates/vel-text/`.

`vel-text` is the persistent text data structure under every buffer in Velocity. Everything above
it (selections, undo, syntax highlighting, search, layout, LSP positions) converts between
coordinates through this crate, so it must be **fast, correct for all Unicode input, and cheap to
snapshot**.

## Background and measured baseline

From `spikes/RESULTS.md` (M1 Pro, release, ASCII corpus):

| | crop 0.4.3 | ropey 1.6.1 |
|---|---:|---:|
| load 1 GB (median) | 257 ms | 742 ms |
| 1M `byte→line` lookups, 1 GB | 578 ms | 1,172 ms (char→line) |
| 10k cursors, one insert each, one transaction (1 GB) | 1.86 ms | 2.53 ms |
| 100k cursors, same | 22.3 ms | 28.2 ms |
| clone | ~2 ns | ~2 ns |

crop has no char / UTF-16 metrics; ropey is slower. We implement our own rope so that we control
the metrics, the batch-edit path and the memory layout. Studying crop, ropey, xi-rope and Zed's
`rope`/`sum_tree` designs is encouraged (all are MIT/Apache); if you adapt code, credit it in a
comment and in `crates/vel-text/README.md`.

## Model

- Content is always **valid UTF-8**. Decoding other encodings and BOM handling live in a higher
  layer.
- Lines are separated by `'\n'` only. Like Sublime Text, the buffer layer normalizes CRLF/CR to LF
  on load and restores the file's line endings on save, so `vel-text` never sees `'\r\n'` as a
  unit (a lone `'\r'` is an ordinary character).
- The line count is `newlines + 1` (an empty text has 1 line; a text ending in `'\n'` has an empty
  last line).
- Primary offset unit: **byte offset** (`usize`). Secondary metrics that must be O(log n):
  Unicode scalar values (`char`), **UTF-16 code units** (for LSP and the macOS text input APIs),
  and lines. Points: `Point { row, column }` where `column` is in bytes, and
  `PointUtf16 { row, column }` where `column` is in UTF-16 units.

## Required API (names may be refined; behavior may not)

```rust
pub struct Text;            // persistent B-tree rope; Clone is O(1) (Arc-shared nodes)
pub struct TextSlice<'a>;   // zero-copy view of a byte range
pub struct Edit<'a> { pub range: std::ops::Range<usize>, pub text: &'a str }

impl Text {
    pub fn new() -> Self;
    pub fn from_str(s: &str) -> Self;
    /// Streaming load. Must not hold a second full copy of the input in memory.
    pub fn from_reader(r: impl std::io::Read) -> std::io::Result<Self>; // invalid UTF-8 -> io::ErrorKind::InvalidData

    pub fn len_bytes(&self) -> usize;
    pub fn len_chars(&self) -> usize;
    pub fn len_utf16(&self) -> usize;
    pub fn len_lines(&self) -> usize;

    // Coordinate conversions, all O(log n). Out-of-range or non-boundary inputs panic
    // with a clear message; `clip_*` variants exist for callers that need clamping.
    pub fn byte_to_char(&self, byte: usize) -> usize;
    pub fn char_to_byte(&self, ch: usize) -> usize;
    pub fn byte_to_utf16(&self, byte: usize) -> usize;
    pub fn utf16_to_byte(&self, u: usize) -> usize;        // must be on a scalar boundary
    pub fn byte_to_line(&self, byte: usize) -> usize;
    pub fn line_to_byte(&self, line: usize) -> usize;      // start of line; line == len_lines() allowed? -> no, panic
    pub fn byte_to_point(&self, byte: usize) -> Point;
    pub fn point_to_byte(&self, p: Point) -> usize;
    pub fn byte_to_point_utf16(&self, byte: usize) -> PointUtf16;
    pub fn point_utf16_to_byte(&self, p: PointUtf16) -> usize;
    pub fn line_len_bytes(&self, line: usize) -> usize;    // excluding the '\n'
    pub fn is_char_boundary(&self, byte: usize) -> bool;
    pub fn floor_char_boundary(&self, byte: usize) -> usize;
    pub fn ceil_char_boundary(&self, byte: usize) -> usize;
    pub fn next_grapheme_boundary(&self, byte: usize) -> usize;   // extended grapheme clusters,
    pub fn prev_grapheme_boundary(&self, byte: usize) -> usize;   // correct across chunk edges

    // Access
    pub fn slice(&self, range: std::ops::Range<usize>) -> TextSlice<'_>;
    pub fn line(&self, line: usize) -> TextSlice<'_>;             // without the '\n'
    pub fn char_at(&self, byte: usize) -> char;
    pub fn byte_at(&self, byte: usize) -> u8;
    pub fn chunks(&self) -> impl DoubleEndedIterator<Item = &str>;   // also on TextSlice
    pub fn chars(&self) -> impl DoubleEndedIterator<Item = char>;
    pub fn bytes(&self) -> impl DoubleEndedIterator<Item = u8>;
    pub fn lines(&self) -> impl Iterator<Item = TextSlice<'_>>;
    pub fn chunk_at_byte(&self, byte: usize) -> (&str, usize);     // chunk + its start offset

    // Editing
    pub fn replace(&mut self, range: std::ops::Range<usize>, text: &str);
    pub fn insert(&mut self, at: usize, text: &str);
    pub fn delete(&mut self, range: std::ops::Range<usize>);
    /// Applies many edits as one operation. `edits` are sorted by `range.start`, non-overlapping
    /// (touching is allowed), and expressed in coordinates of the text *before* the call.
    /// Must be substantially faster than applying them one by one (single pass, rebuilds only
    /// touched leaves). Returns nothing; callers compute new selections themselves.
    pub fn apply_edits(&mut self, edits: &[Edit<'_>]);
}
// Also: TextSlice has the same read-only API (len_*, conversions relative to the slice,
// chunks/chars/bytes/lines, to_string). Text: Clone, Default, Debug (truncated), Display,
// PartialEq<str>, PartialEq<Text> (content equality, fast when nodes are shared), From<&str>,
// From<String>, Hash.
```

Search integration: behind a default-on cargo feature `regex-cursor`, implement
[`regex_cursor::Cursor`](https://docs.rs/regex-cursor) for a chunk cursor over `Text`/`TextSlice`
so `regex-cursor` / `regex-automata` can search the rope without copying. Include a test that a
regex match spanning a chunk boundary is found.

## Performance budget (Apple M1 Pro, `--release`)

Measure with a benchmark binary in `crates/vel-text/benches/perf.rs` (`harness = false`) that can
read the corpora in `spikes/data/` (`text-100mb.txt`, `text-1gb.txt`; ASCII) and also generates a
**non-ASCII corpus** (mix of CJK, emoji incl. astral-plane chars, combining marks, ~30% non-ASCII
by bytes) of 100 MB. Report median and P95 of ≥5 runs, peak RSS via `getrusage`. Compare against
`crop` and `ropey` as dev-dependencies in the same binary.

| Operation | Target |
|---|---|
| `from_reader` 1 GB ASCII | ≤ 300 ms, peak RSS ≤ 1.15 × file size |
| `from_reader` 100 MB non-ASCII | ≤ 80 ms |
| `clone` | ≤ 50 ns |
| Any single conversion (byte↔char/utf16/line/point), random, 1 GB | ≤ 600 ns average (crop: 578 ns for byte→line) |
| Random single-char `insert` / `delete`, 1 GB | ≤ 2 µs average |
| `apply_edits`, 10k insertions of `"foo"` at the start of the first 10k lines | ≤ 1 ms |
| `apply_edits`, 100k insertions (same pattern) | **≤ 8 ms** (crop one-by-one: 22 ms) |
| `apply_edits`, 100k edits spread uniformly over a 1 GB text | ≤ 15 ms |
| `chunks()` full iteration, 1 GB | ≥ 5 GB/s |
| `lines()` full iteration, 1 GB | ≥ 1 GB/s |
| Memory overhead at rest (1 GB) | ≤ 10% above content size |

If a target turns out to be unreachable, say so with numbers and the reason; do not quietly relax
it.

## Correctness requirements

- **Property tests** (`proptest`) against a `String` model for every public operation, with
  generators that produce ASCII, multi-byte, astral-plane (UTF-16 surrogate pairs), combining
  sequences, ZWJ emoji, lone `'\r'`, many/zero newlines, empty strings, and texts large enough to
  force a multi-level tree (use a small chunk size under `cfg(test)` if needed).
- `apply_edits` must equal applying the same edits sequentially from last to first.
- Invariant checker (`#[cfg(test)] fn assert_invariants`) for tree balance, chunk sizes, summary
  consistency and chunk UTF-8 boundaries; run it after every operation in property tests.
- Grapheme boundaries must agree with `unicode-segmentation` on the flattened string.
- No `unsafe` unless a benchmark proves the need; every `unsafe` block has a `// SAFETY:` comment.
- `cargo clippy -p vel-text --all-targets -- -D warnings` and `cargo test -p vel-text` pass;
  every public item has rustdoc.

## Out of scope

Undo history, selections, encodings, line-ending conversion, file IO beyond `from_reader`,
memory-mapped loading (a later "huge file" mode), syntax data.

## Deliverables

1. `crates/vel-text/` — library, tests, `benches/perf.rs`, `README.md` (design: node layout,
   chunk size, summary fields, batch-edit algorithm, credits).
2. `crates/vel-text/PERF.md` (Chinese) — the budget table with measured numbers next to each
   target, comparison with crop/ropey, method, machine state, and conclusions.
