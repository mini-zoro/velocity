# Theme coverage inventory

Based on the public theme reference linked in README.md. This lists the compiled identifiers,
including compatible extensions; `color` is an alias of `fg`. No drawing code is in this crate.

## Typed non-layer properties

| Property | Type |
| --- | --- |
| `hit_test_level` | Number |
| `content_margin` | Insets |
| `fg` | Color |
| `bg` | Color |
| `font.face` | Text |
| `font.size` | FontSize |
| `font.bold` | Bool |
| `font.italic` | Bool |
| `shadow_color` | Color |
| `shadow_offset` | Point |
| `opacity` | Number |
| `tint_index` | Integer |
| `tint_modifier` | Modifier |
| `match_fg` | Color |
| `selected_fg` | Color |
| `selected_match_fg` | Color |
| `selected_bg` | Color |
| `dark_content` | Bool |
| `row_padding` | Insets |
| `border_color` | Color |
| `background_color` | Color |
| `style` | TitleStyle |
| `indent` | Integer |
| `indent_offset` | Integer |
| `indent_top_level` | Bool |
| `spacer_rows` | Bool |
| `case` | Case |
| `spacing` | Integer |
| `tab_overlap` | Integer |
| `tab_width` | Integer |
| `tab_min_width` | Integer |
| `tab_height` | Integer |
| `mouse_wheel_switch` | Bool |
| `max_margin_trim` | Integer |
| `accent_tint_index` | Integer |
| `accent_tint_modifier` | Modifier |
| `close_button_side` | Side |
| `link_color` | Color |
| `monospace_color` | Color |
| `monospace_background_color` | Color |
| `background_modifier` | Text |
| `border_size` | Number (fractional sizes draw hairlines) |
| `viewport_color` | Color |
| `viewport_opacity` | Number |
| `fg_blend` | Bool |
| `inside_spacing` | Integer |
| `outside_vspacing` | Integer |
| `outside_hspacing` | Integer |
| `min_size` | Point |
| `overlay` | Bool |
| `left_shadow` | Color |
| `left_shadow_size` | Integer |
| `top_shadow` | Color |
| `top_shadow_size` | Integer |
| `right_shadow` | Color |
| `right_shadow_size` | Integer |
| `bottom_shadow` | Color |
| `bottom_shadow_size` | Integer |
| `color_scheme_tint` | Color |
| `color_scheme_tint_2` | Color |
| `icon_color` | Color |
| `icon_size` | Point |
| `icon_margin` | Insets |
| `radius` | Number |

Velocity extensions. Sublime Text themes do not set these, so their elements render as before;
Velocity's bundled theme uses them for inset cards, soft shadows, focus rings and capsules.

| Property | Type | Effect |
| --- | --- | --- |
| `surface_inset` | Insets | Paints layers, border and shadow inside the element's bounds by these margins; the hit target is unchanged. |
| `shadow_blur` | Number | Blur of `shadow_color` (default 4). |
| `shadow_spread` | Number | How far the shadow extends past the surface before blurring (default 4); its corners follow `radius` plus the spread. With `shadow_blur: 0` this draws a focus ring outside an opaque control. |
| `pill` | Boolean | Corner radius is half the shorter side: capsules and circles at any size. |
| `viewport_radius` | Number | Corner radius of the minimap viewport box (`minimap_control`). |

Layer indices 0–3 each expose `tint` (animated color), `opacity` (animated float), `texture`
(PNG or frame sequence), `inner_margin` (Insets), `draw_center` / `repeat` (boolean), and the
compatible `tint_modifier` extension. Explicit null resets a previously set property.

## Elements and specializations

All receive hover matching; specific attributes are in `ELEMENTS` in `src/registry.rs`.
Unknown application classes and attributes remain matchable.

