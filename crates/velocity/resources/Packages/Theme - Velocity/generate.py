#!/usr/bin/env python3
"""Generate original Velocity theme resources using only Python's standard library.

No application artwork, screenshots or third-party textures are inputs. Icons are
supersampled geometric strokes authored here. Re-running is deterministic.
"""
import json
import math
import os
from pathlib import Path
import re
import shutil
import struct
import tempfile
import zlib

ROOT = Path(__file__).resolve().parent


def write(path, data):
    if path.exists():
        backup = Path(tempfile.gettempdir()) / "velocity-round8-backups" / path.name
        backup.parent.mkdir(exist_ok=True)
        shutil.copy2(path, backup)
    temp = path.with_suffix(path.suffix + ".tmp")
    temp.write_bytes(data)
    os.replace(temp, path)


def png(name, shape, width=16, height=16):
    """Writes 1x/2x/3x white masks. `shape(x, y, scale)` returns coverage 0..1 at a DIP point;
    `scale` lets axis-aligned strokes snap to that density's pixel grid."""
    for scale in (1, 2, 3):
        w, h = width * scale, height * scale
        rows = bytearray()
        for y in range(h):
            rows.append(0)
            for x in range(w):
                count = sum(float(shape((x + (sx + .5) / 4) / scale, (y + (sy + .5) / 4) / scale, scale))
                            for sy in range(4) for sx in range(4))
                rows.extend((255, 255, 255, round(min(count, 16) * 255 / 16)))

        def chunk(kind, data):
            return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

        data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
        data += chunk(b"IDAT", zlib.compress(bytes(rows), 9)) + chunk(b"IEND", b"")
        suffix = "" if scale == 1 else f"@{scale}x"
        write(ROOT / f"{name}{suffix}.png", data)


STROKE = 1.1  # DIP; one physical pixel per DIP at 1x, two at 2x.


def snap(v, scale, width=STROKE):
    """Centers an axis-aligned stroke of `width` DIPs on the device pixel grid."""
    edge = round((v - width / 2) * scale) / scale
    return edge + width / 2


def segment(x, y, ax, ay, bx, by, width=STROKE):
    dx, dy = bx - ax, by - ay
    t = max(0, min(1, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy)))
    return math.hypot(x - ax - t * dx, y - ay - t * dy) <= width / 2


def lines(segments, width=STROKE, snapped=False):
    def shape(x, y, scale):
        for ax, ay, bx, by in segments:
            if snapped:
                if ax == bx:
                    ax = bx = snap(ax, scale, width)
                if ay == by:
                    ay = by = snap(ay, scale, width)
            if segment(x, y, ax, ay, bx, by, width):
                return 1
        return 0
    return shape


def rounded_box(x, y, left, top, right, bottom, radius):
    cx = min(max(x, left + radius), right - radius)
    cy = min(max(y, top + radius), bottom - radius)
    return left <= x <= right and top <= y <= bottom and math.hypot(x - cx, y - cy) <= radius


# Chrome glyphs on a 16-DIP grid, one consistent stroke weight.
png("close", lines([(5, 5, 11, 11), (5, 11, 11, 5)]))
png("chevron", lines([(6.5, 4.5, 10, 8), (10, 8, 6.5, 11.5)]))
png("chevron-down", lines([(4.5, 6.5, 8, 10), (8, 10, 11.5, 6.5)]))
png("chevron-left", lines([(9.5, 4.5, 6, 8), (6, 8, 9.5, 11.5)]))
png("plus", lines([(3.5, 8, 12.5, 8), (8, 3.5, 8, 12.5)], snapped=True))
png("dot", lambda x, y, s: math.hypot(x - 8, y - 8) <= 3.25)


def file_icon(x, y, scale):
    # A page with a folded corner and two faint text lines.
    page = lines([(3.5, 1.5, 9, 1.5), (3.5, 1.5, 3.5, 14.5), (3.5, 14.5, 12.5, 14.5),
                  (12.5, 5, 12.5, 14.5), (9, 1.5, 12.5, 5), (9, 1.5, 9, 5), (9, 5, 12.5, 5)],
                 snapped=True)(x, y, scale)
    text = lines([(5.8, 8.5, 10.2, 8.5), (5.8, 11, 10.2, 11)], snapped=True)(x, y, scale)
    return max(page, .55 * text)


png("file", file_icon)


