# vel-search

Device-free navigation and search. This is original clean-room code using public documentation,
not Sublime binaries/resources. No dependency on the new syntax/highlight/color-scheme crates.

`fuzzy::Matcher` prepares Unicode lowercase labels, character masks and word/CamelCase starts
once. Ranking favors filename stems, then filenames, then path-only matches; adjacency and word
starts earn bonuses, long paths lose points, and recency supplies a bounded boost. Top K uses
partial selection followed by sorting only K rows. Prefix extensions narrow the complete prior
match set (not merely the previous top K); backspace/non-prefix changes rescore the snapshot.
`fuzzy::Worker` serializes queries, discards queued stale generations and checks cancellation
every 256 candidates. A generation check also rejects results already in the output channel.
These are explicit ranking choices, not a claim of exact proprietary scores.

Golden fixture, in order: `src/main.rs`, `tests/main_test.rs`, `docs/main/guide.md`,
`src/market.rs`, `src/MapReader.rs`, `src/你好.rs`, `main.rs`.

| Query | Expected top results |
| --- | --- |
| `main` | `main.rs`, `src/main.rs`, `tests/main_test.rs` |
| `mr` | `src/MapReader.rs`, `src/market.rs`, `main.rs` |
| `你好` | `src/你好.rs` |

Tests additionally compare incremental/backspace results with fresh rescoring and check MRU
ordering and stale-query cancellation. No-match and empty-query paths are part of the benchmark.

`project::Index` builds a complete sorted file snapshot with ignore's parallel walker and prepares
fuzzy candidates on that worker. Recursive notify events rebuild snapshots; a directory/ignore-file
mtime fallback handles restricted notification sessions. Directory expansion is lazy, on separate
workers. Global/per-folder file, folder and binary exclusions share `Rules`; `.gitignore`, `.ignore`
and git exclude rules are respected. Binary patterns suppress Goto/Find in Files candidates while
binary-named files can remain visible in the tree, matching the public project-setting meaning.
Directory symlink following is opt-in. Invalid globs/IO failures are surfaced.

`goto::Query` parses `file`, `file:line`, `file:line:col`, `file@symbol`, `#term`, `:line`, `@symbol`;
coordinates are one-based scalar positions. `goto::Symbols` is the later syntax-metadata seam.
Velocity offers fuzzy word rows for `#term` and invokes that provider for symbols when installed.

`text::Search` uses vel-text's `ChunkCursor` with regex-cursor, retaining rope context across
chunks and selection/viewport spans. Only matched substrings are copied when expanding `$1`,
`${name}`, `$name`, `$$` or applying preserve-case replacements. `text::apply` makes one
transaction and maps every selection. Regex supports Rust regex syntax, Unicode classes/word
boundaries, multiline anchors, named/numbered groups and inline flags. Differences from Sublime's
Boost-like Perl syntax: no lookaround, pattern backreferences, recursion, atomic groups or
possessive quantifiers. Unsupported expressions show errors; they are never silently rewritten.
An empty find query clears highlights. Zero-width expressions advance at UTF-8 boundaries.

Full highlighting uses one reusable worker over O(1) snapshots. Viewport search is synchronous;
full results are accepted only for the current query generation, buffer identity and revision.
Navigation waits for that pass if necessary. Cancellation occurs between regex matches/scans;
an individual no-match engine scan finishes before the next generation runs.

`files::Task` uses ignore + grep-searcher on at most four walker threads with a bounded channel
and cancellation-aware backpressure. Where accepts comma-separated folders/files, `<project>`,
`<open files>`, include globs and `-exclude` globs; quoted paths can contain spaces. Commas inside
paths/globs are not supported. Open-file snapshots include unsaved edits and take precedence over
disk copies. Search is line-oriented for disk files; multiline matches belong to document search.
Results contain file headers, numbered match/context lines, gap rows and UTF-8 highlight spans.
Parallel streams can repeat a file header when their output interleaves.

`FileChange::prepare` reads/expands without writing. After native confirmation, `commit` rechecks
content/encoding/EOL, saves original bytes as `NAME.velocity-backup-TIMESTAMP`, and uses vel-fileio's
permission-preserving atomic save. Changed files fail instead of overwriting a stale preview.
The batch is atomic per file, not across files; completed backups remain if a later file fails.
Velocity requires dirty target tabs to be saved before disk replacement. Restore a backup by
copying its bytes over the original with the app closed, then reopening. No backup is deleted
automatically. Workspace-session persistence is deferred by the spec.

```sh
cargo test -p vel-search
cargo run --release -p vel-search --example search_bench
```

The example exits automatically and reports JSONL: 100k prepared paths/top 100, 180 synchronous
queries including prefix/reset/backspace, 90 worker queue+scoring queries, five literal searches
over exactly 100,000,000 bytes and 100 sixty-line viewport searches. Indexing/text construction
is outside timing; there are no full-buffer copies in timed searches. Budgets are 10/200/5 ms.
Final observations and hardware limits are in velocity/RESULTS.md. GUI scanout is a separate
reviewer measurement; CPU results do not establish an input-to-display budget.

Sources: [public API](https://www.sublimetext.com/docs/api_reference.html),
[projects](https://www.sublimetext.com/docs/projects.html),
[file patterns](https://www.sublimetext.com/docs/file_patterns.html),
[ignore](https://docs.rs/ignore/latest/ignore/struct.WalkBuilder.html),
[grep-searcher](https://docs.rs/grep-searcher/latest/grep_searcher/).
