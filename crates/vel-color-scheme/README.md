# vel-color-scheme

Clean-room Sublime-compatible color scheme parsing and style resolution. Velocity Dark and
Velocity Light are original designs; no bundled or third-party schemes were read or copied.
See the palettes and contrast measurements in the resource directory's DESIGN.md.

`SchemeSource::parse` reuses vel-config's lenient JSON (BOM, comments, trailing commas).
`SchemeSource::from_tm_theme` handles XML plist themes, including CDATA, and normalizes legacy
style keys. Merge unresolved sources before `compile`: variable/global dictionaries overwrite
by key and rule arrays append, so a User variable override changes base rules too.
`load_resources` accepts `(resource_path, source_text)` pairs in package precedence order.
Hidden scheme extensions and the JSON `hidden` flag are retained. `selected_scheme(settings,
os_is_dark)` implements explicit selection or the `auto` dark/light pair; the editor calls it
again when the OS appearance changes.

Coverage: all color-valued globals use the same color evaluator; scalar/options/CSS globals
remain accessible through `Globals::value`. Rules support foreground/background,
selection_foreground, foreground_adjust, all six documented font decorations (including
explicit empty-style reset), and hashed gradients through either an array-valued foreground
or hashed_foreground. There is no renderer here: underline/glow flags and globals are data for
vel-render to draw.

Color expressions support every CSS named color, 3/4/6/8-digit hex, rgb/rgba, hsl/hsla, hwb,
transparent, recursive var() with fallback, and color() adjusters alpha/a, saturation/s,
lightness/l, blend/blenda (RGB or HSL), and min-contrast. Scalar adjusters support replacement,
addition/subtraction, and multiplication, with cycle/depth checks. Gradients precompute 256
HSL interpolations. Token hashing uses Velocity's deterministic FNV-1a distribution; the
proprietary editor's exact hash and rounding are not claimed to be identical.

```rust
use std::sync::Arc;
use vel_color_scheme::{ColorScheme, Highlighter};

let scheme = Arc::new(ColorScheme::velocity_dark());
let mut resolver = Highlighter::new(scheme.clone());
let scopes = "source.rust keyword.control.rust".parse().unwrap();
let style = resolver.style_for(&scopes);
let id = resolver.intern(&scopes);
assert_eq!(style, resolver.style_for_id(id));
let token_style = scheme.style_for_token(style, "identifier");
```

Properties resolve independently by vel-scope specificity, then resource/rule order. Explicit
background rules select their foreground_adjust, which is applied after foreground resolution
and again to the chosen gradient color. Warm `style_for` lookups intern the active stack into a
scheme-local ID, allocate nothing, and take no scope-interner locks. IDs belong to their
creating resolver; replace the resolver when changing schemes.

Five measured runs (release, arm64 macOS, no sysctl): median **63.78M scope-stack lookups/s**
against the 20M/s budget; all five runs measured **zero allocations** over 5M warm lookups each.
Direct IDs exceeded 1B/s in this microbenchmark, which is not an end-to-end rendering rate.
Raw observations: `timings.jsonl`.

```sh
cargo fmt -p vel-color-scheme --check
cargo clippy -p vel-color-scheme --all-targets -- -D warnings
cargo test -p vel-color-scheme --release
RUSTDOCFLAGS='-D warnings' cargo doc -p vel-color-scheme --no-deps
cargo run --release -p vel-color-scheme --example style_cache_bench
```

The implementation follows public color scheme/minihtml documentation linked by the lane
specification. csscolorparser supplies standard CSS color decoding and plist supplies the
legacy plist model. The CDATA normalization compensates for plist's XML reader dropping those
events. No private application assets or binaries are involved.
