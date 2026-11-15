//! Documented classes, specializations and the vel-ui compatibility seam.

/// Supported element and its element-specific state vocabulary. `hover` applies to all classes.
#[derive(Debug, Clone, Copy)]
pub struct ElementInfo {
    /// Space-separated class conjunction, as used in theme rules.
    pub class: &'static str,
    /// Element-specific attributes; matching also accepts application-defined attributes.
    pub attributes: &'static [&'static str],
}
const LUMINOSITY: &[&str] = &["file_light", "file_medium", "file_medium_dark", "file_dark"];
const VCS: &[&str] = &[
    "ignored",
    "untracked",
    "modified",
    "missing",
    "added",
    "staged",
    "deleted",
    "unmerged",
];
macro_rules! elements {
    ($($class:literal => $attrs:expr),* $(,)?) => {
        /// Complete public theme element registry, including documented specializations.
        pub const ELEMENTS: &[ElementInfo] = &[$(ElementInfo { class: $class, attributes: $attrs }),*];
    }
}
elements! {
    "window" => LUMINOSITY, "edit_window" => &[], "switch_project_window" => &[],
    "title_bar" => LUMINOSITY, "sidebar_container" => &[], "sidebar_tree" => &[],
    "tree_row" => &["selectable", "selected", "expandable", "expanded"],
    "sidebar_heading" => &[], "file_system_entry" => VCS, "sidebar_label" => &[],
    "close_button" => &[], "disclosure_button_control" => &[], "icon_folder" => &[],
    "icon_folder_loading" => &[], "icon_folder_dup" => &[], "icon_file_type" => &[],
    "vcs_status_badge" => VCS,
    "tabset_control" => LUMINOSITY,
    "tab_control" => &["file_light", "file_medium", "file_medium_dark", "file_dark", "dirty", "added", "modified", "deleted", "selected", "transient", "highlighted", "focused", "left", "right", "multiple", "left_of_selected", "left_of_hover", "right_of_selected", "right_of_hover", "left_overhang", "right_overhang"],
    "tab_label" => &["dirty", "added", "modified", "deleted", "transient"],
    "tab_close_button" => &[], "scroll_tabs_left_button" => &[], "scroll_tabs_right_button" => &[],
    "show_tabs_dropdown_button" => &[], "new_tab_button" => &[],
    "tab_connector" => &["left_overhang", "right_overhang", "selected"],
    "overlay_control" => &[], "overlay_control goto_file" => &[],
    "overlay_control goto_symbol" => &[], "overlay_control goto_symbol_in_project" => &[],
    "overlay_control goto_line" => &[], "overlay_control goto_word" => &[], "overlay_control command_palette" => &[],
    "quick_panel" => &[], "mini_quick_panel_row" => &["selected"],
    "quick_panel_row" => &["selected"], "quick_panel_entry" => &[], "kind_container" => &[],
    "kind_container kind_ambiguous" => &[], "kind_container kind_keyword" => &[],
    "kind_container kind_type" => &[], "kind_container kind_function" => &[],
    "kind_container kind_namespace" => &[], "kind_container kind_navigation" => &[],
    "kind_container kind_markup" => &[], "kind_container kind_variable" => &[],
    "kind_container kind_snippet" => &[],
    "kind_container kind_color_redish" => &[], "kind_container kind_color_orangish" => &[],
    "kind_container kind_color_yellowish" => &[], "kind_container kind_color_greenish" => &[],
    "kind_container kind_color_cyanish" => &[], "kind_container kind_color_bluish" => &[],
    "kind_container kind_color_purplish" => &[], "kind_container kind_color_pinkish" => &[],
    "kind_container kind_color_dark" => &[], "kind_container kind_color_light" => &[],
    "kind_label" => &[], "symbol_container" => &[], "quick_panel_label" => &[],
    "quick_panel_path_label" => &[], "quick_panel_label key_binding" => &[],
    "quick_panel_label hint" => &[], "quick_panel_detail_label" => &[],
    "sheet_contents" => &["file_light", "file_medium", "file_medium_dark", "file_dark", "highlighted"],
    "text_area_control" => LUMINOSITY, "grid_layout_control" => &[], "minimap_control" => &[],
    "fold_button_control" => &["expanded"], "popup_shadow" => &[], "popup_control" => &[],
    "popup_control html_popup" => &[], "popup_control annotation_popup" => &[],
    "annotation_close_button" => &[], "popup_control auto_complete_popup" => &[],
    "auto_complete" => &[], "table_row" => &["selected"], "trigger_container" => &[],
    "auto_complete_label" => &[], "auto_complete_label auto_complete_hint" => &[],
    "auto_complete_detail_pane" => &[], "auto_complete_info" => &[],
    "auto_complete_kind_name_label" => &[], "auto_complete_details" => &[],
    "panel_control" => &[], "panel_control find_panel" => &[], "panel_control replace_panel" => &[],
    "panel_control find_in_files_panel" => &[], "panel_control input_panel" => &[],
    "panel_control console_panel" => &[], "panel_control output_panel" => &[],
    "panel_control output_panel io_panel" => &[], "panel_control switch_project_panel" => &[],
    "panel_grid_control" => &[], "panel_close_button" => &[], "status_bar" => &["panel_visible"],
    "panel_button_control" => &[], "sidebar_button_control" => &[], "status_container" => &[],
    "status_button" => &[], "vcs_status" => &[], "vcs_branch_icon" => &[], "vcs_changes_annotation" => &[],
    "dialog" => &[], "progress_bar_control" => &[], "progress_gauge_control" => &[],
    "scroll_area_control" => &["scrollable", "hscrollable"], "scroll_bar_control" => &["dark", "horizontal"],
    "scroll_track_control" => &["dark", "horizontal"], "scroll_corner_control" => &["dark"],
    "puck_control" => &["dark", "horizontal"], "text_line_control" => &[], "dropdown_button_control" => &[],
    "button_control" => &["pressed", "disabled"], "icon_button_group" => &[],
    "icon_button_control" => &["selected", "left", "right"], "icon_regex" => &[], "icon_case" => &[],
    "icon_whole_word" => &[], "icon_wrap" => &[], "icon_in_selection" => &[], "icon_highlight" => &[],
    "icon_preserve_case" => &[], "icon_context" => &[], "icon_use_buffer" => &[], "icon_use_gitignore" => &[],
    "label_control" => &[], "title_label_control" => &[], "tool_tip_control" => &[], "tool_tip_label_control" => &[]
}

/// Lane A class names are accepted directly as well as these canonical theme classes.
pub const WIDGET_ALIASES: &[(&str, &str)] = &[
    ("text_input", "text_line_control"),
    ("scroll_area", "scroll_area_control"),
    ("kind_badge", "kind_container"),
    ("quick_panel_annotation", "quick_panel_label hint"),
    ("quick_panel_details", "quick_panel_detail_label"),
    ("panel", "panel_control"),
    ("button", "button_control"),
    ("open_file", "file_system_entry"),
    ("text_area", "text_area_control"),
    ("grid_layout", "grid_layout_control"),
];

/// Attribute based on HSV value, used by theme elements (different from minihtml's HSL class).
pub fn luminosity(color: vel_color_scheme::Color) -> &'static str {
    match color.r.max(color.g).max(color.b) {
        153..=255 => "file_light",
        77..=152 => "file_medium",
        26..=76 => "file_medium_dark",
        _ => "file_dark",
    }
}
