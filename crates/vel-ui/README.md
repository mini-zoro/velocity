# vel-ui

Original retained widgets painted with `vel-render::scene` primitives. No window, font or GPU
is initialized by this crate. Layout supports rows, columns and stacks, fixed/flexible extents,
ancestor-clipped hit testing, hover/press attributes, retained focus and finite animations.
`Scroll` bounds offsets and virtualizes rows; `QuickPanel` paints only visible rows, with kind
badges, annotations, details and matched-character coloring. Navigation returns a payload for
the application's transient-sheet preview callback. No Sublime application resources were used.

`input::Input` uses vel-buffer and vel-commands for committed edits, clipboard, selection and
undo. Native IME previews stay outside buffer history; UTF-16 ranges include the preview.
Candidate geometry, scrolling and hit testing count grapheme display cells, including CJK,
combining sequences and emoji. The widget grid uses 12pt UI text (7.2pt cells); paragraph bidi
and the future `.sublime-theme` engine are outside this round.

Keyboard capture is overlay → focused bottom panel → view. A panel can stay visible while the
document has focus. Closing an overlay restores the lower surface's focus. Velocity retains
nodes for visible virtual rows across frames; offscreen rows are discarded without allocating
one widget per project file. File loading, palette actions and previews stay in the controller.

The theme seam is `Node::class` + `Node::attributes`:

| Class | Widget | Dynamic attributes |
| --- | --- | --- |
| `quick_panel` | Goto/palette overlay | hover, pressed |
| `text_input` | Overlay, find, replace, Where, input panel | focused, hover, pressed |
| `scroll_area` | Quick-panel viewport | hover, pressed |
| `quick_panel_row` | Visible ranked result | selected, hover, pressed |
| `quick_panel_label` | Caption | inherited row state |
| `kind_badge` | File/command/symbol/word badge | inherited row state |
| `quick_panel_annotation` | Key binding hint | inherited row state |
| `quick_panel_details` | Path/command details | inherited row state |
| `panel` | Input/find/output/results dock | hover, pressed |
| `button` | Find flags and actions | selected, hover, pressed |
| `sidebar_container` | Folder/Open Files tree | hover, pressed |
| `open_file` | Visible open document | selected, dirty, transient, hover, pressed |
| `tree_row` | Lazy folder/file row | selected, expanded, hover, pressed |
| `tab_control` | Scrolling, reorderable tab | selected, dirty, transient, hover, pressed |
| `status_bar` | Position, selections, encoding, EOL, indent, syntax | hover, pressed |
| `text_area` | Main editor viewport | hover, pressed |

Caption/badge/detail nodes are noninteractive children; theme lookup may inspect their parent
row's state. Colors are original hardcoded scene colors until the P3 theme engine arrives.
Output and Find Results use the same bounded scroll state, with append-only result rows.

```sh
cargo test -p vel-ui
cargo clippy -p vel-ui --all-targets -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc -p vel-ui --no-deps
```
