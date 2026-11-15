# Spec: multi-root syntax loading (remove the editor's staging tree) + remaining throughput

Status: P2 / lane B. Locations: `crates/vel-syntax/`, `crates/vel-regex/`, `crates/vel-scope/`,
`crates/vel-highlight/` (only if needed).

## 1. Multi-root + extra-definition loading

The editor (`crates/velocity/src/language.rs`) builds a temporary "staging" directory on every
launch — copying or symlinking the whole `third_party/sublime-packages` corpus and writing our
original `Plain Text.sublime-syntax` into it — only because the loaders accept one root. This cost
~250 ms of cold restore time and, in round 9, caused a cross-lane break (symlinks vs an
`is_file()` guard). Remove the need for it:

- `SyntaxSet::load_cached_roots(roots: &[&Path], extra: &[(virtual_path, source_text)], cache_dir)`
  (name it as you see fit): load several package roots in Sublime's resource override order
  (later roots override earlier ones by package-relative path, like `Packages/` over shipped
  packages and `User` last), plus in-memory definitions with a virtual package path (e.g.
  `Packages/Default/Plain Text.sublime-syntax`); one binary cache keyed by all inputs (root
  content hashes + extra sources); invalidation when any input changes.
- Same for `MetadataSet` (tmPreferences) if useful to the editor (it already takes several roots).
- Keep `load_cached(root, cache)` working (thin wrapper) and all existing tests green.
- Budget: index all 203 syntaxes + extras from a warm cache ≤ 20 ms; cold build unchanged.
- Write the migration note for lane A in `crates/vel-syntax/README.md` (exact replacement for the
  staging code path).

## 2. Highlighting throughput: Rust, JavaScript, C++ to ≥ 2.5 MB/s

Background parse+style (the `vel-highlight` worker benchmark) is ≈ 2.2–2.4 MB/s for Rust in the
reviewer's paired runs, JS ≈ 1.9, C++ ≈ 1.7. Profile those three end to end (regex search,
context transitions, scope ops, styling, checkpoints, allocation) and fix with general rules.
**Measure only with paired/alternating A/B against the current main build** (the machine is
shared and sequential numbers swing ±30%); report medians with confidence intervals; zero
differential divergences; baseline monotonic.

## Rules

- Your worktree: /Users/zoro/dev/velocity-lane-b (branch `lane-b`). Modify only `vel-syntax`,
  `vel-regex`, `vel-scope`, `vel-highlight` (+ root `Cargo.lock`). Do not commit.
- Lane A works in another worktree on the theme/minihtml UI; it will switch `language.rs` to your
  new API next round, so don't edit `crates/velocity`.
- fmt, clippy -D warnings, release tests, rustdoc -D warnings; differential test.
