# vel-theme

Headless, clean-room `.sublime-theme` engine. It shares lenient JSON with `vel-config` and CSS /
`color()` evaluation with `vel-color-scheme`. It never initializes a window or GPU. The original
Velocity UI resources live in `../velocity/resources/Packages/Theme - Velocity/`.

## Coverage

All properties and current element classes in the [public theme reference](https://www.sublimetext.com/docs/themes.html)
are represented, including multiclass specializations. `ELEMENTS` contains 130 entries and their
attribute vocabularies. `PROPERTIES` and `COVERAGE.md` enumerate the typed property model; four
layers support tint, opacity, texture, nine-slice margins, draw-center, repeat and tint modifiers.
The engine accepts custom classes/attributes as well.

Rules support class conjunctions, attribute negation, boolean settings arrays, boolean/string /
string-array settings objects, missing-string coercion, ordered distinct ancestor subsequences,
platform filters and later-property precedence without CSS specificity. Modern objects and
legacy rule arrays accept comments, BOMs and trailing commas. `extends` is recursive with cycle
detection. Package fragments merge lexically, User last; variables rebind inherited rules before
compilation. Variables may contain any JSON value. CSS, color-mod, RGB/HSL and legacy derived
scheme colors are compiled once. `color` and `fg` share one property identifier.

Float/color transitions retain target, speed and interpolation; `Animated::sample` provides a
deterministic linear/smoothstep sample from a caller-owned starting value and monotonic elapsed
seconds. The sampling helper uses speed as a rate multiplier (`duration = 1/speed`). PNG sequences
retain keyframes, frame time and loop behavior. CSS tint modifiers, `-1` disabled tint indices,
and explicit null resets are accepted for third-party compatibility. Unrecognized properties
remain `PropertyValue::Extension` and generate diagnostics; they are never silently interpreted.

`load_texture` selects 1x/2x/3x resources through the host's package provider, decodes straight-alpha
RGBA8, and returns logical margins and nine-slice source patches. Palette, grayscale, RGB, RGBA
and 16-bit PNG input are normalized. The decoded-image limit is 64 megapixels. `FileIcons` merges
global mappings then theme mappings; resolution prefers the longest relative path, exact filename,
compound extension, extension and optional `*`. Its result is an icon resource identifier; package
lookup (root or `icons/`) and uploads belong to the host.

## Lane A integration

```rust
use std::sync::Arc;
use vel_theme::{Appearance, Element, ElementPath, Palette, Platform, Property, Resolver, Settings, Theme};

let palette = Palette::default(); // or Palette::from_scheme(&active_global_scheme)
let theme = Arc::new(Theme::velocity(Appearance::Dark, &palette)); // pass the OS appearance
let mut resolver = Resolver::new(theme);
let settings = Settings::default(); // use the application's existing layered Settings
let nodes = [
    Element::new("window", &[]),
    Element::new("quick_panel_row", &["selected"]),
    Element::new("quick_panel_label", &[]),
];
let style = resolver.resolve(&ElementPath(&nodes), &settings, Platform::Mac);
let foreground = style.color(&Property::Fg); // copy values while this cache borrow is held
assert!(foreground.is_some());
```

Keep one resolver per UI thread and use stack-backed ancestor/attribute slices. Cache hits return
`&ResolvedStyle` without allocation. Keys include path, relevant setting values, revision and
platform, with full equality checks after hashing. The default bound is 4096 combinations; a miss
at capacity clears old entries. `replace_theme` invalidates the cache after palette, theme or User
resource changes. Cold resolution and style cloning may allocate. Properties are sparse: absent
values mean the widget's default, not an invented engine default.

Every class in `vel-ui/README.md` is accepted. Canonical classes are unchanged; aliases are:

| vel-ui class | Theme class(es) |
| --- | --- |
| `text_input` | `text_line_control` |
| `scroll_area` | `scroll_area_control` |
| `kind_badge` | `kind_container` |
| `quick_panel_annotation` | `quick_panel_label hint` |
| `quick_panel_details` | `quick_panel_detail_label` |
| `panel` | `panel_control` |
| `button` | `button_control` |
| `open_file` | `file_system_entry` |
| `text_area` | `text_area_control` |
| `grid_layout` | `grid_layout_control` |

Aliases preserve raw class matching too. Child labels inspect their actual ancestors; generic
attribute inheritance is not invented. Pass selected/hover/pressed/focused/dirty/transient states
from widgets, plus file luminosity or other application states when applicable. `luminosity`
implements the documented HSV thresholds. Velocity styles direct `open_file`/`tab_control`
transient states as well as child labels. A `kind_badge` can add a `kind_function` etc. class.

Theme margins are **left, top, right, bottom**; two entries mean horizontal, vertical. Font sizes
remain `FontSize::{Pixels, Rem}`: call `pixels(root_font_size)` using the global UI setting, or the
completion view's size where documented. Keep start values and start times per widget/property
when an animation target changes; resolving a cached style must not restart an animation.

For installed themes, enumerate `Resource { path, contents }`, then call `load_resources` with the
requested theme name; qualified inheritance paths are also supported. Resolve textures off the
frame path through `load_texture`, upload once, and retain their scale/slice/repeat metadata.
The host applies texture tint/accent modifiers, hit-test thresholds, dimming and font decorations.
Filter labels select fg/match_fg/selected_fg/selected_match_fg from row selection and matched
character ranges; these remain separate typed properties for the painter.
No renderer is linked here. On an OS appearance change, select `Appearance::Light` or `Dark` and
replace the resolver's theme. The application must register the theme preference/default itself;
this lane does not change application wiring or preferences outside its owned resource directory.

`Palette::from_scheme` derives predefined variables from public scheme styles. Use
`Palette::from_scheme_json` when raw scheme JSON is available to retain explicit predefined
variable overrides, or `Palette::set` after a host-managed merged scheme load. Pass the same
palette to minihtml, including scheme-supplied overrides; arbitrary CSS variable additions are
available through `Palette::variables`.

## Verification and timings

```sh
cargo fmt -p vel-theme --check
cargo clippy -p vel-theme --all-targets -- -D warnings
cargo test --release -p vel-theme
RUSTDOCFLAGS='-D warnings' cargo doc -p vel-theme --no-deps
cargo run --release -p vel-theme --example resolve_bench
```

The benchmark performs seven independent 100,000-call runs over all registry entries and asserts
zero allocations after warming the cache. Raw final samples and environment are in `timings.jsonl`.
Final median: **360.47 ns/call** (range 340.58–372.02 ns), zero allocations in every run.
No `sysctl` call is used. Property tests exercise later-rule precedence and ancestor ordering;
tests also cover merge rebinding, settings/cache invalidation, typed properties, aliases, original
theme states, file icons, PNG color types, density fallback and nine-slice metadata.

The public-fixture test fetches **ayu Light, ayu Dark** ([MIT](https://github.com/dempfi/ayu/blob/master/LICENSE))
and **Spacegray** ([MIT](https://github.com/SublimeText/Spacegray/blob/main/LICENSE)) into memory at
test time and measures seven parses each. These are compatibility inputs only; no theme source,
textures or artwork is vendored or used to design Velocity. Missing network/curl skips fetching;
`VEL_OFFLINE=1` explicitly opts out. An available but unparseable fixture fails the test. ayu has
an extension property, retained with a diagnostic. Fixture timings exclude network download time.
