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


def png(name, shape):
    for scale in (1, 2, 3):
        size = 16 * scale
        rows = bytearray()
        for y in range(size):
            rows.append(0)
            for x in range(size):
                count = sum(shape((x + (sx + .5) / 4) / scale,
                                  (y + (sy + .5) / 4) / scale)
                            for sy in range(4) for sx in range(4))
                rows.extend((255, 255, 255, round(count * 255 / 16)))

        def chunk(kind, data):
            return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

        data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
        data += chunk(b"IDAT", zlib.compress(bytes(rows), 9)) + chunk(b"IEND", b"")
        suffix = "" if scale == 1 else f"@{scale}x"
        write(ROOT / f"{name}{suffix}.png", data)


def stroke(x, y, ax, ay, bx, by, radius=.7):
    dx, dy = bx - ax, by - ay
    t = max(0, min(1, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy)))
    return math.hypot(x - ax - t * dx, y - ay - t * dy) <= radius


png("close", lambda x, y: stroke(x, y, 5, 5, 11, 11) or stroke(x, y, 5, 11, 11, 5))
png("chevron", lambda x, y: stroke(x, y, 6, 4, 10, 8) or stroke(x, y, 10, 8, 6, 12))
png("chevron-down", lambda x, y: stroke(x, y, 4, 6, 8, 10) or stroke(x, y, 8, 10, 12, 6))
png("file", lambda x, y: (4 <= x <= 12 and 2 <= y <= 14) and not (5.2 < x < 10.8 and 3.2 < y < 12.8))
png("folder", lambda x, y: (2 <= x <= 14 and 5 <= y <= 13) or (2 <= x <= 7 and 3 <= y <= 5))
png("dot", lambda x, y: math.hypot(x - 8, y - 8) <= 2.5)
png("plus", lambda x, y: stroke(x, y, 4, 8, 12, 8) or stroke(x, y, 8, 4, 8, 12))
png("chevron-left", lambda x, y: stroke(x, y, 10, 4, 6, 8) or stroke(x, y, 6, 8, 10, 12))
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
    "branch": [(5, 3, 5, 13), (5, 9, 11, 5), (11, 5, 11, 3)],
    "sidebar": [(2, 3, 14, 3), (14, 3, 14, 13), (14, 13, 2, 13), (2, 13, 2, 3), (6, 3, 6, 13)],
    "sidebar-right": [(2, 3, 14, 3), (14, 3, 14, 13), (14, 13, 2, 13), (2, 13, 2, 3), (10, 3, 10, 13)],
}
for name, lines in segments.items():
    png(name, lambda x, y, lines=lines: any(stroke(x, y, *line) for line in lines))


registry = ROOT.parents[3] / "vel-theme/src/registry.rs"
classes = re.findall(r'"([\w ]+)"\s*=>', registry.read_text())
variables = dict(surface="#14232e", raised="#1d303d", inset="#101d27", fg="#e3ebee",
                 muted="#8b9da5", accent="#83c5e8", selected="#304e63", border="#486777",
                 green="#a6ce8d", amber="#efc17b", red="#ff9b93", violet="#c9acf1", teal="#7dd2c2", rose="#e9a8c0")
rules = []


def rule(cls, **values):
    rules.append(dict({"class": cls}, **values))


for cls in classes:
    rule(cls, **{"layer0.tint": "var(surface)", "layer0.opacity": 0.0,
                 "fg": "var(fg)", "font.face": "system-ui", "font.size": 12,
                 "font.bold": False, "font.italic": False, "content_margin": 0})

for cls in ("title_bar", "sidebar_container", "tabset_control", "status_bar", "panel_control", "dialog", "sheet_contents"):
    rule(cls, **{"layer0.opacity": 1.0, "layer0.tint": "var(surface)", "content_margin": [8, 4]})
for cls in ("overlay_control", "popup_control", "tool_tip_control", "auto_complete_detail_pane"):
    rule(cls, **{"layer0.opacity": 1.0, "layer0.tint": "var(raised)", "content_margin": 8, "radius": 6})
