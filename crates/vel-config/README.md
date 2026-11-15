# vel-config

Clean-room resource support for Velocity. No application-bundled Sublime Default files were
read or copied. Formats and functional names follow the public settings, key bindings,
menus and packages documentation linked from the round specification.

`json::Document` accepts UTF-8 BOM, line/block comments and trailing commas. `value` is the
semantic tree; `spans` indexes original UTF-8 byte ranges by JSON pointer. `set` changes one
root key without rewriting unrelated tokens, comments, line endings or key spelling. The
caller can persist the returned source using its preferred atomic writer.

`Settings` resolves Default → Platform → User → Syntax → Project → View. Observers fire on
effective changes; typed accessors tolerate malformed individual preferences. Its revision
lets UI clients cache `EditorSettings` outside the typing/layout path.

`Resources` overlays installed zip packages with loose package files. Default sorts first,
other packages alphabetically, User last; platform files follow common files within each
package. Archives cannot escape their package or contain resources above 32 MiB.

`Keymap` indexes chords in a trie. Later resources/entries win, contexts combine with AND,
and selection predicates use any/all semantics. A pending sequence has no timeout; a failed
continuation is retried at the root. Regex predicates compile once, using Rust regex syntax;
unsupported lookarounds/backreferences return a resource error. `ScopeProvider` is the syntax
integration seam; `PlainScope` handles plain-text scope names, comma alternatives and simple
exclusions. Full scope-selector grammar belongs to the syntax lane.

`ui` parses mouse maps, recursive menus and palette descriptions. Menus can extend existing
submenu IDs. Menu-derived palette entries are deduplicated by invocation; binding hints honor unconditional key overrides. The native palette interface lives in velocity/vel-ui.

```sh
cargo test -p vel-config
cargo run --release -p vel-config --example keymap_bench
```

The microbenchmark resolves one million events over 1,000 indexed bindings and reports mean
microseconds, including command argument cloning. It is a CPU measurement, not native input
or GPU latency. Property tests exercise arbitrary Unicode settings values and lossless edits.


ContextProvider::scopes_ready defaults to true for synchronous clients. Asynchronous editor
clients return false until current-version scopes arrive: both positive and negative selector
predicates then fail, preventing quote-pairing contexts from treating pending scopes as code.
Other context keys remain usable while scopes are pending.


Round 7 adds a `DistractionFree` settings layer between Project and View, loaded from
`Distraction Free.sublime-settings`. Effective values/observers revert when the layer is removed.
Parsed `Keymap` values are cloneable so fresh session placeholders reuse compiled contexts and
tries. Each clone retains its own pending chord state; the application's cached template starts
at the root. Application-wide Hot Exit/remember/open-window settings bypass project/syntax/view
layers in the workbench adapter. Malformed optional user resources become startup diagnostics.

Keymap clones share immutable binding and prefix-trie tables; each view keeps its own pending
chord. Appending an override copies shared tables before changing them. This keeps large Hot
Exit restores from rebuilding the same tables for every sheet.


Round 10 adds process-unique settings value identities alongside the local revision counter.
Clones share a value identity until one changes. `Settings::project` copies selected effective
keys into one lookup layer with its own immutable value identity, allowing theme callers to cache compact snapshots without repeatedly
traversing all application setting layers. Observer behavior and precedence remain shared with
the ordinary settings API.
