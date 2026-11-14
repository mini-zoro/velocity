# vel-index

Scope-selected project definitions/references and completion statistics, with a background worker
pool and an append-only cache. Locations use zero-based rows, UTF-8 byte columns and original
byte ranges; transformed names never change source coordinates. `SymbolKind` follows the public
scope naming conventions, with `Other` for unclassified/custom scopes.

```rust,no_run
use std::{path::Path, sync::Arc};
use vel_index::{IndexOptions, ProjectIndex};
use vel_syntax::{MetadataSet, SyntaxSet};

# fn attach(root: &Path, syntaxes: Arc<SyntaxSet>, metadata: Arc<MetadataSet>) -> Result<ProjectIndex, vel_index::Error> {
let index = ProjectIndex::open(root, syntaxes, metadata, IndexOptions::default())?;
// Query on a navigation worker; the UI consumes the owned locations.
let definitions = index.lookup_definition("render");
let references = index.lookup_references("render");
let matches = index.fuzzy_symbols("rw", 50);
let progress = index.status(); // fraction(), pending_files and bounded logs
# let _ = (definitions, references, matches, progress);
# Ok(index)
# }
```

For highlighted open buffers, reuse their complete scopes and symbols on the analysis worker:

```rust
use std::path::Path;
use vel_highlight::{ScopeSnapshot, SymbolList};
use vel_index::{ProjectIndex, analyze_scopes};

# fn publish(index: &ProjectIndex, path: &Path, scopes: &ScopeSnapshot, symbols: &SymbolList) -> Result<(), vel_index::Error> {
let analysis = analyze_scopes(scopes, symbols)?; // words/stats and locations; no syntax parse
index.update_buffer_analysis(path, scopes.version.0, scopes.text.clone(), analysis);
# Ok(())
# }
```

This preserves unsaved-buffer precedence while avoiding another parser. A counting matcher
backend test verifies that submitting prepared analysis does not build any syntax context.
For buffers without a highlighter, send `index.update_buffer(path, change_id.0, text.clone())`.
Choose one publication path per buffer/revision. Repeated updates coalesce for 15 ms by default; older/equal buffer revisions are rejected.
Unsaved text takes precedence over disk events, including while older tasks are in flight.
`close_buffer(path)` restores the disk version. `reindex_file(path)` provides an explicit watcher
fallback and handles deletions; `rescan()` refreshes directory/ignore changes. `shared_index()`
feeds vel-complete directly. Word counts, bigrams/trigrams and directly trailing punctuation are
replaced with their file's symbols, so edits/deletions remove old contributions.

When watching is enabled, 500 ms metadata checks supplement notify, including silent watcher
registrations, file changes, directory changes and ignore-file updates. Absolute buffer paths
under the original root alias map to its canonical root without UI-thread file I/O.

`index_workers == 0` chooses up to eight available cores. Workers use macOS utility QoS. Paths are
walked with ignore, without following symlinks; exclusions match project-relative paths,
basenames and ancestors. `.gitignore`/global git excludes obey `index_exclude_gitignore`;
`.ignore` stays active. Unknown syntaxes, NUL/invalid UTF-8 and files above `max_file_size`
(default 2 MiB) are skipped. Detection includes first-line matches. Explicit buffer updates use
the same exclusions and limits. Cancellation invalidates the service immediately and workers
stop between files; a current parser/regex call is cooperative. Cache replay checks cancellation between records
and preserves the untouched journal when cancelled. Drop never joins a slow parser.

The cache defaults to the platform user's cache directory (`Library/Caches/velocity/index` on
macOS), with an override in `IndexOptions::cache_dir`. Its key includes the canonical project,
serialized syntax definitions, `MetadataSet::fingerprint()` and options. Each journal frame has
a length and BLAKE3 checksum. An incomplete/corrupt tail is truncated to its last complete frame;
missing cache entries are then rebuilt from disk. Restart replays symbol/statistic records and
checks file length/mtime, without parsing unchanged files. Unsaved overlays are never persisted.
Use `flush()` on a background/shutdown thread to await pending work and sync the journal.
Journal space and interned retired names/sequence keys are reclaimed on cache rebuild, rather
than on every edit. Changes that preserve both file length and modification timestamp require
an explicit `reindex_file`; this is also the fallback when notify is unavailable.

`Index` is the device-free in-memory API. `analyze()` applies all branch corrections before
metadata extraction, honoring `showInIndexedSymbolList`, `symbolIndexTransformation` and
`showInIndexedReferenceList`. `replace_file()` accepts an owned analysis and removes prior file
contributions. Exact queries follow interned name postings. Fuzzy queries use bounded top-K
heaps, safe character masks, query-local common-prefix scoring, and up to eight scoring threads
for large vocabularies. The prefix optimization is tested against the scalar scorer. It does
not reuse or modify lane A's path-biased vel-search matcher. N-gram counters use compact context
posting chains; completion queries gather context counts once before scanning words. Name,
location and n-gram arrays use stable allocation chunks, and spelling storage uses bounded
chunks, avoiding allocator-retained growth copies at million-symbol scale.

Measurements, limits and reproduction are in [ROUND6.md](ROUND6.md). Run:

```sh
cargo test --release -p vel-index
cargo run --release -p vel-index --example index_bench -- prepare
cargo run --release -p vel-index --example index_bench -- queries
cargo run --release -p vel-index --example index_bench -- edit
cargo run --release -p vel-index --example index_bench -- corpus /path/to/10000/rust/files
```

Behavioral references: public [indexing documentation](https://www.sublimetext.com/docs/indexing.html)
and [API reference](https://www.sublimetext.com/docs/api_reference.html), plus public permissively
licensed package preferences. No application-private resources or binaries were inspected.
