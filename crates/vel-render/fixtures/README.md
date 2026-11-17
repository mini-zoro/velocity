`ligature-fixture.ttf` is an original, deliberately simple test font under the repository's
MIT OR Apache-2.0 license. Its glyphs are rectangles, not artwork copied from another font.
`generate.py` rebuilds it with fontTools. GSUB defines `liga`, `calt` and `ss01`; tests load
it directly without installation or a system font service. Fira Code, when supplied through
`VELOCITY_LIGATURE_FONT`, is an external timing input and is not bundled here.

`fallback-fixture.ttf` and `emoji-fixture.ttf` use the same original rectangle glyphs.
The latter has a separate face identity and emoji cmap coverage to test presentation
selection without CoreText. Replaceable fixture/user fonts are retained as snapshots;
only immutable macOS system font locations use memory mapping.
