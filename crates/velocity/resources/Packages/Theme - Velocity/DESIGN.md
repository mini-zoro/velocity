# Velocity UI

An original, quiet UI built around the existing Velocity color schemes. The chrome (title bar,
tab strip, side bar, status bar, panels) is derived from the bundled scheme backgrounds: in dark
mode it is the scheme background darkened by 25% (`surface` `#0f1a22`), floating panels lighten
it by 5% (`overlay`); in light mode chrome darkens it by 6% (`#e8e6df`) and floating panels are
60% towards white. They are plain variables, so user themes can override `layer0.tint` or the
variables. Only the selected tab uses the live scheme background (`tint_index` 0), so it always
matches the sheet below it. The host selects `Velocity.sublime-theme` or
`Velocity Light.sublime-theme` from OS appearance; the light variant inherits the full rule set
and overrides variables, title-bar style and content darkness.

Structure comes from hairlines rather than frames: one-DIP `line` edges between the side bar
and the editor, under the tab strip, above the status bar and above panels, and a short divider
between inactive tabs. The selected tab takes the editor background so it reads as part of the
sheet below it; the focused group's selected tab also carries a two-DIP accent bar. Inactive tab
labels are muted. Close buttons appear on the selected and hovered tab; a dirty tab shows a dot
in their place until hovered. Group and Split View dividers use `grid_layout_control`.

Spacing follows a 4-DIP scale: 4 for tight gaps, 8 for standard insets, 12 for tab side insets,
16 for side-bar indentation and icon columns. Tabs are 32 DIPs high; buttons, toggles and tab
controls are at least 20–24 DIPs. Icons are drawn at their intrinsic 16-DIP size and snapped to
device pixels, never stretched. Popup containers and inputs have 4–6 DIP radii; kind badges 3.
UI text uses the platform system family at 12 DIPs (quick-panel captions 13), secondary text and
headings at 11; headings are bold uppercase. Long labels end in an ellipsis. Preview/transient
tabs and open files use italic text.

States: hover is a translucent foreground wash (5–14%), selected rows use the opaque `selected`
mix of chrome and accent, pressed is stronger still. Option toggles that are on are solid accent
with an inverse glyph, so on/off is unmistakable in both appearances; a panel's default (Enter)
action is accent-tinted. Inputs are inset wells with a hairline border that becomes the
accent when focused and red when the value is invalid. Floating surfaces (overlays, popups,
completion, tool tips) have a hairline border and a soft shadow; the border is repainted above
minihtml content because the popup's own background covers the surface. Disabled controls dim.
Dirty tabs can use amber when `highlight_modified_tabs` is set. Kind badges and Git badges use
the palette's rose, violet, blue, amber, green and teal. Text meets WCAG AA (4.5:1) in both
appearances, including secondary text on chrome and on floating panels.

Every entry in the engine's 130-entry registry has a baseline rule stating only what differs
from the engine defaults; the generated theme is written one rule per line to keep start-up
parsing cheap. Twenty-six small textures
are authored in `generate.py`: search toggles, files/folders, disclosure/folding/scroll arrows,
close, new-tab, side-bar and branch icons, plus nine-sliced hairline edges, the tab divider and
the focus indicator. Strokes are one DIP wide and axis-aligned strokes are snapped per density,
so the 1x/2x/3x PNGs are crisp. The script uses only Python's standard library and produces
78 compact, supersampled PNGs. File type icons use the matching `.sublime-file-icons` mapping
rather than overriding the dynamic `icon_file_type` texture in the main theme.

Run `python3 crates/velocity/scripts/theme/generate.py` from the repository
root to reproduce the JSON and PNG resources, then list any new PNG in `theme_ui.rs` (`BUNDLED`).
Existing outputs are backed up to `/tmp/velocity-round8-backups` (or the platform temporary
directory) and replaced atomically. Neither application-only resources nor third-party
artwork/themes are inputs. Public third-party themes are used only by runtime compatibility tests
in `vel-theme`; no textures were copied. `velocity --render-scenes <dir>` renders the UI states
used to review this theme (see `docs/design/ui-audit-r1.md`).