rule("title_bar", fg="var(muted)", bg="var(surface)", style="dark")
rule("sidebar_tree", indent=16, indent_offset=4, indent_top_level=False, spacer_rows=True)
rule("sidebar_heading", **{"fg": "var(muted)", "font.size": 11, "font.bold": True, "case": "upper"})
rule("tabset_control", tab_height=32, tab_width=180, tab_min_width=72, tab_overlap=0)
rule("tab_control", content_margin=[12, 8], close_button_side="right", max_margin_trim=6)
rule("tab_label", fg="var(muted)")
rule("tab_control", attributes=["transient"], **{"font.italic": True})
rule("open_file", attributes=["transient"], **{"font.italic": True})
rule("open_file", attributes=["dirty"], fg="var(amber)")
rule("tab_control", attributes=["dirty"], settings=["highlight_modified_tabs"], fg="var(amber)")
rule("tab_label", attributes=["transient"], **{"font.italic": True})
rule("tab_label", parents=[{"class": "tab_control", "attributes": ["transient"]}], **{"font.italic": True})
for cls in ("tab_control", "tree_row", "quick_panel_row", "mini_quick_panel_row", "table_row", "button_control", "icon_button_control", "status_button", "file_system_entry"):
    rule(cls, attributes=["hover"], **{"layer0.tint": "var(raised)", "layer0.opacity": {"target": 1.0, "speed": 8, "interpolation": "smoothstep"}})
    rule(cls, attributes=["selected"], **{"layer0.tint": "var(selected)", "layer0.opacity": 1.0})
    rule(cls, attributes=["pressed"], **{"layer0.tint": "var(border)", "layer0.opacity": 1.0})
    rule(cls, attributes=["disabled"], opacity=.45)
rule("tab_control", attributes=["focused"], **{"layer1.tint": "var(accent)", "layer1.opacity": .15})
rule("tab_connector", attributes=["selected"], **{"layer0.tint": "var(selected)", "layer0.opacity": 1.0})
for cls in ("tab_label", "sidebar_label", "quick_panel_label", "quick_panel_path_label", "quick_panel_detail_label"):
    for parent in ("tab_control", "tree_row", "quick_panel_row", "mini_quick_panel_row"):
        rule(cls, parents=[{"class": parent, "attributes": ["selected"]}], fg="var(fg)")
for cls in ("quick_panel", "auto_complete"):
    rule(cls, row_padding=[8, 4], dark_content=True)
for cls in ("quick_panel_label", "quick_panel_path_label", "auto_complete_label"):
    rule(cls, match_fg="var(accent)", selected_fg="var(fg)", selected_match_fg="var(accent)")
for cls in ("quick_panel_path_label", "quick_panel_label hint", "quick_panel_label key_binding", "quick_panel_detail_label", "auto_complete_details"):
    rule(cls, **{"fg": "var(muted)", "font.size": 11, "link_color": "var(accent)", "monospace_color": "var(teal)", "monospace_background_color": "var(inset)"})
rule("quick_panel_entry", spacing=4)
rule("kind_container", content_margin=[4, 2], **{"layer0.tint": "var(raised)", "layer0.opacity": 1.0, "radius": 3})
for kind, color in dict(ambiguous="muted", keyword="rose", type="violet", function="red", namespace="accent", navigation="amber", markup="amber", variable="teal", snippet="green").items():
    rule(f"kind_container kind_{kind}", fg=f"var({color})")
    rule("kind_label", parents=[{"class": f"kind_container kind_{kind}"}], fg=f"var({color})")
for kind, color in dict(redish="red", orangish="amber", yellowish="amber", greenish="green", cyanish="teal", bluish="accent", purplish="violet", pinkish="rose", dark="inset", light="fg").items():
    rule(f"kind_container kind_color_{kind}", fg=f"var({color})")
    rule("kind_label", parents=[{"class": f"kind_container kind_color_{kind}"}], fg=f"var({color})")
rule("text_line_control", **{"content_margin": [8, 6], "layer0.tint": "var(inset)", "layer0.opacity": 1.0, "color_scheme_tint": "var(inset)", "radius": 3})
rule("text_line_control", attributes=["focused"], **{"layer1.tint": "var(accent)", "layer1.opacity": .12})
rule("button_control", min_size=[56, 26], content_margin=[8, 4], **{"layer0.tint": "var(raised)", "layer0.opacity": 1.0, "radius": 3})
rule("panel_grid_control", inside_spacing=8, outside_vspacing=4, outside_hspacing=8)
rule("status_button", content_margin=[8, 4], min_size=[24, 24])
rule("grid_layout_control", border_size=1, border_color="var(border)")
rule("minimap_control", viewport_color="var(accent)", viewport_opacity=.08)
rule("minimap_control", attributes=["hover"], viewport_opacity=.18)
rule("minimap_control", settings=["always_show_minimap_viewport"], viewport_opacity=.18)
rule("scroll_bar_control", content_margin=[2, 2], **{"layer0.opacity": 0.0})
rule("puck_control", **{"layer0.tint": "var(muted)", "layer0.opacity": .35, "content_margin": [3, 3], "radius": 3})
rule("puck_control", attributes=["hover"], **{"layer0.opacity": {"target": .7, "speed": 8}})
rule("scroll_area_control", settings=["overlay_scroll_bars"], overlay=True)
rule("scroll_area_control", settings={"overlay_scroll_bars": ["enabled", "system"]}, overlay=True)
rule("scroll_area_control", settings={"overlay_scroll_bars": "disabled"}, overlay=False)
rule("sidebar_label", parents=[{"class": "tree_row", "attributes": ["expandable"]}], settings=["bold_folder_labels"], **{"font.bold": True})
rule("tab_label", attributes=["dirty"], settings=["highlight_modified_tabs"], fg="var(amber)")
rule("tabset_control", settings=["mouse_wheel_switches_tabs", "!enable_tab_scrolling"], platforms=["linux"], mouse_wheel_switch=True)
rule("tab_close_button", settings=["!show_tab_close_buttons"], **{"layer0.opacity": 0.0, "content_margin": 0})
rule("tab_control", settings=["show_tab_close_buttons_on_left"], close_button_side="left")
rule("sheet_contents", attributes=["!highlighted"], settings=["inactive_sheet_dimming"], background_modifier="blend(black 8%)")
for cls in ("scroll_tabs_left_button", "scroll_tabs_right_button"):
    rule(cls, settings=["hide_tab_scrolling_buttons"], **{"layer0.opacity": 0.0, "content_margin": 0})
