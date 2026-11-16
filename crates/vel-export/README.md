# vel-export

Standalone highlighted HTML, dual clipboard flavours and persistent temporary
browser-print files. This crate has no UI, clipboard or process-launch dependency.

```rust
use vel_export::{Options, export_range, copy_as_html, write_print_file};
use vel_buffer::{Region, Selection};
use vel_color_scheme::ColorScheme;
use vel_text::Text;

let text = Text::from("let answer = \"猫 & 🦀\";\n");
let scheme = ColorScheme::velocity_dark();
let options = Options { line_numbers: true, ..Options::default() };
let document = export_range(&text, 0..text.len_bytes(), &[], &scheme, &options)?;
let selection = Selection::from(Region::new(0, text.len_bytes()));
let clipboard = copy_as_html(&text, &selection, &[], &scheme, &options)?;
assert_eq!(document, clipboard);
let path = write_print_file(&document)?;
assert_eq!(std::fs::read_to_string(&path)?, document.html);
std::fs::remove_file(path)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Pass `&batch.highlighted` from vel-highlight in place of `&[]`. Those resolved
`StyledLine` spans are line-local UTF-8 byte ranges; they may include LF. Caller
must match the text snapshot, `ChangeId` and scheme revision to the batch. The
exporter validates sorted unique line indices, sorted non-overlapping spans and
scalar boundaries. Gaps and approximate (`exact == false`) lines use scheme
defaults; incomplete highlights never produce invented exact colors. Hashed token
colors are already resolved by the highlighter. No parsing happens during export.

Options select title, a literal font-family name, positive finite point size,
tab stops (1–256 display columns) and original one-based line numbers. Colors
include alpha; styles include bold, italic, underline variants and glow. Tabs are
expanded at original line columns with grapheme widths, including CJK/emoji and
combining marks. Partial-line ranges therefore retain their original tab stops.
Plain text keeps source tabs, CR and Unicode. HTML escapes text/title and quotes
CSS font names, including `<`, quotes and control characters; literal CR is an
entity, and NUL becomes U+FFFD in HTML because HTML cannot represent NUL. Everything
else stays UTF-8. Source markup never executes. The document loads no resources.

Copy uses nonempty selections in document order and inserts LF between fragments
unless the preceding fragment already ended in LF. With only carets it copies
complete distinct touched lines. In mixed selections it ignores empty carets.
Line numbers appear only in HTML. These copy-fragment choices are explicit Velocity
policy, not a black-box assertion of undocumented editor behavior.

Publish `HtmlExport.html` as `text/html` and `.plain_text` as `text/plain` in the
same platform clipboard transaction. Platforms needing additional transport framing
(for example Windows CF_HTML byte offsets) should add it in their clipboard adapter.
The document includes its own CSS and print media rules, with print color adjustment,
wrapping of long printed lines and no JavaScript. `write_print_file` writes and
closes a unique `.html` file **without deleting it**, then returns its `PathBuf`.
Open that path in the default browser on the app's existing platform path. Let the
user invoke the browser print dialog; clean up the file after the browser finishes,
or on a later application startup. Do not delete it immediately after launching an
asynchronous browser. Lane A wires menus and clipboard/browser actions next round.

Golden tests cover complete documents and disjoint selections. Tests also read the
actual print file after the writer drops, reject invalid coordinates/highlights,
exercise CSS/HTML injection escaping, alpha/decorations, tab columns, EOF and empty
input. `cargo run --release -p vel-export --example export_bench` exports 10k lines
with 30k resolved spans and validates the generated print file. Paired measurements
and 95% CIs are recorded in [ROUND13.md](../vel-regex/ROUND13.md).