def folder_icon(x, y, scale):
    body = rounded_box(x, y, 1.5, 4.5, 14.5, 13.5, 1.2)
    tab = rounded_box(x, y, 1.5, 2.5, 6.8, 6, .9) or (6 <= x <= 8.2 and 3.4 <= y <= 6 and y >= 3.4 + (x - 6) * -0)
    return 1 if body or tab else 0


png("folder", folder_icon)


def branch_icon(x, y, scale):
    ring = lambda cx, cy: abs(math.hypot(x - cx, y - cy) - 1.6) <= STROKE / 2
    return 1 if (ring(5, 3.5) or ring(5, 12.5) or ring(11, 5)
                 or lines([(5, 5.1, 5, 10.9), (11, 6.6, 11, 7.6), (11, 7.6, 6.2, 11.2)])(x, y, scale)) else 0


png("branch", branch_icon)
# Original search-option glyphs, constructed from line segments in a 16-DIP grid.
segments = {
    "regex": [(4, 11, 4, 12), (10, 3, 10, 10), (7, 5, 13, 8), (7, 8, 13, 5)],
    "case": [(3, 12, 6, 4), (6, 4, 9, 12), (4, 9, 8, 9), (11, 7, 14, 7), (14, 7, 14, 12), (11, 10, 14, 10), (11, 10, 11, 12)],
    "whole-word": [(2, 11, 2, 14), (2, 14, 14, 14), (14, 14, 14, 11), (4, 4, 6, 10), (6, 10, 8, 6), (8, 6, 10, 10), (10, 10, 12, 4)],
    "wrap": [(3, 4, 12, 4), (12, 4, 14, 7), (14, 7, 11, 10), (11, 10, 4, 10), (4, 10, 7, 7), (4, 10, 7, 13)],
    "selection": [(5, 3, 3, 3), (3, 3, 3, 13), (3, 13, 5, 13), (11, 3, 13, 3), (13, 3, 13, 13), (13, 13, 11, 13), (7, 6, 9, 6), (7, 10, 9, 10)],
    "highlight": [(3, 13, 13, 13), (5, 9, 10, 3), (10, 3, 13, 6), (13, 6, 8, 11), (8, 11, 5, 9)],
    "preserve-case": [(3, 12, 6, 4), (6, 4, 9, 12), (4, 9, 8, 9), (11, 4, 14, 4), (14, 4, 14, 11), (14, 11, 11, 11), (11, 11, 12, 9)],
    "context": [(3, 4, 13, 4), (3, 8, 10, 8), (3, 12, 13, 12)],
    "buffer": [(3, 2, 13, 2), (13, 2, 13, 14), (13, 14, 3, 14), (3, 14, 3, 2), (6, 6, 10, 6), (6, 10, 10, 10)],
    "gitignore": [(3, 8, 13, 8), (8, 3, 8, 5), (8, 11, 8, 13)],
    "sidebar": [(2, 3, 14, 3), (14, 3, 14, 13), (14, 13, 2, 13), (2, 13, 2, 3), (6, 3, 6, 13)],
    "sidebar-right": [(2, 3, 14, 3), (14, 3, 14, 13), (14, 13, 2, 13), (2, 13, 2, 3), (10, 3, 10, 13)],
}
for name, parts in segments.items():
    png(name, lines(parts, snapped=True))
# Structural textures, nine-sliced by the rules below: one-DIP hairlines along an edge,
# a two-DIP accent bar for the focused tab and a short inset divider between tabs.
png("edge-top", lambda x, y, s: y < 1)
png("edge-bottom", lambda x, y, s: y >= 15)
png("edge-right", lambda x, y, s: x >= 15)
png("tab-indicator", lambda x, y, s: y < 2)
png("tab-divider", lambda x, y, s: x >= 15 and 7 <= y < 9)


registry = ROOT.parents[3] / "vel-theme/src/registry.rs"
classes = re.findall(r'"([\w ]+)"\s*=>', registry.read_text())


def hex_mix(color, other, amount):
    a = [int(color[i:i + 2], 16) for i in (1, 3, 5)]
    b = [int(other[i:i + 2], 16) for i in (1, 3, 5)]
    return "#" + "".join(f"{round(x + (y - x) * amount):02x}" for x, y in zip(a, b))


