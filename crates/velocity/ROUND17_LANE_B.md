# Round 17 Lane B review record

Current starting point: `7925499` ("WIP round 17 lane B fixes 3") on main `58fd98c`.
This fourth verification pass remains uncommitted on top of that WIP; it is not amended,
reset or rebased. Its evidence is retained in `/tmp/velocity-lane-b-fourth/`. The main
baseline is the independently built `git archive 58fd98c` in the third pass's `base-target/`, sharing
only the pinned permissively licensed syntax checkout and identical 10 MB Rust fixture.
Third-pass evidence below is historical; still earlier measurements use `d442835`.
No GUI, packaging script, `cargo clean` or `target/package-macos/` access was used.

## Fourth verification corrections

| Finding | Fix (file:function) | Headless test |
| --- | --- | --- |
| 1 Outside-click drags cannot reach the ends | `view.rs:pointer_with_option`, `MinimapGeometry::drag_scroll`: retain click centring through the live drawn-row map, then anchor a piecewise-linear drag at the press and centred scroll. Each side reaches its corresponding document end. Inside-box grabs keep the travel inverse. | `minimap_outside_click_drags_reach_both_ends_from_top_middle_and_bottom` (300/10,000 lines, long/wrapped/folded, real nonempty folds, both directions, stationary events, exact 1px motion and release); retained click-centre/inside-box tests |
| 2 VS15 keycaps omit the enclosing mark | `fonts.rs:cluster_characters`, `system_face`; `font.rs:poll_fallbacks`, `fallback_face`: one coverage rule requires all bases and combining marks, including U+20E3; only variation selectors and ZWJ are ignored. An incomplete first CoreText run advances through the ordered cascade. | `native_keycaps_cover_combining_marks_and_converge_with_every_selector` (1/#/*, VS15/VS16/bare, both load orders; non-notdef glyphs, whole-cluster cmap coverage, available VS15 text face and no further requests); expanded `presentation_fallback_converges_for_color_only_text_missing_and_combining_clusters` |
| 3 Bare default emoji follow loaded text faces | `font.rs:emoji_presentation`, generated `emoji_presentation.rs:default_presentation`: use Unicode's `Emoji_Presentation=Yes` property before scalar/loaded-face coverage; VS15/VS16 retain their explicit presentation. | `default_emoji_presentation_is_independent_of_text_face_load_order` (watch/hourglass/alarm/coffee/lightning and default-text heart; mocked platform; both orders, explicit selectors, repeat shaping and idle convergence) |

The compact table contains **80 merged ranges / 1,228 scalars**, generated from
[Unicode 18.0.0 emoji data](https://www.unicode.org/Public/18.0.0/ucd/emoji/emoji-data.txt).
The generator, input SHA-256 and Unicode license are retained alongside our original font
fixtures. Regenerate from a downloaded data file with
`python3 crates/vel-render/fixtures/generate_emoji_presentation.py INPUT`.
The table is Unicode data; no Sublime package/resource was read or copied. The native
cascade continuation follows Apple's documented
[ordered fallback descriptors](https://developer.apple.com/documentation/coretext/ctfontcopydefaultcascadelistforlanguages(_:_:)).
It runs on the existing worker, with no native work added to shaping or idle frames.
No preference defaults change in this pass. All previously confirmed behavior is retained.

## Fourth verification validation (current, main 58fd98c)

The full gate passes: fmt; strict release/locked workspace clippy over all targets;
`cargo test --workspace --locked --release` with default parallelism (**969 passed,
0 failed, 18 ignored; 114 test executables**); and
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`.
Logs and `gate-summary.json` are under `/tmp/velocity-lane-b-fourth/`.
The complete `app::lane_b_round17_tests` module passes **10/10 runs at four threads**:
29 passed, 0 failed, one ignored timing test each time (`stability-*.log`, `stability.json`).

All **14 application** and **2 renderer/font** ignored timing/stress tests pass explicitly,
serially and without concurrent builds (`ignored-app.log`, `ignored-render.log`).
The existing Fira Code fixture is external and remains unbundled. Ragged selection still
meets its scene budgets through the real Select All path:

| Visible rows | Points | Triangulation µs | Plain scene µs | First selected µs | Cached selected µs |
| --- | ---: | ---: | ---: | ---: | ---: |
| 50 | 510 | 9.190 | 106.674 | 145.083 | 111.165 |
| 200 | 2010 | 35.143 | 430.262 | 542.167 | 458.943 |
| 1000 | 10010 | 179.343 | 2204.532 | 2589.250 | 2382.156 |

Nine alternating pairs use the supplied `paired-headless.py`, with the existing
`/tmp/velocity-lane-b-review/paired-instructions.py` wrapper sampling each child before
reaping it. Counters include process startup, warmup and background workers. Baseline
`58fd98c` uses the independently built binary from the third pass; its hash is unchanged.
There were no concurrent builds or timing tests. The acceptance boundary is retired
instructions; every wall-clock percentile also passes the 5% gate.
Raw process counters, timing samples and hashes: `paired/{instructions,report}.json`.

| Retired instructions, nine-pair median | Main 58fd98c | Candidate | Change |
| --- | ---: | ---: | ---: |
| features | 240,122,221,653 | 239,244,326,600 | -0.37% |
| typing1 | 425,772,660 | 427,568,489 | +0.42% |
| typing1000 | 10,652,403,565 | 10,638,609,743 | -0.13% |

One-minute host load ranged 2.23–16.81 on 8 logical CPUs.
Baseline SHA-256: `75a08653f8c08380738251283b7bdff41a5c164c14ef4e8893a2667725aaef4d`.
Candidate SHA-256: `8bc0538e468e91938a45efaba061419e63934501d720836e04b85339032bff5a`.

| Wall CPU phase, p50 / p95 / p99 milliseconds | Main | Candidate | Change |
| --- | --- | --- | --- |
| `features/highlight_minimap_scroll_cpu/stats_ms` | 0.298563 / 0.382681 / 0.450277 | 0.272292 / 0.352506 / 0.393459 | -8.80% / -7.89% / -12.62% |
| `features/highlighted_typing_cpu/edit_stats_ms` | 0.049375 / 0.071502 / 0.083353 | 0.046666 / 0.066170 / 0.077508 | -5.49% / -7.46% / -7.01% |
| `features/highlighted_typing_cpu/scene_stats_ms` | 0.819166 / 1.254336 / 1.332650 | 0.782792 / 1.276089 / 1.310054 | -4.44% / +1.73% / -1.70% |
| `features/highlighted_typing_cpu/stats_ms` | 0.903938 / 1.318165 / 1.417048 | 0.839438 / 1.336062 / 1.376145 | -7.14% / +1.36% / -2.89% |
| `typing1/headless_summary/edit_and_scene_cpu_ms` | 0.202875 / 0.348077 / 6.697951 | 0.204313 / 0.346030 / 6.717529 | +0.71% / -0.59% / +0.29% |
| `typing1000/headless_summary/edit_and_scene_cpu_ms` | 5.335458 / 14.970654 / 27.471866 | 5.323146 / 14.600223 / 27.097394 | -0.23% / -2.47% / -1.36% |

The first-non-ASCII measurement was rerun in **nine fresh processes**, using the exact
ignored font timing test, Menlo and `shape("你", 26.)`, after the paired bench completed.
The OS/font-service caches are warm; these are fresh-process measurements, not cold boot.
Median first call **0.087 ms**; asynchronous fallback ready **66.726 ms**.
Median RSS before primary / after primary / after the resolved glyph: **7.203 / 7.578 / 20.000 MiB**.
The resolved Chinese face remains the mapped, sealed-system `PingFangUI.ttc`; user and
AssetsV2 copies and all mapping-policy checks are unchanged. First-frame fallback stays
asynchronous. Menlo 13 keeps **16 px at 1× / 15.5 px at 2×**, before padding.
Raw samples and font metadata: `font-run-*.log`, `font-runs.json`.

| Font, nine-process median | Unique run µs | Cached run µs |
| --- | ---: | ---: |
| Menlo | 0.119 | 0.071 |
| FiraCode-Regular | 27.806 | 0.113 |

Native visual review remains pending in `MANUAL_TEST.md`. No existing defaults or the
unconfirmed B6 wrapping behavior changed. The old View-zoom migration limitation below
still applies. No Lane A source file or manual-test section was changed.

## Third verification corrections (retained)

| Finding | Fix (file:function) | Headless test |
| --- | --- | --- |
| A VS15 resolution loop | `font.rs:fallback_face`, `poll_fallbacks`; `fonts.rs:resolve_text_cluster`: whole-cluster text-presentation cache accepts a color-only cascade answer and caches misses too | `presentation_fallback_converges_for_color_only_text_missing_and_combining_clusters`; `native_vs15_color_only_fallbacks_stop_requesting_frames_after_resolution` |
| B minimap clicks | `view.rs:pointer_with_option`, `MinimapGeometry::visual_at`: outside clicks centre the row drawn beneath the pointer; fourth-pass anchored dragging above replaces the initial outside-box grab offset | `minimap_clicks_centre_the_drawn_row_before_starting_a_continuous_drag`; retained long/wrapped/folded end-to-end drag table |
| C blocked queue | `workspace.rs:poll`: overlay newer UI values during pending writes, defer a retry until drain and coalesce completion reloads; `track_settings_write` accepts a batch | `blocked_preference_queue_defers_one_reload_and_tracks_multiple_keys_per_write` (11 tabs, 3-second blocked global queue, external syntax settings, multiple UI keys and one final reload); retained rapid zoom/poll tests |
| D unstable border test | Wait for actual language/highlight/minimap publication before comparing scenes | `minimap_border_draws_four_viewport_edges_without_hover_through_dispatch` |
| E empty fold variant | Install a real fold via `set_folded` and assert the count is nonzero | `minimap_drag_reaches_both_ends_without_jumping_in_long_wrapped_and_folded_documents` |
| F unrecoverable old View zoom | Document the absent serialized View-layer zoom information; retain the available effective-settings migration | Documentation note below |
| Main: syntax status click | `app_mouse.rs:route_mouse_event`: opening a picker does not reveal the caret; mouse-up does not request a reveal | `status_bar_syntax_picker_preserves_the_scrolled_view_on_press_and_release` |
| Main: repaired User JSON | `workspace.rs:Configuration::load`: record watched file stamps before parsing, including invalid resources | `fixed_user_json_reloads_after_an_in_place_repair` (repairs the file without changing its directory timestamp) |

Lane A API update: `Workspace::track_settings_write(rx, &[(String, Option<Value>)])` now
tracks **one receiver and one generation for the whole write**, including every affected
key. `Some(value)` protects that value; `None` protects removal. Existing zoom and spelling
writers use the same serial queue and atomic writer; both call sites pass a one-key batch.
The round-19 appearance pickers can pass all their keys in one slice.

VS15 prefers an already loaded text face, otherwise CoreText resolves the entire sequence
using the primary text cascade. A returned color face is accepted when text coverage is
unavailable. Results, including unavailable/unsuitable faces, settle in the presentation
cache; later frames neither re-request nor invalidate. The native repro covers watch,
thermometer, desktop computer and mixed ASCII/watch for 64 subsequent 8ms frames each.
The mocked table additionally covers text/emoji presentation, keycaps, flags, ZWJ/modifiers,
private-use/unassigned scalars and a lone combining mark. Original tiny fixture fonts cover
their control/dotted-circle glyphs so tests require actual non-notdef glyphs when covered.

A legacy main session saved after **View-layer font zoom** contains neither that override
nor its effective line height. Exact migration of those pixels is impossible from the
wire data alone. Migration uses the available restored settings and the old height formula;
new `scroll_line` sessions carry the row/fraction and have no such ambiguity.

## Third verification validation (historical, main 58fd98c)

The full gate passes: fmt; strict release/locked workspace clippy over all targets;
`cargo test --workspace --locked --release` at default parallelism (**966 passed,
0 failed, 18 ignored; 114 test executables**); and
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`.
Logs: `fmt.log`, `clippy.log`, `workspace-tests.log`, `rustdoc.log`, `candidate-build.log`
under `/tmp/velocity-lane-b-third/`. That pass's changes were uncommitted above `00e1544`;
the user subsequently committed them as `7925499`. No reset, amend, commit or rebase was
performed by the agent.

The complete `app::lane_b_round17_tests` module passed **10/10 runs** with
`--test-threads=4`: 28 passed, 0 failed, one ignored timing test each time. Raw runs and
summaries are in `stability-01.log` … `stability-10.log` and `stability.json`.
The font convergence tests also pass in the workspace gate, including the real CoreText
repro, and the original typed settings/menu/geometry/Vintage regressions remain passing.

All **14 ignored application timing/stress tests** (our previous 11 plus Lane A's 3
round-18 input timings) and **2 ignored renderer/font tests** pass explicitly and serially.
Native FSEvents and Lane A's separate completion-worker timing remain ignored by the
default gate. Timing evidence: `ignored-app.log`, `ignored-render.log`.

Nine alternating pairs use the unchanged supplied `paired-headless.py`, with the existing
wrapper sampling retired instructions/cycles of the same child processes before reaping.
Counters include startup, warmup and background work. The baseline is archived main
`58fd98c`, built in an independent target directory; both binaries use the identical pinned
syntax checkout and 10 MB fixture. Raw wall samples, hashes and whole-process counters
are retained in `paired/`; the wrapper is `/tmp/velocity-lane-b-review/paired-instructions.py`.

| Retired instructions, nine-pair median | Main 58fd98c | Candidate | Change |
| --- | ---: | ---: | ---: |
| features | 240,072,549,453 | 239,229,283,239 | -0.35% |
| typing1 | 425,771,966 | 428,125,944 | +0.55% |
| typing1000 | 10,638,638,999 | 10,649,783,449 | +0.10% |

All three instruction workloads pass the 5% gate. One-minute load ranged 2.37–5.91 on 8 logical CPUs. All wall-clock percentiles also pass the 5% gate.

Baseline SHA-256: `75a08653f8c08380738251283b7bdff41a5c164c14ef4e8893a2667725aaef4d`.
Candidate SHA-256: `a9e4e98565dc2d60c0a15e9a66e609af79c5cc2c6fb17a53ff8330359b5664ae`.

| Wall CPU phase, p50 / p95 / p99 milliseconds | Main | Candidate | Change |
| --- | --- | --- | --- |
| `features/highlight_minimap_scroll_cpu/stats_ms` | 0.291542 / 0.360621 / 0.401479 | 0.268729 / 0.335325 / 0.388393 | -7.82% / -7.01% / -3.26% |
| `features/highlighted_typing_cpu/edit_stats_ms` | 0.049875 / 0.074396 / 0.085169 | 0.049396 / 0.071089 / 0.083047 | -0.96% / -4.44% / -2.49% |
| `features/highlighted_typing_cpu/scene_stats_ms` | 0.779813 / 1.239044 / 1.295470 | 0.776917 / 1.277423 / 1.310029 | -0.37% / +3.10% / +1.12% |
| `features/highlighted_typing_cpu/stats_ms` | 0.833834 / 1.303758 / 1.367550 | 0.832333 / 1.338252 / 1.375262 | -0.18% / +2.65% / +0.56% |
| `typing1/headless_summary/edit_and_scene_cpu_ms` | 0.203354 / 0.342983 / 6.707178 | 0.203500 / 0.342806 / 6.677633 | +0.07% / -0.05% / -0.44% |
| `typing1000/headless_summary/edit_and_scene_cpu_ms` | 5.312708 / 14.798090 / 27.353069 | 5.317209 / 14.611033 / 27.195342 | +0.08% / -1.26% / -0.58% |

The explicitly rerun native font timing test below is one process (not a replacement for
the historical nine-process font comparison). Line metrics and the mapping policy remain
unchanged. Native presentation/GPU checks remain pending in `MANUAL_TEST.md`.

First non-ASCII call **0.432 ms**, fallback ready **57.550 ms**; RSS before primary / after primary / after resolved glyph **7.203 / 7.578 / 19.953 MiB**. Menlo 13 remains **16 px at 1×, 15.5 px at 2×** before padding.

| Font | Unique run µs | Cached run µs |
| --- | ---: | ---: |
| Menlo | 0.117 | 0.063 |
| FiraCode-Regular | 26.123 | 0.110 |

Ragged selection budgets pass at all sizes (real Select All and cached contour reuse):

| Visible rows | Points | Triangulation µs | Plain scene µs | First selected µs | Cached selected µs |
| --- | ---: | ---: | ---: | ---: | ---: |
| 50 | 510 | 9.610 | 612.269 | 144.459 | 108.093 |
| 200 | 2010 | 37.234 | 418.582 | 519.375 | 449.378 |
| 1000 | 10010 | 189.944 | 2198.389 | 2573.833 | 2345.160 |

No preference defaults change in this pass. B6 wrapping remains unconfirmed and unchanged.
The legacy View-zoom ambiguity is documented above; all remaining requested findings and
both local main issues are fixed. Lane A overlay/hover/navigation/editing files are untouched.

| Item | Implementation | Headless verification |
| --- | --- | --- |
| B1 centred wrap | `view.rs:effective_wrap_columns`, `margin`, `motion_layout_options` | `zero_wrap_width_and_centered_geometry_share_columns_with_hit_testing`; randomized sweep |
| B2 fallback | `vel-platform/fonts.rs:FontFallback`, `FallbackCache`, `AsyncFontFallback`; `font.rs:fallback_face`, `poll_fallbacks` | `scripts_and_unavailable_private_use_are_cached_independently`; `missing_scripts_resolve_lazily_once_per_scalar_and_load_one_mapped_file` (all requested scripts plus private use, mocked service) |
| B3 ligatures/options | `typography.rs:FontOptions`, `grid_x`; `font.rs:shape_styled`; `view.rs` token boundaries; `gpu.rs:prepare` cache keys/raster flags | GSUB-only capability gate (Menlo AAT features retain the fast path); original generated GSUB font: `features_shape_ascii_cache_runs_and_split_ligature_width_on_the_caret_grid`; option/metric table; actual scheme-loader test |
| B4 font files | `vel-platform/fonts.rs:map_font_file`; `font.rs:load_face`, `Face::load`; immutable system maps, copied replaceable fonts | `font_file_snapshots_are_deduplicated_without_loading_fallbacks`; mocked script test loads exactly one extra file snapshot |
| B5 zoom | `workbench.rs:zoom_font`, queued atomic preference writer; `app.rs:handle_named_action` | `zoom_key_dispatch_persists_user_preference_and_reset_removes_or_uses_default`: real keymap, multiple tabs, rapid writes, reload, fractional reset, unrelated settings/comments |
| B6 opportunities | `layout.rs:make_line` retains existing whitespace/fallback opportunities | `randomized_wrap_fold_wide_and_tiny_geometry_stays_in_bounds` (300 deterministic cases) |
| B7 metrics | `typography.rs:FontMetrics::line`; `view.rs:set_font_metrics`, `baseline`, `apply_style`; native scene wiring | `font_metrics_drive_baseline_carets_gutter_scroll_and_minimap`; fractional-metrics table; actual Menlo timing probe |
| C1 new tabs | `workbench.rs:apply_window_layers`, `refresh_window_layers`; `Tab::new` integration | `new_open_clone_move_and_session_tabs_share_window_layers_and_separate_modes`: new/open/clone/group move/session wire round trip; existing clone/spelling regression |
| C2 minimap | `view.rs:MinimapGeometry`, `minimap_geometry`, `paint_minimap`, `pointer_with_option`; cached `layout.rs:Segment` columns | `minimap_mapping_table_short_long_wrapped_and_scrolled`; randomized fold/wide sweep; scene benchmark; normal files consume existing overview samples, large-file fallback respects its stride |
| C3 window toggles | `workbench.rs:WindowLayers`, `set_window_toggle`; `panels.rs` toggles; additive `vel-session::Window` fields | real named dispatch and session round trip above; all menu/palette commands resolve |
| C4 scroll distance | `settings.rs:deserialize_scroll_past_end`; `view.rs:max_scroll_y` shared by clamp, track and thumb | real settings table accepts booleans/fraction/clamps; `fractional_scroll_distance_and_paging_use_the_real_viewport` |
| C5 viewport/border | `Preferences.sublime-settings`; `view.rs:paint_minimap` | `minimap_viewport_default_and_borders_are_independent`; fresh bundled startup |
| C6 simple full screen | `vel-platform/macos.rs:toggle_full_screen_with_mode`, borderless key-capable subclass, saved frame/style/presentation; `app.rs` native and distraction-free dispatch | `preference_changes_and_distraction_entry_share_the_fullscreen_transition_table`; native transitions pending below |
| D4 bracket glyphs | typed `ColorScheme` accessors in `view.rs:bracket_glyph_styles`, `paint_brackets`, `decoration_underline` | options table (unknown/empty/foreground never underlines); `real_scheme_loader_drives_bracket_glyphs_glow_and_connected_selection_corners` |
| D5 glow | `view.rs:flush` only appends glow runs when requested; `gpu.rs:prepare`, `quads.wgsl` glyph-mask pass; split translation | actual scheme-loader test checks glowing runs share glyph-run coordinates; headless shader validation; native GPU checks pending |
| D7 selection/caret corners | typed scheme accessors; `selection.rs:outlines`, `corners`, `triangulate`; `view.rs:scene`, long-line projection; GPU contour fill/edge pass | connected outline geometry table (square/round/cut, zero/normal/huge radii, concave joins/disconnected rows); real scheme loader; explicit timing test |
| A3 scroller | `vel-platform/mouse.rs:MousePreferences::read`, `jumps`; `view.rs:pointer_with_option` | Option inversion table; actual track geometry click test including fractional end distance |
| A4 selected-text delay | `MousePreferences::drag_ready`; separate `app.rs:text_drag_delay_elapsed` gate in existing mouse dispatch | exact delay-boundary table; `selected_text_drag_waits_for_the_configured_delay_through_mouse_dispatch` |
| F1 menu/palette integration | original View > Indentation rows and palette entries, `set_translate_tabs: true` | `indentation_menu_arguments_dispatch_and_undo_as_one_edit`; every menu and palette command resolves |
| F4 rulers | `settings.rs:parse_rulers` keeps typed columns compatible with commands and stores drawing metadata; `SolidRect::patterned_ruler` shares stippled shader | malformed-entry isolation table, real settings loader; shader validation |

The window-layer installer deliberately skips identical replacements, preserving shared
settings identities across clones. The existing Vintage `change_ran || steps > 0` logic
is untouched. The old structural theme golden remains byte-identical; the snapshot adapter
can now record connected contours when a fixture contains them.

## Verified public behavior and retained limits

- [Distraction-free docs](https://www.sublimetext.com/docs/distraction_free.html): zero wrap
  width means window width; chrome choices are remembered separately in that mode.
- [Ligature docs](https://www.sublimetext.com/docs/ligatures.html): ASCII-symbol sequences
  stay within one syntax token; `no_liga`, `no_clig`, `no_calt`, `dlig`.
  [Fira Code's author documentation](https://github.com/tonsky/FiraCode/wiki/How-to-enable-stylistic-sets)
  confirms stylistic sets. Default fonts without matching features retain the ASCII fast path.
- [Current settings reference](https://docs.sublimetext.io/reference/settings.html#visual-settings):
  `always_show_minimap_viewport` defaults to false. This is the explicit visible preference
  default change. `draw_minimap_border` gates the viewport border. Grayscale and disabled
  antialiasing are supported; the existing macOS grayscale pipeline remains for subpixel
  requests. Windows-only renderer switches are inapplicable to this macOS implementation.
- [Settings reference](https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/reference/settings.html#automatic-behavior)
  says `word_separators` does not control wrapping. Current documentation/changelog does not
  confirm the proposed B6 rule. Existing whitespace/fallback wrapping remains unchanged.
- [ST4 changelog](https://www.sublimetext.com/download): numeric scroll distance (4107),
  drag delay (4121), horizontal minimap scrolling and ruler style (4142), scrollbar system
  preference (4151), ruler width (4166), default font size (4200).
- [Reset command reference](https://docs.sublimetext.io/reference/commands.html#reset-font-size):
  reset removes the user size. [Indentation docs](https://www.sublimetext.com/docs/indentation.html)
  establish the conversion/detection command names and menu placement.
- Simple full screen is confirmed by the public settings reference and
  [Sublime's maintainer clarification](https://forum.sublimetext.com/t/build-4180-cannot-type-in-full-screen-mode/73072/17).
  It remains opt-in. All transitions require native review.
- Native fallback visibility depends on installed coverage; unavailable private-use scalars
  remain missing. CoreText work is asynchronous. The first frame may contain a temporary
  missing glyph until the requested face is ready; no blocking cascade is on that path.

## Verification-pass fixes

This pass addresses the eight remaining findings. Confirmed behaviors from the first review
remain covered by their tests below. There is no rebase onto Lane A's newer main and no commit.

| Finding | Fix (file:function) | Headless test |
| --- | --- | --- |
| 1 minimap range/release jump | `view.rs:MinimapGeometry::{new,scroll_at,viewport}`, `pointer_with_option`: inverse of live box travel; retain only the pixel grab offset | `minimap_drag_reaches_both_ends_without_jumping_in_long_wrapped_and_folded_documents`; `minimap_drag_paint_and_hit_testing_share_the_live_inverse` |
| 2 captured gesture swallowed | `app_mouse.rs:handle_mouse_event`, `route_mouse_event`, `end_mouse_gesture`: captured drag/up bypass panels; up clears all view/text gestures and drop carets; a new press cancels interrupted capture | `minimap_release_over_status_bar_ends_capture_before_wheel_and_text_drag` |
| 3 rapid zoom reload rollback | `workspace.rs:track_settings_write`, `poll`: generation/pending-state snapshot for each resource load; overlay newer own keys until a settled load; both UI writers register their keys | `repeated_zoom_with_polling_preserves_every_press_and_then_applies_external_preferences` (8/33 ms, poll between all presses, spelling interleave, subsequent external edit) |
| 4 custom-font old sessions | `workspace.rs:Tab::restore_scroll`, `workbench.rs:restore_window`, `ViewState::top_visual_line`: old height = 20 × clamp(font_size,6,96) / 13 + nonnegative top/bottom padding, from the restored view's layers, including syntax and mode | `legacy_session_pixels_use_the_restored_font_size_and_padding`; `legacy_lazy_session_normalizes_scroll_before_zoom_and_resave`; retained default/current-format round trip |
| 5 mapping immutability | `vel-platform/fonts.rs:MappingPolicy::permits`, `font_volume`, `map_font_file`: canonical path, `statfs` read-only/fsid and `/usr/share/firmlinks`; copy AssetsV2 and Data/user files | `canonical_font_mapping_policy_uses_mocked_mounts_and_firmlinks`; retained in-place font truncation test |
| 6 VS15 order dependence | `font.rs:Face::load`, `fallback_face`: color-table capability cached per face; explicit text presentation prefers a loaded text face; the third pass additionally caches resolved color-only answers | `text_presentation_ignores_loaded_and_cached_emoji_faces_in_either_order` (real shaping, mocked text/emoji platform cascade) |
| 7 shortened file restore | `view.rs:restore_scroll_position`, `finish_scroll_restore`; `app.rs:sync_active_view` and background pane restoration clamp only after complete document geometry is available | `restored_scroll_clamps_when_the_saved_file_shrank`; `restored_scroll_waits_for_background_file_loading_before_clamping` |
| 8 Lane A navigation API | `View::scroll_position() -> ScrollPosition { x, visual_row }`; `View::restore_scroll_position(&Editor, ScrollPosition)` uses current metrics and clamps | `visual_row_scroll_api_restores_through_zoom_and_scale_changes` |

`visual_row` is the top visual row plus its fractional offset; `x` remains logical pixels.
Lane A can replace saved pixel tuples in jump history/Goto preview with this value and use
`restore_scroll_position` when accepting or cancelling navigation. This pass leaves
`intelligence.rs` and the overlay/quick-panel implementation untouched. Background file loading
uses the separate deferred restore internally so an unavailable prefix cannot erase the saved row.
Legacy lazy sheets are normalized immediately; saving after zoom cannot remigrate old pixels
against the new size on the next startup.
The minimap clamp probes now remain inside its own bounds, and release tests compare actual
scene rectangles before/after release. No test asserts a frozen map or a swallowed early drag.

## Retained first-review corrections

| Finding | Fix (file:function) | User-path / geometry test |
| --- | --- | --- |
| 1 minimap feedback | `view.rs:pointer_with_option`, `minimap_geometry` invert the live viewport travel using only pointer/grab and the shared proportional geometry | `minimap_drag_reaches_both_ends_without_jumping_in_long_wrapped_and_folded_documents`; `minimap_drag_paint_and_hit_testing_share_the_live_inverse` |
| 2 last line hidden | `view.rs:max_scroll_y` uses fraction × (viewport − line height), shared with thumb | `scroll_past_end_and_scrollbar_drag_keep_the_last_line_visible`; corrected `fractional_scroll_distance_and_paging_use_the_real_viewport` |
| 3 emoji presentation | `font.rs:fallback_face` requests a whole emoji cluster before text cmap coverage; `fonts.rs:resolve_cluster` starts with Apple Color Emoji | `emoji_presentation_cluster_table_precedes_primary_text_coverage` (mock cascade, VS16/keycaps/ZWJ/flags/modifiers, text forms and Indic joiners) |
| 4 contour complexity | `selection.rs:triangulate` merges monotone chains in O(n), including horizontal ties; `view.rs:scene` caches immutable contour geometry by selection, scroll and layout revision | `monotone_triangulation_matches_ragged_outline_area_in_a_randomized_sweep`; `borderless_round_selection_reuses_contours_and_square_uses_rectangles`; both ignored ragged benchmarks |
| 5 scroll persistence | `ViewState::scroll_line` stores visual row + fraction, `with_line_height` writes the legacy field in the old grid; `workbench.rs` migrates old pixels with the effective legacy font/padding height; `view.rs:apply_style` converts live scroll; app tab storage retains its grid | `session_scroll_migrates_old_pixels_and_round_trips_visual_rows`; `zoom_and_scale_preserve_active_and_background_top_rows_and_new_view_metrics` |
| 6 font truncation | `fonts.rs:map_font_file` canonicalizes and checks mount identity/read-only flags and the host firmlink table; sealed system fonts are mapped, writable assets/user files are copied | `replacing_user_font_in_place_keeps_shaping_the_loaded_snapshot` (actual load, in-place truncation, fresh shape); retained dedup/fallback tests |
| 7 preference race | `workbench.rs:zoom_font` uses `release::enqueue` + `vel_session::atomic_write`, with no backup file | `zoom_interleaved_with_ui_writes_preserves_both_preferences_without_backups` |
| 8 minimap outline | `view.rs:paint_minimap` draws the viewport outline independently of hover/fill | `minimap_border_draws_four_viewport_edges_without_hover_through_dispatch`; corrected viewport/border table |
| 9 borderless corners | `view.rs:scene` applies corner style to fill and uses the cache; square/no-border stays rectangular | `borderless_round_selection_reuses_contours_and_square_uses_rectangles`; corner geometry table |
| 10 first-frame metrics | `app.rs:initialize_view_metrics`, `sync_active_view` install shared metrics before input/reveal, including `output_view` | `never_shown_tab_reveal_and_output_use_metrics_before_the_first_scene` |
| 11 drag delay | `MousePreferences::text_delay` reads milliseconds; `app.rs:select_before_text_drag_delay` commits early movement to selection; quick release places caret | corrected `selected_text_drag_waits_for_the_configured_delay_through_mouse_dispatch`; `releasing_selected_text_before_drag_delay_places_the_caret`; native unit/boundary table |
| 12 numeric rulers / zoom / side bar | `settings.rs:parse_rulers` accepts a number; zoom uses the non-mode User preference; side-bar effects publish window toggles; reset test uses an actual user file | `numeric_ruler_and_distraction_zoom_use_the_real_settings_loader`; `sidebar_reveal_and_add_folder_remember_the_current_window_mode`; corrected zoom persistence test |

For zoom with an explicit distraction-free `font_size`, this review chooses the user
(non-mode) size as the value to change and persist. The explicit mode override still
applies while that mode is active; leaving it reveals the zoomed user size. Project,
syntax and view font overrides likewise retain their precedence. No documentation found
specifies zoom's treatment of a mode override, so this choice avoids writing a mode size
into unrelated User preferences.

The compiled globals currently provide no `selection_border_width` value. Drawing consults
`Globals::number` and honors it when available, otherwise retaining the existing 1-point
border. This lane does not parse an unknown scheme key or change Lane A's global compiler.

`NSDragAndDropTextDelay` uses **milliseconds**, confirmed by the public [archived Apple
Xcode reference](https://leopard-adc.pepas.com/documentation/DeveloperTools/Reference/XcodeUserDefaultRef/100-Xcode_User_Defaults/UserDefaultRef.html)
and [TextMate's own manual](https://macromates.com/manual/en/expert_preferences#nsdraganddroptextdelay).
The latter also documents early movement selecting and negative values disabling drag;
[Sublime's maintainer](https://forum.sublimetext.com/t/nsdraganddroptextdelay/61519/2)
confirms zero removes the delay. Unset still retains the prior distance-only behavior.

## Previous verification measurements (historical, d442835)

The final full gate passes: `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets --release --locked -- -D warnings`,
`cargo test --workspace --locked --release` (**913 passed, 0 failed, 14 ignored; 114
executables**, default parallelism), and `RUSTDOCFLAGS="-D warnings" cargo doc --workspace
--no-deps --locked`. All **11 ignored application timing/stress tests** and **2 ignored
render/font timing tests** pass explicitly, serially; native FSEvents remains ignored.
The fresh-start messages check and retained round-16/17 regressions pass. Final logs:
`verification-{fmt,clippy,workspace-tests,rustdoc,candidate-build}.log`,
`verification-ignored-{app,render}.log`; measurements and harnesses are retained beside them
under `/tmp/velocity-lane-b-review/`. `verification-original/` preserves the incoming patch
and untracked files. Earlier `paired-final/` and `font-pairs.json` measurements belong to
the preceding review and are superseded by this section.

Menlo 13 remains **16 logical pixels at 1×** and **15.5 logical pixels at 2×** (16/31
physical pixels), before existing nonnegative top/bottom padding. Main's old grid was
`20 × font_size / 13 + padding` (20 logical pixels at size 13). Ascent is
12.06689453125, descent 3.06591796875, leading 0. Legacy migration uses that old effective
height; current sessions retain a visual row/fraction and are never migrated a second time.
Synthetic views without a font snapshot retain the historical 20-pixel test grid.

The supplied `tools/scroll-gate/paired-headless.py` runs unchanged, wrapped only to sample
Darwin `rusage_info_v4.ri_instructions` and cycles after `waitid(WEXITED | WNOWAIT)` and
before reaping those same processes. Counters include startup, warmup and workers. All
three workloads pass the ≤5% retired-instruction gate over nine alternating pairs against
`d442835`. Raw samples: `paired-verification/`; wrapper: `paired-instructions.py`.

| Median retired instructions, 9 pairs | Main `d442835` | Candidate | Change |
| --- | ---: | ---: | ---: |
| features | 242,012,391,079 | 240,956,808,192 | -0.44% |
| typing1 | 426,060,308 | 428,056,926 | +0.47% |
| typing1000 | 10,687,909,007 | 10,704,882,670 | +0.16% |

One-minute load ranged 5.07–14.35 on 8 logical CPUs. Retired instructions are the acceptance boundary under the requested busy-machine rule.
The wall gate has these >5% results (retained in `report.json`): `features/highlighted_typing_cpu/stats_ms/p50` +7.02%; `features/highlighted_typing_cpu/edit_stats_ms/p95` +7.67%; `typing1/headless_summary/edit_and_scene_cpu_ms/p95` +6.32%. All other wall percentiles pass.

Baseline SHA-256: `998b9b1d09c5fd22086b0893383d1eae598a6b85ab9413bbf75e65616a931550`.
Candidate SHA-256: `825cf77310b26cf0f048b3b8078de7964cd1e54652c2e02c5bf77669fbe8290c`.

| CPU phase, milliseconds (p50 / p95 / p99) | Main | Candidate | Change |
| --- | --- | --- | --- |
| `features/highlight_minimap_scroll_cpu/stats_ms` | 0.269917 / 0.339982 / 0.380336 | 0.252500 / 0.310173 / 0.346069 | -6.45% / -8.77% / -9.01% |
| `features/highlighted_typing_cpu/edit_stats_ms` | 0.068833 / 0.088971 / 0.120287 | 0.059688 / 0.095798 / 0.113129 | -13.29% / +7.67% / -5.95% |
| `features/highlighted_typing_cpu/scene_stats_ms` | 0.842854 / 1.285528 / 1.356102 | 0.801208 / 1.307390 / 1.346130 | -4.94% / +1.70% / -0.74% |
| `features/highlighted_typing_cpu/stats_ms` | 1.065855 / 1.365477 / 1.444090 | 1.140626 / 1.393188 / 1.435502 | +7.02% / +2.03% / -0.59% |
| `typing1/headless_summary/edit_and_scene_cpu_ms` | 0.226000 / 0.365300 / 7.051967 | 0.232000 / 0.388379 / 7.006931 | +2.65% / +6.32% / -0.64% |
| `typing1000/headless_summary/edit_and_scene_cpu_ms` | 5.460938 / 15.122731 / 27.911132 | 5.485563 / 15.075364 / 27.915812 | +0.45% / -0.31% / +0.02% |

The font probes compile the actual main/candidate font and native sources with the same
harness/dependencies, in separate fresh processes. Nine pairs alternate order; OS caches
are warm. They measure font loading/shaping independently of editor startup.
Sources and raw evidence: `font-{base,candidate}/`, `font-target/`,
`verification-font-pairs.{py,json,log}`. The synchronous call may display `.notdef` for the
first frame; worker completion invalidates it and repaints without another input event.

| First non-ASCII `你`, median of 9 | Main | Candidate |
| --- | ---: | ---: |
| Synchronous first-glyph call (ms) | 12.960 | 0.050 |
| Fallback ready (ms) | 12.961 | 9.534 |
| RSS before primary load (MiB) | 6.453 | 6.438 |
| RSS after primary load (MiB) | 6.703 | 6.766 |
| RSS after resolved glyph (MiB) | 68.906 | 15.578 |
| RSS growth on first glyph (MiB) | 62.203 | 8.812 |

CoreText resolves `你` to the **61,081,560-byte** private font, canonically
`/System/Library/PrivateFrameworks/FontServices.framework/Versions/A/Resources/Reserved/PingFangUI.ttc`.
It is on the sealed read-only system mount (fsid 16777235/26), so it is mapped.
The other PingFang file actually present is the **78,222,888-byte**
`/System/Library/AssetsV2/com_apple_MobileAsset_Font8/86ba2c91f017a3749571a82f2c6d890ac7ffb2fb.asset/AssetData/PingFang.ttc`;
it is on Data (fsid 16777230/26) through a firmlink and is copied.
[Apple's statfs reference](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/statfs.2.html)
documents mount identity and read-only flags; path prefix and ownership alone do not decide.

Direct `Fonts::load_file` measurements below are nine fresh-process medians, followed by
shaping `你`, using `.PingFangUITextSC-Default` and `PingFangSC-Regular` respectively.
These file-load measurements exclude the asynchronous cascade/native-table identity work
included in the preceding first-glyph table. Raw canonical paths, statfs values and every
sample are in `verification-font-paths.{py,json,log}`.

| Actual file | Policy | Load ms | RSS before / after load / after shape, MiB |
| --- | --- | ---: | --- |
| Private PingFangUI.ttc | mapped | 0.160 | 6.438 / 6.953 / 7.312 |
| AssetsV2 PingFang.ttc | copied | 11.066 | 6.438 / 81.344 / 81.844 |

Fira Code 6.2 is an external OFL timing input, never bundled or installed. The explicit
font timing test averages 4,000 unique punctuation runs and 20,000 cached repetitions;
shaping only, excluding rasterization and GPU. The one-shot ignored-test first-glyph/RSS
sample is retained in its log; the nine-process medians above are the reported comparison.

| Font | Unique run µs | Cached run µs |
| --- | ---: | ---: |
| Menlo | 0.120 | 0.065 |
| FiraCode-Regular | 26.919 | 0.113 |

The ragged scene test dispatches Select All in actual 1400-wide views containing 50/200/1000
visible rows. First-selected and unchanged-selected scenes meet their relative plain-scene
budgets. Unchanged contours share point/triangle allocations. Scene numbers include ordinary
CPU scene work; isolated O(n) triangulation is averaged over 1,000 iterations per size.

| Visible rows | Points | Triangulation µs | Plain scene µs | First selected µs | Cached selected µs |
| --- | ---: | ---: | ---: | ---: | ---: |
| 50 | 510 | 9.880 | 104.760 | 150.417 | 113.213 |
| 200 | 2010 | 38.006 | 468.359 | 553.542 | 491.295 |
| 1000 | 10010 | 190.221 | 2388.741 | 2656.167 | 2494.372 |

Retained ordinary scene/glow staging timings:

```text
SCENE_TIMING selected=false us=47.679978999999996 rects=3 contours=0
SCENE_TIMING selected=true us=47.8914585 rects=2 contours=1
GLOW_STAGING glow=false us_per_run=0.06871875
GLOW_STAGING glow=true us_per_run=0.12664794999999998
```

Native appearance/GPU/window-system checks remain open in `MANUAL_TEST.md` (no GUI).
No new defaults change in this verification pass. `always_show_minimap_viewport: false`
remains the documented default. B6's proposed wrapping opportunities remain unconfirmed
and unchanged. Lane A's navigation stores remain untouched; adopt the visual-row API
listed in the verification table above during its jump-history work.
