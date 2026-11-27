# Tab dividers (rounded tabs)

Feedback: with the default `file_tab_style: "rounded"`, unselected tabs had no boundary, and a
dirty dot at a wide tab's far right sat closer to the next tab's icon than to its own name.

Rendered offscreen with `velocity --render-scenes DIR --only tab-dividers-… --scale 2 --appearance
both` (scenes in `crates/velocity/src/render_scenes.rs`): five synthetic tabs, `main.rs` and
`配置.toml` modified, `README.md` selected, the `-hover` scenes rest the pointer on `笔记.md`.

Decisions (2026-11-28, after reviewing the renders):

- Rounded tabs get a short divider between plain tabs. It is 1 DIP wide (2 px at 2x) and 16 of the
  tab's 32 DIPs tall, centred, and hidden on the selected and hovered tabs and on the tab
  immediately left of either, so no line touches a card. Window focus does not change the strip.
- `tab_width` goes from 180 to 140 DIPs, so a dirty dot sits near its own name.
- In the dark theme, `angled` and `square` tabs draw their divider in the rounded tabs' colour;
  `line` was darker than the strip and all but invisible there. The light theme keeps `line`.
- The last tab keeps its right-hand divider, as `square` always did.

Measured on rendered pixels at the tab's centre row (WCAG contrast against the strip):

| Style | Theme | Divider | Strip | Contrast | Before |
| --- | --- | --- | --- | --- | --- |
| rounded | light | `#adb2b0` | `#e8e6df` | 1.72:1 | none drawn |
| rounded | dark | `#374249` | `#0f1a22` | 1.71:1 | none drawn |
| angled, square | light | `#d9d7d1` | `#e8e6df` | 1.15:1 | unchanged |
| angled, square | dark | `#253038` | `#0f1a22` | 1.31:1 | about 1.05:1 |

The `angled` and `square` divider is a 1-DIP slice of `tab-divider.png` that renders at roughly
half strength, which is why it measures below its colour's nominal contrast; its geometry is
unchanged.

Max-width study, measured gap from the end of the name to the dirty dot while the dot stays 27
DIPs from the next tab's icon (`tab-max-width-study@2x.png`):

| `tab_width` | `main.rs` | `配置.toml` | Note |
| --- | --- | --- | --- |
| 180 (before) | 87 DIPs | 77 DIPs | |
| 140 (applied) | 47 DIPs | 37 DIPs | every sample name fits |
| 120 | 27 DIPs | 17 DIPs | the selected `README.md` truncates |