# Chrome is derived from the bundled scheme backgrounds: darker than the editor in dark mode,
# slightly darker in light mode; floating panels are lighter. They are plain variables so user
# themes can still override `layer0.tint` or the variables themselves. Only the selected tab
# uses the live scheme background (tint_index 0) so it always matches the sheet below it.
SCHEME = {"dark": "#14232e", "light": "#f7f5ed"}
dark = dict(surface=hex_mix(SCHEME["dark"], "#000000", .25),
            overlay=hex_mix(SCHEME["dark"], "#ffffff", .05),
            inset=hex_mix(SCHEME["dark"], "#000000", .42), fg="#e3ebee", muted="#94a6ae",
            accent="#83c5e8", line=hex_mix(SCHEME["dark"], "#000000", .55), border="#36505f",
            shadow="#0000008c", green="#a6ce8d", amber="#efc17b", red="#ff9b93", violet="#c9acf1",
            teal="#7dd2c2", rose="#e9a8c0")
dark["selected"] = hex_mix(dark["surface"], dark["accent"], .24)
light = dict(surface=hex_mix(SCHEME["light"], "#000000", .06),
             overlay=hex_mix(SCHEME["light"], "#ffffff", .6),
             inset="#ffffff", fg="#243843", muted="#51626b", accent="#1d6290",
             line=hex_mix(SCHEME["light"], "#000000", .17), border="#b9bdb4", shadow="#0000002e",
             green="#3a6b22", amber="#7d540d", red="#a8332f", violet="#6a4597", teal="#1f6b60",
             rose="#8d3d61")
light["selected"] = hex_mix(light["surface"], light["accent"], .18)
rules = []


def rule(cls, **values):
    rules.append(dict({"class": cls}, **values))


def fill(color, opacity, layer=0, animated=False):
    value = {"target": opacity, "speed": 8, "interpolation": "smoothstep"} if animated else opacity
    return {f"layer{layer}.tint": f"var({color})", f"layer{layer}.opacity": value}


def texture(name, color, margin, layer=1, opacity=1.0):
    return {f"layer{layer}.texture": f"Theme - Velocity/{name}.png", f"layer{layer}.tint": f"var({color})",
            f"layer{layer}.opacity": opacity, f"layer{layer}.inner_margin": margin, f"layer{layer}.draw_center": False}


CHROME = {"layer0.tint": "var(surface)", "layer0.opacity": 1.0}
FLOATING = {"layer0.tint": "var(overlay)", "layer0.opacity": 1.0, "radius": 6,
            "border_color": "var(border)", "shadow_color": "var(shadow)", "shadow_offset": [0, 3]}

# Baselines state only what differs from the engine defaults (regular weight, no margin):
# a transparent layer 0, the UI font and the foreground. Fewer values keep start-up cheap.
for cls in classes:
    rule(cls, **{"layer0.tint": "var(surface)", "layer0.opacity": 0.0,
                 "fg": "var(fg)", "font.face": "system-ui", "font.size": 12})

# Chrome: one derived surface, separated from the editor by hairlines rather than frames.
for cls in ("title_bar", "sidebar_container", "tabset_control", "status_bar", "panel_control", "dialog"):
    rule(cls, content_margin=[8, 4], **CHROME)
rule("sidebar_container", **texture("edge-right", "line", [0, 0, 1, 0]))
rule("tabset_control", **texture("edge-bottom", "line", [0, 0, 0, 1]))
rule("status_bar", **texture("edge-top", "line", [0, 1, 0, 0]))
rule("panel_control", **texture("edge-top", "line", [0, 1, 0, 0]))
for cls in ("overlay_control", "popup_control", "tool_tip_control", "auto_complete_detail_pane"):
    rule(cls, content_margin=8, **FLOATING)
rule("title_bar", fg="var(muted)", bg="var(surface)", style="dark")
rule("sidebar_tree", indent=16, indent_offset=4, indent_top_level=False, spacer_rows=True)
rule("sidebar_heading", **{"fg": "var(muted)", "font.size": 11, "font.bold": True, "case": "upper"})
rule("tabset_control", tab_height=32, tab_width=180, tab_min_width=72, tab_overlap=0)
# Tabs: inactive tabs sit on the chrome with a short divider; the selected tab takes the
# editor background so it reads as part of the sheet below; the focused one gets an accent bar.
rule("tab_control", content_margin=[12, 8], close_button_side="right", max_margin_trim=6,
     **texture("tab-divider", "line", [0, 7, 1, 7]))
rule("tab_control", attributes=["hover"], **fill("fg", .05, animated=True))
rule("tab_control", attributes=["selected"], **{"layer0.opacity": 1.0, "tint_index": 0, "layer1.opacity": 0.0})
rule("tab_control", attributes=["selected", "focused"], **texture("tab-indicator", "accent", [0, 2, 0, 0], layer=2))
rule("tab_label", fg="var(muted)")
for parent in ("selected", "hover"):
    rule("tab_label", parents=[{"class": "tab_control", "attributes": [parent]}], fg="var(fg)")
