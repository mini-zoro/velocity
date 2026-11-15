# Velocity UI

An original, quiet UI built around the existing Velocity color schemes. The dark surface is deep
blue slate (`#14232e`), with pale ink and a cool blue accent. Light appearance uses a near-white
surface (`#f5f7f8`), dark blue ink and the same palette relationships at lower lightness. The host
selects `Velocity.sublime-theme` or `Velocity Light.sublime-theme` from OS appearance. The light
variant inherits the full rule set and overrides variables, title-bar style and content darkness.

Spacing follows a 4-DIP scale: 4 for tight gaps, 8 for standard control insets, 12 for tab side
insets, 16 for sidebar indentation. Tabs are 32 DIPs high, button minimum height is 26, and status
controls are at least 24. A 1-DIP group divider makes pane boundaries visible without heavy frames.
Popup containers have 6-DIP radii; small controls and kind badges use 3. UI text uses the platform
system family at 12 DIPs, secondary text/headings at 11; headings are bold uppercase. Relative
sizes remain available to user overrides. Preview/transient tabs and open files use italic text.

Hover raises a surface using a short smoothstep opacity transition. Selection uses a blue-tinted
fill, press uses the stronger border tone, focus adds a quiet accent wash, disabled text dims, and
dirty tabs can use amber when the setting requests highlighting. Child labels read their row's
selected state. Semantic kind colors and VCS statuses reuse the original palette's rose, violet,
blue, amber, green and teal. Selected text is tested for at least 4.5:1 contrast in both appearances.
Overlay scrollbars and persistent minimap viewports follow settings; inactive sheets may dim.

Every entry in the engine's 130-entry registry has a baseline rule. Tints/fills draw the surfaces,
selection, focus, progress bars and scroll pucks. Twenty-one small geometric glyph textures are authored
in `generate.py`, including search toggles, folders/files, disclosure/folding/scroll arrows, close,
new-tab, sidebar and branch icons. The script uses only Python's standard library and produces
63 compact, supersampled PNGs (1x/2x/3x). File type icons use the matching `.sublime-file-icons`
mapping rather than overriding the dynamic `icon_file_type` texture in the main theme.

Run `python3 'crates/velocity/resources/Packages/Theme - Velocity/generate.py'` from the repository
root to reproduce the JSON and PNG resources. Existing outputs are backed up to
`/tmp/velocity-round8-backups` (or the platform temporary directory) and replaced atomically.
Neither application-only resources nor third-party artwork/themes are inputs. Public third-party
themes are used only by runtime compatibility tests in `vel-theme`; no textures were copied.
