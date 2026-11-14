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