- `window`
- `edit_window`
- `switch_project_window`
- `title_bar`
- `sidebar_container`
- `sidebar_tree`
- `tree_row`
- `sidebar_heading`
- `file_system_entry`
- `sidebar_label`
- `close_button`
- `disclosure_button_control`
- `icon_folder`
- `icon_folder_loading`
- `icon_folder_dup`
- `icon_file_type`
- `vcs_status_badge`
- `tabset_control`
- `tab_control`
- `tab_label`
- `tab_close_button`
- `scroll_tabs_left_button`
- `scroll_tabs_right_button`
- `show_tabs_dropdown_button`
- `new_tab_button`
- `tab_connector`
- `overlay_control`
- `overlay_control goto_file`
- `overlay_control goto_symbol`
- `overlay_control goto_symbol_in_project`
- `overlay_control goto_line`
- `overlay_control goto_word`
- `overlay_control command_palette`
- `quick_panel`
- `mini_quick_panel_row`
- `quick_panel_row`
- `quick_panel_entry`
- `kind_container`
- `kind_container kind_ambiguous`
- `kind_container kind_keyword`
- `kind_container kind_type`
- `kind_container kind_function`
- `kind_container kind_namespace`
- `kind_container kind_navigation`
- `kind_container kind_markup`
- `kind_container kind_variable`
- `kind_container kind_snippet`
- `kind_container kind_color_redish`
- `kind_container kind_color_orangish`
- `kind_container kind_color_yellowish`
- `kind_container kind_color_greenish`
- `kind_container kind_color_cyanish`
- `kind_container kind_color_bluish`
- `kind_container kind_color_purplish`
- `kind_container kind_color_pinkish`
- `kind_container kind_color_dark`
- `kind_container kind_color_light`
- `kind_label`
- `symbol_container`
- `quick_panel_label`
- `quick_panel_path_label`
- `quick_panel_label key_binding`
- `quick_panel_label hint`
- `quick_panel_detail_label`
- `sheet_contents`
- `text_area_control`
- `grid_layout_control`
- `minimap_control`
- `fold_button_control`
- `popup_shadow`
- `popup_control`
- `popup_control html_popup`
- `popup_control annotation_popup`
- `annotation_close_button`
- `popup_control auto_complete_popup`
- `auto_complete`
- `table_row`
- `trigger_container`
- `auto_complete_label`
- `auto_complete_label auto_complete_hint`
- `auto_complete_detail_pane`
- `auto_complete_info`
- `auto_complete_kind_name_label`
- `auto_complete_details`
- `panel_control`
- `panel_control find_panel`
- `panel_control replace_panel`
- `panel_control find_in_files_panel`
- `panel_control input_panel`
- `panel_control console_panel`
- `panel_control output_panel`
- `panel_control output_panel io_panel`
- `panel_control switch_project_panel`
- `panel_grid_control`
- `panel_close_button`
- `status_bar`
- `panel_button_control`
- `sidebar_button_control`
- `status_container`
- `status_button`
- `vcs_status`
- `vcs_branch_icon`
- `vcs_changes_annotation`
- `dialog`
- `progress_bar_control`
- `progress_gauge_control`
- `scroll_area_control`
- `scroll_bar_control`
- `scroll_track_control`
- `scroll_corner_control`
- `puck_control`
- `text_line_control`
- `dropdown_button_control`
- `button_control`
- `icon_button_group`
- `icon_button_control`
- `icon_regex`
- `icon_case`
- `icon_whole_word`
- `icon_wrap`
- `icon_in_selection`
- `icon_highlight`
- `icon_preserve_case`
- `icon_context`
- `icon_use_buffer`
- `icon_use_gitignore`
- `label_control`
- `title_label_control`
- `tool_tip_control`
- `tool_tip_label_control`

## Lane A seam

Every class/attribute from vel-ui/README.md is accepted, including tab_connector selected and
tab_control focused/selected/transient/dirty. WIDGET_ALIASES is the authoritative alias table.
The default theme covers all registry entries and has additional direct-widget state rules.