rule("new_tab_button", settings=["hide_new_tab_button"], **{"layer0.opacity": 0.0, "content_margin": 0})
for cls, texture in dict(close_button="close", tab_close_button="close", panel_close_button="close", annotation_close_button="close", disclosure_button_control="chevron", icon_folder="folder", icon_folder_loading="folder", icon_folder_dup="folder", vcs_status_badge="dot", new_tab_button="plus", show_tabs_dropdown_button="chevron-down", scroll_tabs_left_button="chevron-left", scroll_tabs_right_button="chevron", fold_button_control="chevron", dropdown_button_control="chevron-down", vcs_branch_icon="branch", sidebar_button_control="sidebar", panel_button_control="sidebar", icon_regex="regex", icon_case="case", icon_whole_word="whole-word", icon_wrap="wrap", icon_in_selection="selection", icon_highlight="highlight", icon_preserve_case="preserve-case", icon_context="context", icon_use_buffer="buffer", icon_use_gitignore="gitignore").items():
    rule(cls, **{"layer0.texture": f"Theme - Velocity/{texture}.png", "layer0.tint": "var(muted)", "layer0.opacity": 1.0, "content_margin": [8, 8]})
rule("disclosure_button_control", parents=[{"class": "tree_row", "attributes": ["expanded"]}], **{"layer0.texture": "Theme - Velocity/chevron-down.png"})
rule("fold_button_control", attributes=["expanded"], **{"layer0.texture": "Theme - Velocity/chevron-down.png"})
for cls in ("progress_bar_control", "progress_gauge_control"):
    rule(cls, **{"layer0.tint": "var(accent)" if cls=="progress_gauge_control" else "var(inset)", "layer0.opacity": 1.0, "content_margin": [0, 2], "radius": 2})
for attribute, color in dict(added="green", untracked="green", modified="amber", staged="teal", deleted="red", missing="red", unmerged="red", ignored="muted").items():
    rule("vcs_status_badge", attributes=[attribute], **{"layer0.tint": f"var({color})"})
rule("sidebar_button_control", settings=["sidebar_on_right"], **{"layer0.texture": "Theme - Velocity/sidebar-right.png"})
# Settings overrides follow the base icon rules, so hiding wins.
for cls, setting in (("tab_close_button", "!show_tab_close_buttons"), ("scroll_tabs_left_button", "hide_tab_scrolling_buttons"), ("scroll_tabs_right_button", "hide_tab_scrolling_buttons"), ("new_tab_button", "hide_new_tab_button")):
    rule(cls, settings=[setting], **{"layer0.opacity": 0.0, "content_margin": 0})

write(ROOT / "Velocity.sublime-theme", ("// Original Velocity UI; generated by generate.py. MIT OR Apache-2.0.\n" + json.dumps(dict(variables=variables, rules=rules), indent=2) + "\n").encode())
light = dict(surface="#f5f7f8", raised="#e9eff2", inset="#ffffff", fg="#243843", muted="#627984", accent="#236c98", selected="#d6e7f1", border="#9fb5c0", green="#397127", amber="#875c13", red="#b13b37", violet="#71509d", teal="#27766b", rose="#9c476a")
write(ROOT / "Velocity Light.sublime-theme", ("// Original Velocity Light UI; generated by generate.py.\n" + json.dumps({"extends": "Velocity.sublime-theme", "variables": light, "rules": [{"class": "title_bar", "style": "light"}, {"class": "quick_panel", "dark_content": False}, {"class": "auto_complete", "dark_content": False}]}, indent=2) + "\n").encode())
write(ROOT / "Velocity.sublime-file-icons", b'{"icons":{"*":"Theme - Velocity/file","rs":"Theme - Velocity/file","Makefile":"Theme - Velocity/file"}}\n')
print(f"Generated {len(classes)} element baselines, {len(rules)} rules, original PNGs with 1x/2x/3x densities.")