rule("tab_control", attributes=["transient"], **{"font.italic": True})
rule("open_file", attributes=["transient"], **{"font.italic": True})
rule("open_file", attributes=["dirty"], fg="var(amber)")
rule("tab_control", attributes=["dirty"], settings=["highlight_modified_tabs"], fg="var(amber)")
rule("tab_label", attributes=["transient"], **{"font.italic": True})
rule("tab_label", parents=[{"class": "tab_control", "attributes": ["transient"]}], **{"font.italic": True})
# Rows and buttons use translucent fills so they read on any derived surface.
for cls in ("tree_row", "quick_panel_row", "mini_quick_panel_row", "table_row", "file_system_entry"):
    rule(cls, attributes=["hover"], **fill("fg", .06, animated=True))
    rule(cls, attributes=["selected"], **fill("selected", 1.0))
    rule(cls, attributes=["pressed"], **fill("accent", .32))
    rule(cls, attributes=["disabled"], opacity=.45)
# Base button fill first: later state rules must win over it.
rule("button_control", min_size=[56, 24], content_margin=[10, 4], radius=4, **fill("fg", .08))
# Toggles that are on are solid accent with an inverse glyph: unmistakable in both appearances.
for cls in ("button_control", "icon_button_control", "status_button"):
    rule(cls, attributes=["hover"], **fill("fg", .14, animated=True))
    rule(cls, attributes=["pressed"], **fill("fg", .22))
    rule(cls, attributes=["selected"], fg="var(surface)", **fill("accent", 1.0))
    rule(cls, attributes=["selected", "hover"], **fill("accent", .85))
    rule(cls, attributes=["selected", "pressed"], **fill("accent", .7))
    rule(cls, attributes=["disabled"], opacity=.45)
# The panel's default (Enter) action is tinted with the accent.
rule("button_control", attributes=["primary"], **fill("accent", .28))
rule("button_control", attributes=["primary", "hover"], **fill("accent", .4))
rule("button_control", attributes=["primary", "pressed"], **fill("accent", .52))
rule("tab_connector", attributes=["selected"], **fill("accent", 1.0))
# Secondary labels brighten on selected rows (tab labels: see above; other labels already use fg).
for cls in ("quick_panel_path_label", "quick_panel_detail_label"):
    for parent in ("quick_panel_row", "mini_quick_panel_row"):
        rule(cls, parents=[{"class": parent, "attributes": ["selected"]}], fg="var(fg)")
for cls in ("quick_panel", "auto_complete"):
    rule(cls, row_padding=[8, 4], dark_content=True)
for cls in ("quick_panel_label", "quick_panel_path_label", "auto_complete_label"):
    rule(cls, match_fg="var(accent)", selected_fg="var(fg)", selected_match_fg="var(accent)")
rule("quick_panel_label", **{"font.size": 13})
for cls in ("quick_panel_path_label", "quick_panel_label hint", "quick_panel_label key_binding", "quick_panel_detail_label", "auto_complete_details", "auto_complete_label auto_complete_hint"):
    rule(cls, **{"fg": "var(muted)", "font.size": 11, "link_color": "var(accent)", "monospace_color": "var(teal)", "monospace_background_color": "var(inset)"})
rule("quick_panel_entry", spacing=4)
rule("kind_container", content_margin=[4, 2], radius=3, **fill("muted", .16))
rule("kind_label", **{"font.bold": True, "font.size": 11})
for kind, color in dict(ambiguous="muted", keyword="rose", type="violet", function="red", namespace="accent", navigation="amber", markup="amber", variable="teal", snippet="green").items():
    rule(f"kind_container kind_{kind}", **fill(color, .16))
    rule("kind_label", parents=[{"class": f"kind_container kind_{kind}"}], fg=f"var({color})")
for kind, color in dict(redish="red", orangish="amber", yellowish="amber", greenish="green", cyanish="teal", bluish="accent", purplish="violet", pinkish="rose", dark="inset", light="fg").items():
    rule(f"kind_container kind_color_{kind}", **fill(color, .16))
    rule("kind_label", parents=[{"class": f"kind_container kind_color_{kind}"}], fg=f"var({color})")
