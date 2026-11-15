# Spec: theme engine + minihtml in the UI (popups, phantoms, annotations, HTML sheets)

Status: P3 / lane A (platform, UI). Locations: lane-A crates (`velocity`, `vel-ui`, `vel-render`,
`vel-platform`, `vel-config`, `vel-commands`, …). Consumes (read-only): `vel-theme`, `vel-minihtml`
and the bundled `Packages/Theme - Velocity/` (lane B).

References (public docs only): themes.html, minihtml.html, the API reference for popups/phantoms/
annotations/HTML sheets (`View.show_popup`, `PopupFlags`, `Phantom`, `PhantomSet`, `PhantomLayout`,
`View.add_regions` with `annotations`/`annotation_color`, `Window.new_html_sheet`), settings
(`theme`, `dark_theme`, `light_theme`, `ui_scale`, `animation_enabled`, `overlay_scroll_bars`,
`themed_title_bar`, `file_tab_style`, `tab_completion`…).

## 1. Theme-driven UI

- Every `vel-ui` widget asks `vel_theme::Resolver::resolve(ElementPath, Settings, Platform)` for
  its style each frame (cached; zero allocations on hit, per lane B's API): layers (tint, opacity,
  textures as nine-slice), content margins, fonts/colors, shadows, `color_scheme_tint`, icons; apply
  the documented attributes (hover, pressed, selected, dirty, highlighted, transient, expandable,
  expanded, left/right, file_light/file_medium/file_dark, …) and settings-driven rules.
- `theme` setting incl. `"auto"` with `dark_theme`/`light_theme` following the OS appearance (live),
  theme override files in User, `ui_scale`, theme animations (opacity/tint transitions) driven by
  the frame clock without keeping the app awake when idle (`animation_enabled`).
- Upload theme textures to the GPU atlas (respect @2x/@3x selection from lane B), nine-slice
  drawing in `vel-render`.
- File icon theme in the side bar, tabs and Goto Anything.
- Visual regression: render key screens headlessly to CPU scenes and compare structural golden
  data (layers/rects/glyph runs), not pixels.

## 2. minihtml in the UI

- Text shaping/metrics provider for `vel_minihtml::layout` using our existing font stack.
- **Popups**: `show_popup` semantics (location, max width/height, flags `COOPERATE_WITH_AUTO_COMPLETE`,
  `HIDE_ON_MOUSE_MOVE`, `HIDE_ON_MOUSE_MOVE_AWAY`, `KEEP_ON_SELECTION_MODIFIED`,
  `HIDE_ON_CHARACTER_EVENT`), scrolling when larger than max height, link navigation (`subl:`
  command URLs run commands; `http(s):` opens the browser) — migrate the existing definition hover
  popup and completion details to minihtml.
- **Phantoms** (`PhantomLayout` `INLINE`, `BELOW`, `BLOCK`) and **annotations** (right-aligned
  inline HTML at region ends, `annotation_color`): migrate build errors (`show_errors_inline`) to
  annotations, as ST4 does ("build systems now use annotations instead of phantoms").
- **HTML sheets** (`new_html_sheet`): a tab that shows a minihtml document (used later for docs/
  release notes).
- Hit testing, tooltips (`title`), selection of popup text not required.

## Budgets

- Theme resolution must not add measurable frame time: scroll/typing within 5% of round 9
  (isolated A/B, as before).
- Popup open (2 KB HTML) ≤ 2 ms to first frame on CPU; 200 annotations visible: frame CPU ≤ 2 ms.
- Theme switch (dark↔light) ≤ 50 ms to the next frame.

## Rules

- Your worktree: /Users/zoro/dev/velocity-lane-a (branch `lane-a`). Do not commit. Modify only lane-A
  crates (+ root `Cargo.lock`). Lane B is working on `vel-vintage`, `vel-syntax`, `vel-regex`,
  `vel-scope` in parallel — don't touch them.
- Fresh/read-only/corrupt profile robustness as always. Headless tests, automatic GUI modes, and an
  updated reviewer script + MANUAL_TEST.md section (the human tester will check the new look).
