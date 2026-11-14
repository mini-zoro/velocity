# vel-complete

Prepared, scope-aware completions over immutable view snapshots. `CompletionList` contains the
view/version/generation, original prefix replacement ranges, presentation rows and Tab/cycle
policies. Each row carries trigger, annotation, insertion text, text/snippet format, kind and
minihtml details. Queries never parse syntax or mutate a buffer.

```rust
use vel_buffer::ChangeId;
use vel_complete::{CompletionEngine, ViewSnapshot};
use vel_highlight::ScopeSnapshot;
use vel_text::Text;

# fn query(scopes: &ScopeSnapshot) -> Result<(), vel_complete::Error> {
let mut engine = CompletionEngine::new();
engine.add_completions(r#"{
  "scope": "source.rust",
  "completions": [{"trigger":"func", "contents":"fn ${1:name}($2) { $0 }",
                   "kind":"function", "annotation":"function"}]
}"#)?;
let view = ViewSnapshot::new(1, ChangeId(7), Text::from("fun"));
let list = engine.query(&view, &[3], scopes);
// Accept only list.view_id/version matching the live buffer/view.
// Replace list.replace_ranges with the chosen item's completion.
# let _ = list;
# Ok(())
# }
```

For asynchronous use, retain an `Arc<CompletionEngine>` and create
`CompletionService::new(engine.clone())`. Submit
`service.query(view_snapshot, carets, Arc::new(scope_snapshot))`; poll with `service.poll()`.
New requests/cancel invalidate queued, scanning and already-published generations. Stale or
unavailable scopes suppress results; resubmit when the matching HighlightService batch arrives.
Cancellation is checked every 256 source candidates. Resource loading and vocabulary preparation
belong on a publisher/worker, outside typing/layout.

`ViewSnapshot::new` prepares current-buffer words. Reuse `Arc<BufferWords>` across caret-only
queries, and publish refreshed words after edits. `set_open_buffer(view_id, words)` and
`remove_open_buffer` update other open-buffer sources while a service is active. Attach the live
project with `engine.set_index(project.shared_index())`. `add_completions` accepts JSON/JSONC,
strings, rich objects, legacy tab annotations, custom kind triples, details and scope selectors.
`add_snippet(SnippetFile::parse(xml)?)` supplies `.sublime-snippet` tab triggers. Lane A's resource
loader should call these once per resource version and rebuild an engine when resources change.

Ranking combines Unicode fuzzy quality, prefix/word/CamelCase starts, current-buffer caret
proximity, frequency, bounded commit history, and preceding project word-sequence counts. Exact
snippet/tab triggers rank first. `record_commit(item)` records recency. Contextual project rows
include observed trailing punctuation and scope-derived definition kinds. Equal-ranked sources
retain source order; `auto_complete_preserve_order` selects that order directly after exact
triggers. These are clean-room ranking rules, not a claim of proprietary numeric score identity.

`CompletionSettings` mirrors all settings in the round-six spec, including index/history,
snippet inclusion and typing inclusion, selectors/character triggers, delay/size, fields, Tab and
cycle policies. Convert JSON millisecond settings to `Duration`. Manual/Tab requests bypass
automatic delay, size and selector restrictions. Typing requests use `Invocation::Typing` with
the input scalar and elapsed idle time. Explicit triggers override the main selector; active
fields suppress automatic popup only when commit-on-Tab is enabled and with-fields is disabled.
`auto_complete_when_likely` requires a word prefix or explicit trigger. The UI owns the delay
timer, popup lifetime, Enter/Tab dispatch and navigation wrapping.

Apply literal rows in one buffer transaction. For snippet rows, parse `item.completion` with
vel-snippet, expand at `list.replace_ranges` (removing the typed prefixes), apply the expansion
edits together, and retain its Session for field navigation/mirrors. A selection transaction
must not query a different buffer's scope snapshot merely because its change ID happens to match.

Performance evidence is in [../vel-index/ROUND6.md](../vel-index/ROUND6.md). The 100k-word fixture
includes project trigrams and per-word punctuation; preparation is excluded from query timing.

```sh
cargo test --release -p vel-complete
cargo run --release -p vel-complete --example complete_bench
```

References: public [completions](https://www.sublimetext.com/docs/completions.html) and
[API reference](https://www.sublimetext.com/docs/api_reference.html). No private completion assets
or binaries were used.
