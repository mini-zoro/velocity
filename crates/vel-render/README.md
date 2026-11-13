# vel-render

Velocity's wgpu 30.0.2 Metal renderer, MIT OR Apache-2.0. Font discovery, bitmap
fallback, raster plumbing and deferred timestamps are adapted from the repository's
[P0 prototype](../../prototypes/macos-shell/README.md), under the same license.
Scene types, page cache and native integration are product implementations.

Scene layers contain shadows, solid/rounded rectangles, text runs and straight/wavy
underlines. Geometry is in logical points. The renderer rasterizes at physical size,
uses quarter-pixel horizontal buckets, clips in the fragment shader, and batches one
instanced draw per primitive kind per layer. A texture array lets all glyph pages
share that draw. Explicit layers preserve painter order (for example the caret above
text). Rounded rectangles/shadows/underlines use shader distance/coverage functions.

The primary Menlo face is mapped directly from the system font collection; ASCII runs
skip shaping and advance exactly one cell. Fallbacks are loaded on first non-ASCII use:
PingFang SC and Apple Color Emoji resolved by CoreText, retaining the actual CTFont.
Fallback selection preserves graphemes, adjacent same-face text is shaped with swash,
and clusters are positioned on the editor grid. A collision-checked bounded LRU caches
content hashes + size/font style. The UI also caches visible/recent tokenized lines.
This is a unidirectional code-editor layout, without paragraph bidi or automatic wrap.

Swash rasterizes outlines and color bitmaps. If it cannot read the resolved font's
bitmap/outline format (notably some PingFang faces), CoreText draws with the retained
face. Quartz premultiplied color pixels are converted to straight alpha. Bitmap
orientation, mixed fallback and primary ASCII spacing are tested without Metal.

Atlas pages are 1024², grow on demand up to eight, and separate mask/color classes.
All use RGBA8 storage (mask pixels expand to white RGBA). Page-granular LRU eviction
removes old keys; current-frame pages are pinned. An over-budget frame errors rather
than drawing stale UVs. Growth copies earlier array layers. Every upload includes a
transparent border, even after recycling; empty glyphs have a bounded negative cache.
One page is 4 MiB. GPU allocation diagnostics are distinct from process footprint.

Startup submits the whole first scene to an offscreen texture before orderFront.
Presentation is a COPY_DST surface copy immediately after orderFront in the same
launch callback. Later frames render directly to acquired surfaces. Fifo is mandatory,
with desired_maximum_frame_latency=1; Immediate and Mailbox are not selectable.
Surface timeout/outdated/lost are retryable; successful first presentation releases
the offscreen texture. The owner WindowHandle outlives the surface.

## Pipeline cache and measurement

The pinned wgpu 30.0.2 source documents PipelineCache as Vulkan-only (see its
src/api/pipeline_cache.rs). Metal's adapter currently does not advertise PIPELINE_CACHE.
The renderer checks support, records device/shader/pipeline creation time and reports
unsupported explicitly; driver caches remain in use. If supported, actual wgpu cache
data is loaded/saved atomically in velocity/output/pipeline-cache, keyed by adapter.
No placeholder file or WGSL source is represented as a persisted binary pipeline cache.
Hardware cold-ish/warm comparison is still required; no cold-start budget is claimed.

Scroll uses render-pass timestamp pairs and one deferred readback after 600 samples.
CPU timing includes acquisition/layout/uploads where indicated; GPU timing covers
clear + scene draws, excluding upload, timestamp resolve/readback, and presentation.
Tests validate the WGSL with naga and exercise packing, LRU/pinning, cache invalidation,
UTF-16-facing editor layout and CoreText raster behavior without creating a device.
The reviewer must check Metal validation, scanout, Retina/color quality and ProMotion.