# Inputs: inset well with a hairline; focus turns the hairline into the accent.
rule("text_line_control", **{"content_margin": [8, 6], "layer0.tint": "var(inset)", "layer0.opacity": 1.0, "color_scheme_tint": "var(inset)", "radius": 4, "border_color": "var(border)"})
rule("text_line_control", match_fg="var(accent)")
rule("text_line_control", attributes=["focused"], border_color="var(accent)")
rule("text_line_control", attributes=["invalid"], border_color="var(red)")
rule("panel_grid_control", inside_spacing=8, outside_vspacing=4, outside_hspacing=8)
rule("label_control", fg="var(muted)")
rule("label_control", attributes=["invalid"], fg="var(red)")
rule("status_button", content_margin=[8, 4], min_size=[24, 24], radius=4)
rule("status_container", fg="var(muted)")
rule("vcs_status", fg="var(muted)")
rule("grid_layout_control", border_size=1, border_color="var(line)")
rule("minimap_control", viewport_color="var(accent)", viewport_opacity=.12)
rule("minimap_control", attributes=["hover"], viewport_opacity=.2)
rule("minimap_control", settings=["always_show_minimap_viewport"], viewport_opacity=.2)
rule("scroll_bar_control", content_margin=[2, 2], **{"layer0.opacity": 0.0})
rule("puck_control", **{"layer0.tint": "var(muted)", "layer0.opacity": .4, "content_margin": [3, 3], "radius": 3})
rule("puck_control", attributes=["hover"], **{"layer0.opacity": {"target": .75, "speed": 8}})
rule("scroll_area_control", settings=["overlay_scroll_bars"], overlay=True)
rule("scroll_area_control", settings={"overlay_scroll_bars": ["enabled", "system"]}, overlay=True)
rule("scroll_area_control", settings={"overlay_scroll_bars": "disabled"}, overlay=False)
rule("sidebar_label", parents=[{"class": "tree_row", "attributes": ["expandable"]}], settings=["bold_folder_labels"], **{"font.bold": True})
rule("tab_label", attributes=["dirty"], settings=["highlight_modified_tabs"], fg="var(amber)")
rule("tabset_control", settings=["mouse_wheel_switches_tabs", "!enable_tab_scrolling"], platforms=["linux"], mouse_wheel_switch=True)
rule("tab_close_button", settings=["!show_tab_close_buttons"], **{"layer0.opacity": 0.0, "content_margin": 0})
rule("tab_control", settings=["show_tab_close_buttons_on_left"], close_button_side="left")
rule("sheet_contents", attributes=["!highlighted"], settings=["inactive_sheet_dimming"], background_modifier="blend(black 92%)")
for cls in ("scroll_tabs_left_button", "scroll_tabs_right_button"):
    rule(cls, settings=["hide_tab_scrolling_buttons"], **{"layer0.opacity": 0.0, "content_margin": 0})
rule("new_tab_button", settings=["hide_new_tab_button"], **{"layer0.opacity": 0.0, "content_margin": 0})
# Icon elements: a hover/pressed wash on layer 0 and the glyph on layer 1, drawn at its
# intrinsic 16-DIP size. content_margin sizes the element (and its hit target).
ICONS = dict(close_button="close", tab_close_button="close", panel_close_button="close", annotation_close_button="close",
             disclosure_button_control="chevron", icon_folder="folder", icon_folder_loading="folder", icon_folder_dup="folder",
             vcs_status_badge="dot", new_tab_button="plus", show_tabs_dropdown_button="chevron-down",
             scroll_tabs_left_button="chevron-left", scroll_tabs_right_button="chevron", fold_button_control="chevron",
             dropdown_button_control="chevron-down", vcs_branch_icon="branch", sidebar_button_control="sidebar",
             panel_button_control="sidebar", icon_regex="regex", icon_case="case", icon_whole_word="whole-word",
             icon_wrap="wrap", icon_in_selection="selection", icon_highlight="highlight", icon_preserve_case="preserve-case",
             icon_context="context", icon_use_buffer="buffer", icon_use_gitignore="gitignore")
BUTTONS = ("close_button", "tab_close_button", "panel_close_button", "annotation_close_button", "new_tab_button",
           "show_tabs_dropdown_button", "scroll_tabs_left_button", "scroll_tabs_right_button", "dropdown_button_control")
for cls, name in ICONS.items():
    # Clickable icon buttons also carry a (hidden) layer-0 wash for hover and press.
    wash = {"layer0.tint": "var(fg)", "layer0.opacity": 0.0, "radius": 4, "content_margin": [10, 10]} if cls in BUTTONS else {"content_margin": [8, 8]}
    rule(cls, **{"layer1.texture": f"Theme - Velocity/{name}.png", "layer1.tint": "var(muted)", "layer1.opacity": 1.0}, **wash)
