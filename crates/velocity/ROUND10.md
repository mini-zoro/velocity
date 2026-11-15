# Round ten: themed UI and minihtml

Lane A only, no commit. `vel-theme`, `vel-minihtml`, their original bundled theme resources and
lane B's syntax/scope/regex/vintage crates are consumed read-only. Syntax staging remains in place.

The UI now uses the original slate/light Velocity themes for sidebar rows, tabs, quick panels,
search inputs/buttons, completion menus, context menus, tooltips, status controls, fold icons,
minimap viewports and scrollbars. Actual ancestor/state paths reach the resolver every frame.
Compiled paint styles, immutable settings projections, decoded density variants and parsed HTML
are retained. Theme fragments and file-icon mappings follow package/User precedence. Appearance,
UI scaling, fonts, content margins, layer tints/opacities, finite transitions, PNG sequences and
nine-slice patches are applied by the host; the native title-bar view has its own theme appearance
so the document still receives live OS appearance notifications.

Widgets whose paths cannot match a settings-dependent rule use a settings-free compilation of
the same resource graph. Conservative class/attribute/ancestor guards retain the full engine for
affected widgets; selectors the host cannot conservatively index retain the full resolver. Both
paths still call the engine resolver every frame and are compared in tests. The editor painter streams ordered style spans
alongside graphemes instead of restarting its span search for each character.

Definition hover and completion/quick-panel details use minihtml. Popups support documented
flags, viewport limits, scrolling, command/browser/callback navigation and title hit regions.
Named phantom sets reserve inline width or below/block rows and use tracked buffer regions.
Annotations align to region ends and the right edge without inserting rows. Build diagnostics
use that annotation painter and escape diagnostic text. HTML sheets occupy ordinary workbench
tabs, scroll, navigate links and retain their name/content through Hot Exit as read-only sheets.
Transient popup/phantom content is reconstructed by commands after a restart.

The metrics adapter uses the renderer's font stack and proportional UI shaping in native mode.
Headless modes explicitly use deterministic metrics and never initialize AppKit or Metal. Local,
package and PNG/JPEG/GIF data images decode on bounded workers, invalidate retained layouts and
use the existing atlas. Large HTML bitmaps are downsampled to at most 2044 pixels on either axis
while preserving their intrinsic layout dimensions, then uploaded in generation-safe atlas tiles.
The existing eight-page atlas budget also applies to images.

Public behavior references: [themes](https://www.sublimetext.com/docs/themes.html),
[minihtml](https://www.sublimetext.com/docs/minihtml.html),
[popup/phantom/annotation/sheet APIs](https://www.sublimetext.com/docs/api_reference.html).
No application-only theme, art, Python module or executable was inspected or copied.

## Evidence and checks

Measurements and raw-run locations are recorded in `timings-round10.json` and the round-ten
section of RESULTS.md. CPU popup/annotation/switch measurements exclude native shaping,
uploads, GPU execution and display scanout. Native evidence remains the reviewer's task.

- [x] Theme-driven widget paint paths, stateful transitions and density-aware texture atlas path.
- [x] Minihtml popups, inline details, phantoms, annotations, build errors and HTML sheets.
- [x] Structural CPU goldens for dark/light editor, Goto and HTML sheet scenes.
- [x] Popup lifecycle/link/clip/scroll tests, multiline baseline regression, phantom edit/undo
  tracking, annotation row invariance, image decoding, scale/IME geometry and HTML sheet restore.
- [x] Missing/read-only/corrupt profile coverage retained; invalid User themes fall back with a diagnostic.
- [x] Workspace fmt, strict clippy, workspace tests, final six-crate tests and strict rustdoc.
- [x] Ten-pair isolated CPU A/B and five popup/annotation/theme-switch budget processes;
  point estimates pass, with confidence intervals and raw samples retained in RESULTS.md.
- [x] Lane-A-only changes, unchanged syntax staging, no GUI/Metal execution and no commit.
- [ ] Human verification of native fonts, Retina density selection, OS appearance/title bar,
  hover/press/fade quality, links, IME, Metal validation and native CPU/GPU/scanout budgets.

Use `scripts/review-round10-gui.zsh` and MANUAL_TEST.md for the native checklist. Run all required
format/lint/test/rustdoc commands from RUNBOOK.md before review; this file does not substitute
for their actual exit status.
