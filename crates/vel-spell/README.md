# vel-spell

Hunspell-compatible checking and suggestions using pure Rust spellbook. The bundled US English
SCOWL size-60 dictionary is the unmodified 2020.12.07 edition. Its full permissive notices are
in `dictionaries/en_US/LICENSE`, also available as `ENGLISH_LICENSE` for binary packaging.
`PROVENANCE.json` pins the exact upstream revision, URLs, SHA-256 hashes and sizes. Dictionary
terms include Kevin Atkinson's permission notice, public-domain components, Princeton WordNet,
VarCon and Geoff Kuenning's BSD-style notice (advertising clause removed with permission).
Keep the entire license/credits file with redistribution. Engine code is MIT OR Apache-2.0.

```rust
use vel_buffer::ChangeId;
use vel_spell::{SpellChecker, DocumentSpelling};
use vel_text::Text;
use vel_scope::Selector;
let mut checker = SpellChecker::english()?; // parse once, on a worker
checker.set_ignored_words(["Velocity"]);
checker.add_word("customword"); // caller persists added_words() in User settings
let text = Text::from("a mispelled word\n");
let errors = checker.check_words(&text, &[0..text.len_bytes()])?;
let suggestions = checker.suggestions(&errors[0].word, 5); // on worker / on demand
let document = DocumentSpelling::new(text, ChangeId(0), Selector::parse("comment, string")?);
# let _ = (suggestions, document);
# Ok::<(), Box<dyn std::error::Error>>(())
```

At the existing transaction boundary forward each `ChangeSet` and matching post-edit snapshot
to `DocumentSpelling::apply_change`. On viewport requests call
`document.check_lines(&checker, &highlight_batch.scopes, visible_lines)` and render returned
absolute byte ranges. Exact current-version vel-highlight scope runs are filtered with the
user's `spelling_selector`; unavailable runs clear their diagnostics until exact scopes arrive.
The cache keeps at most 4096 lines, relocates unchanged lines and rechecks only edited, newly
visible, scope-changed or dictionary-changed lines. No second parser. The application gates
requests with `spell_check` (default false); ranges are not checked outside the selector.
`check_scoped` offers stateless scope filtering; `check_words` accepts already filtered ranges.

`SpellChecker::from_package(packages_root, "Packages/User/en_GB.dic")` loads UTF-8 user Hunspell
pairs, including `.aff`. `package_dictionaries` discovers immediate `Packages/*/` pairs.
Resources cannot traverse or symlink outside Packages. No user files are written by this crate.

ST-BEHAVIOR: unverified `[TOK1]`: URL/email whitespace tokens are skipped; snake_case and
camelCase/acronym components are checked separately, with uppercase retained at the start of
the next component (`badSpeling` checks `bad` and `Speling`). Numeric components are skipped;
all-uppercase words are checked, not automatically ignored. Unicode word segmentation retains
internal apostrophes; smart apostrophes normalize to ASCII for dictionary lookup. Only complete
words fully covered by allowed ranges are checked, so selector edges never create false partial
words. Ignore/add matching uses Unicode lowercase. These are explicit local choices pending
black-box golden tests, not claims of observed editor behavior.

The spellbook 0.4.2 library dependency is MPL-2.0, separately from the dictionary and this
crate's MIT OR Apache-2.0 code. Its full license and exact-source notice are included in
`LICENSES/`; retain these when packaging the dependency.

Five-run budgets and reproducible commands: [round-seven evidence](../vel-git/ROUND7.md).