for cls in BUTTONS:
    rule(cls, attributes=["hover"], **{"layer0.opacity": .14, "layer1.tint": "var(fg)"})
    rule(cls, attributes=["pressed"], **{"layer0.opacity": .22, "layer1.tint": "var(fg)"})
# Tab close buttons appear on the selected and hovered tab; a dirty tab shows a dot instead
# until it is hovered, as is conventional for editors.
rule("tab_close_button", **{"layer1.opacity": 0.0})
for parent in ("selected", "hover"):
    rule("tab_close_button", parents=[{"class": "tab_control", "attributes": [parent]}], **{"layer1.opacity": 1.0})
rule("tab_close_button", parents=[{"class": "tab_control", "attributes": ["dirty"]}], **{"layer1.texture": "Theme - Velocity/dot.png", "layer1.opacity": 1.0})
rule("tab_close_button", parents=[{"class": "tab_control", "attributes": ["dirty", "hover"]}], **{"layer1.texture": "Theme - Velocity/close.png"})
rule("tab_close_button", attributes=["hover"], **{"layer1.texture": "Theme - Velocity/close.png", "layer1.opacity": 1.0})
# Search toggles: the icon follows the toggle's state.
for icon in ("icon_regex", "icon_case", "icon_whole_word", "icon_wrap", "icon_in_selection", "icon_preserve_case"):
    rule(icon, parents=[{"class": "button_control", "attributes": ["hover"]}], **{"layer1.tint": "var(fg)"})
    rule(icon, parents=[{"class": "button_control", "attributes": ["selected"]}], **{"layer1.tint": "var(surface)"})
rule("disclosure_button_control", parents=[{"class": "tree_row", "attributes": ["expanded"]}], **{"layer1.texture": "Theme - Velocity/chevron-down.png"})
rule("fold_button_control", attributes=["expanded"], **{"layer1.texture": "Theme - Velocity/chevron-down.png"})
for cls in ("progress_bar_control", "progress_gauge_control"):
    rule(cls, **{"layer0.tint": "var(accent)" if cls == "progress_gauge_control" else "var(inset)", "layer0.opacity": 1.0, "content_margin": [0, 2], "radius": 2})
# Git: the badge glyph and its letter share the status colour.
for attribute, color in dict(added="green", untracked="green", modified="amber", staged="teal", deleted="red", missing="red", unmerged="red", ignored="muted").items():
    rule("vcs_status_badge", attributes=[attribute], fg=f"var({color})", **{"layer1.tint": f"var({color})"})
rule("vcs_status_badge", **{"font.size": 11, "font.bold": True})
rule("sidebar_button_control", settings=["sidebar_on_right"], **{"layer1.texture": "Theme - Velocity/sidebar-right.png"})
# Settings overrides follow the base icon rules, so hiding wins.
for cls, setting in (("tab_close_button", "!show_tab_close_buttons"), ("scroll_tabs_left_button", "hide_tab_scrolling_buttons"), ("scroll_tabs_right_button", "hide_tab_scrolling_buttons"), ("new_tab_button", "hide_new_tab_button")):
    rule(cls, settings=[setting], **{"layer0.opacity": 0.0, "layer1.opacity": 0.0, "content_margin": 0})

def compact(value):
    return json.dumps(value, separators=(", ", ": "))


# One rule per line keeps the file reviewable while keeping start-up parsing cheap.
body = "{\n\"variables\": " + compact(dark) + ",\n\"rules\": [\n" + ",\n".join(compact(r) for r in rules) + "\n]\n}\n"
write(ROOT / "Velocity.sublime-theme", ("// Original Velocity UI; generated by generate.py. MIT OR Apache-2.0.\n" + body).encode())
write(ROOT / "Velocity Light.sublime-theme", ("// Original Velocity Light UI; generated by generate.py.\n" + json.dumps({"extends": "Velocity.sublime-theme", "variables": light, "rules": [{"class": "title_bar", "style": "light"}, {"class": "quick_panel", "dark_content": False}, {"class": "auto_complete", "dark_content": False}]}, indent=2) + "\n").encode())
write(ROOT / "Velocity.sublime-file-icons", b'{"icons":{"*":"Theme - Velocity/file","rs":"Theme - Velocity/file","Makefile":"Theme - Velocity/file"}}\n')
print(f"Generated {len(classes)} element baselines, {len(rules)} rules, original PNGs with 1x/2x/3x densities.")
