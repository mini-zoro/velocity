# vel-minihtml

Clean-room HTML/CSS subset layout for popups, phantoms, annotations and inline item details.
The [public minihtml reference](https://www.sublimetext.com/docs/minihtml.html) is the behavior
reference. There is no browser, script runtime, image I/O, native font initialization or GPU here.

## Coverage

The full requested tag set is parsed: html, head, style, body, h1–h6, div, p, ul, ol, li, b, strong,
i, em, u, big, small, a, code, var, tt, span, br, img. Classes, IDs, inline styles, href and title
are supported, along with image src/alt/width/height. Fragments receive implicit html/body nodes;
tag/attribute names are case-insensitive, text/attribute entities are decoded, script/head content
is not laid out, and unknown tags are transparent inline containers. The parser tolerates omitted
list-item closers and common paragraph boundaries. Input is bounded to 4 MiB / 100,000 nodes /
256 nested elements. This is a minihtml fragment parser, not the full HTML5 error-recovery algorithm.

CSS supports type, universal, class, ID, compound, descendant, child and comma-list selectors;
specificity, source order, inline precedence and `!important`; inherited custom properties,
`var()` with fallback, the eleven predefined scheme variables and implicit `html.dark` /
`html.light` from HSL background lightness. Variable recursion and expansion are bounded.
Invalid/unsupported declarations or selectors are ignored. It supports:

| Group | Properties / values |
| --- | --- |
| Flow | display: none, inline, block, inline-block, list-item |
| Position | position: static/relative; top/right/bottom/left, including negative offsets |
| Box | margin/padding and four sides; border and per-side width/style/color; four corner radii |
| Dimensions | width, height, min/max-width, min/max-height; automatic or px/em/rem/%/pt lengths |
| Text | color, background-color, font-family/size/weight/style, line-height, text-decoration |
| Alignment | text-align: left/center/right; vertical-align: baseline/top/middle/bottom/sub/super/length |
| Whitespace | normal, nowrap, pre, pre-wrap |
| Lists | disc/circle/square/none and decimal ordered-list markers |

CSS shorthand edge order is **top, right, bottom, left**, distinct from theme margins. Points
convert at 96/72; rem uses the supplied root font setting. All named/hex/RGB/HSL/HWB/transparent
colors and color-mod adjusters reuse `vel-color-scheme`. Positive sibling block margins collapse;
parent/child and empty-block margin propagation is not a full browser margin-collapse model.
Inline boxes wrap with text, while inline-blocks have independent layout and a last-line baseline.
The subset excludes tables, floats, flex/grid, absolute/fixed positioning and external stylesheets.
These exclusions do not remove any property requested by the lane specification.

Display lists retain parent-before-child boxes, borders/corner radii, styled Unicode text runs,
images, per-fragment link rectangles, inherited/overridden tooltips and scroll extents. Word
boundaries are Unicode aware; actual glyph advances and baselines come from the caller. Long
unbreakable words and nowrap/pre content may overflow, increasing scrollable width.

`http:`/`https:` links return `LinkTarget::Browser`; `subl:name {JSON object}` returns a parsed
command/arguments value; other URLs return `Callback`. No navigation executes here. Image URLs
support file://, res:// and PNG/JPEG/GIF data: MIME types. The host supplies intrinsic dimensions,
decodes pixels and paints; unresolved dimensions use a 16-DIP placeholder until relayout.

`Document::parse_inline` implements the documented details subset (a, b, strong, i, em, u, tt,
code). Other tags are transparent; style/head/script content and inline style declarations are
discarded so item details respect caller theme/font settings. The implicit inline root is
transparent, preserving the row/selection background. For theme-specific inline foregrounds, set
`--foreground` in the supplied palette to the resolved row color; host font overrides may use
`extra_css` on html in addition to the root font size.

## Lane A integration

```rust
use vel_minihtml::{Document, FontMetrics, LayoutOptions, TextMetrics, TextStyle, layout};

#[derive(Debug)]
struct Fonts;
impl FontMetrics for Fonts {
    fn measure(&self, text: &str, style: &TextStyle) -> TextMetrics {
        // Replace with vel-text shaping and font fallback; cache shaped candidates in the host.
        TextMetrics { advance: text.chars().count() as f32 * style.font_size * 0.6,
            ascent: style.font_size * 0.8, descent: style.font_size * 0.2 }
    }
}
let doc = Document::parse("<b>Result</b>: <code>String</code>").unwrap();
let options = LayoutOptions { max_width: 360., ..LayoutOptions::default() };
let display = layout(&doc, &options, &Fonts);
assert!(display.scroll_height > 0.);
```

Retain the parsed document between frames and relayout on width, font, palette or image-size
changes. Supply the same `vel_theme::Palette` used by the theme engine and the view's root font
size. Append the color scheme's popup/phantom CSS through `LayoutOptions::extra_css` (it follows
document styles in source order). `FontMetrics::measure` must shape complete supplied runs with
the requested family fallback list, size/weight/italic settings; paint with the same font metrics.
Glyph rasterization and paragraph bidi processing remain host responsibilities. `image_size`
returns logical dimensions; resource decoding/upload and asynchronous invalidation stay in Lane A.

Translate display coordinates into popup/annotation coordinates when painting. Paint `items` in
order and use `size`/`scroll_height` for viewport/scroll bounds. Convert mouse coordinates back
before `link_at` / `tooltip_at`; command dispatch and browser/callback navigation use the returned
data. Hit regions move with line alignment, wrapping, inline blocks and relative positioning.

## Verification and budgets

```sh
cargo fmt -p vel-minihtml --check
cargo clippy -p vel-minihtml --all-targets -- -D warnings
cargo test --release -p vel-minihtml
RUSTDOCFLAGS='-D warnings' cargo doc -p vel-minihtml --no-deps
cargo run --release -p vel-minihtml --example layout_bench
```

The benchmark uses the original 2043-byte LSP hover fixture and 200 separate diagnostic annotation
documents. It reports seven runs for both retained-document layout and parse+layout, with 100
hover iterations and ten 200-annotation batches per run. The enforced layout budgets are 0.5 ms
per hover and 5 ms per 200 annotations. `timings.jsonl` records every final sample and environment.
The deterministic metrics callback isolates engine time; native shaping, image decoding and GPU
painting are not included. No `sysctl` is used. Final medians: **0.060616 ms** hover layout
(max 0.074715) and **0.833408 ms** per 200 annotations (max 0.980263). Including parsing gives
0.081868 ms per hover and 1.120779 ms per annotation batch; all seven runs satisfy both
budgets even with parsing included.

Golden lists cover a hover, nested inline formatting, lists, inline blocks, borders/radii, images,
command links and tooltips. Additional geometry/state tests exercise selectors/backtracking,
custom variables, dark/light classes, units, constraints, inherited fonts, preserved whitespace,
relative offsets, protocol parsing, inline mode and bounded malformed input. Goldens are original
test content. `VEL_UPDATE_GOLDENS=1` explicitly regenerates them for a reviewed layout change.
