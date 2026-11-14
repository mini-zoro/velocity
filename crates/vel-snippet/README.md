# vel-snippet

Pure-data `.sublime-snippet` and inline snippet expansion. No buffer mutation, UI, file I/O or
shell execution. Fields and edits use absolute UTF-8 byte coordinates. Resource XML preserves
CDATA and escaped character data; the parsed resource exposes content, tab trigger, selector,
description and a reusable compiled Snippet.

```rust
use vel_snippet::{Environment, ExpansionOptions, Snippet, SnippetEdit};
use vel_text::{Edit, Text};

# fn example() -> Result<(), vel_snippet::Error> {
fn apply(text: &mut Text, edits: &[SnippetEdit]) {
    text.apply_edits(&edits.iter().map(|edit| Edit {
        range: edit.range.clone(), text: &edit.text,
    }).collect::<Vec<_>>());
}
let snippet = Snippet::parse("${1:name} = $1; $0")?;
let mut text = Text::from("  ");
let mut expansion = snippet.expand(
    &text, std::slice::from_ref(&(2..2)),
    &Environment::default(), ExpansionOptions::default(),
)?;
apply(&mut text, &expansion.edits);
assert_eq!(text.to_string(), "  name = name; ");
let edits = expansion.session.replace_field("item")?;
apply(&mut text, &edits); // updates primary and mirror together
assert_eq!(text.to_string(), "  item = item; ");
let final_selections = expansion.session.next_field(); // $0, after numeric fields
# let _ = final_selections;
# Ok(())
# }
```

For editor integration, apply owned edits as a single old-coordinate Buffer transaction, then
select `session.selections()`. Carets/selections passed to expand must be sorted and disjoint.
Initial selection is the first numeric field; implicit `$0` is added at the end if absent.
`fields()` includes primary/mirror/transformed regions and caret-instance indices. `next_field`,
`prev_field` and `has_next_field` implement Tab/Shift+Tab/context queries. Replacing a parent
removes its embedded editable stops. Mirrors, including enclosing defaults and transformed
mirrors, are regenerated from the current independent values of each caret instance.

Route primary-field typing/deletion through `session.edit_fields(pre_edit_text, input_edits)`
and apply only its returned transaction. This includes both the input and all mirror changes.
`replace_field_values` accepts one unindented value per caret when the UI already knows the
replacement values. Rewrites replace entire snippet instances atomically; group these with the
same typing/undo action, and do not separately apply the input edits. `rebase(ChangeSet)` maps
external nonintersecting edits; a false result means cancel the session. Undo/redo or edits
inside an instance that bypass these methods should cancel it. Session plans assume each returned
transaction is applied before another call; recreate/cancel state if an application rejects one.

Inline syntax covers `$n`, `${n:default}` with nested fields/variables, forward mirrors,
`${n/regex/format/options}` and variable substitutions. Format replacements support captures,
`$&`, pre/post-match, last capture, `(?1:yes:no)` / `(?1yes:no)` nested conditionals and
`\u \U \l \L \E` Unicode case changes. Global zero-width substitutions retry nonempty matches
before advancing at UTF-8 boundaries. Regexes use vel-regex's Oniguruma-compatible backend;
unsupported flags or compile errors are explicit. Replacement syntax covers numeric captures;
named-capture replacement aliases are not exposed. Backticks remain literal text.

Environment contains caller variables and fills each caret's SELECTION/TM_SELECTED_TEXT,
TM_CURRENT_LINE/WORD, zero/one-based line numbers, TM_TAB_SIZE and TM_SOFT_TABS.
`with_path()` adds TM_FILENAME/FILEPATH/DIRECTORY; `with_metadata()` merges scope-selected shell
variables, including TM_COMMENT_*. Supply TM_SCOPE or custom variables through the environment
map. Inserted continuation lines inherit the caret line's leading indentation. Tab translation
uses visual Unicode widths and tab stops; region offsets are remapped after indentation.

`tests/snippets.rs` contains snippet/input/output tables, field navigation, nested replacement,
forward/transformed mirrors, Unicode, independent carets, escapes, XML and indentation cases.

```sh
cargo test --release -p vel-snippet
```

References: public [completion/snippet grammar](https://www.sublimetext.com/docs/completions.html),
[TextMate snippets](https://macromates.com/manual/en/snippets), and the
[Boost replacement format](https://www.boost.org/doc/libs/latest/libs/regex/doc/html/boost_regex/format/boost_format_syntax.html).
Implementation is original clean-room code; no private application snippets were read.
