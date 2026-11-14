# vel-regex

Lazy, `Arc`-shared, `Send + Sync` Oniguruma patterns. Compile failures are retained
as values and never match; resource-limit errors return no match and increment
`metrics()`. Defaults are 10,000,000 retries and an 8 MiB match stack. No local
`unsafe` is needed.

`PatternSet::find_earliest` chooses the earliest start and then the lowest original
rule index. Captures are exported only for that winner. The Onig implementation
reuses its most recent capture registers when possible. Searches retain the
whole line and start offset, preserving lookbehind and `\G`. `SearchCache` is
per line and must be reset when the caller changes lines; position-dependent
patterns bypass next-match reuse.

The parser actually calls `MatcherBackend::build` for flattened contexts and
searches the resulting `PatternMatcher`. A future multi-pattern DFA can implement
these traits and be installed using `SyntaxSet::with_matcher_backend`; it must
respect rule exclusions, empty-match policies, original rule indices, and
winner-only captures. Single-pattern backends implement `RegexBackend` and
`CompiledPattern`. Alternative backends can supply captures with
`Region::from_spans` and retain workspaces using `SearchCache::backend_state`.

Runtime numeric capture references are instantiated using escaped literal text;
instantiations share compiled machines and a bounded strong cache. Literal
whitespace is protected from extended-mode regex flags.

The conservative literal-prefix filter is adapted from the MIT-licensed syntect
PR #705 at `3eea6bf27014e260d00e99552d4b861410457dd8`, by Tristan Hume, Keith Hall,
Google Inc, titoBouzout and other contributors. It leaves unsupported Onig
dialects and case folding unfiltered. Debug builds verify rejections with Onig.
See [LICENSE-syntect](LICENSE-syntect). No syntect dependency, packaged syntaxes,
themes or editor-private code is included.
