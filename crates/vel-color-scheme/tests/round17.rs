use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use vel_color_scheme::{
    Color, ColorScheme, CornerStyle, DecorationOptions, Highlighter, SchemeSource,
};

#[test]
fn malformed_scheme_items_leave_neighboring_colors_and_rules_usable() {
    for bad in [
        json!({"variables": 3}),
        json!({"variables":{"bad":[], "noncolor":"body { color: red; }"}}),
        json!({"globals":{"caret":"not a color"}}),
        json!({"globals":{"caret":"var(missing)"}}),
        json!({"globals":{"selection_corner_style":"triangle"}}),
        json!({"globals":{"line_diff_width":-1}}),
        json!({"globals":{"shadow_width":[]}}),
        json!({"globals":{"font_style":"unknown"}}),
        json!({"rules":{}}),
        json!({"name": []}),
    ] {
        let mut source = SchemeSource::parse(&bad.to_string()).unwrap();
        source.merge(&SchemeSource::parse(r##"{"globals":{"background":"#123","foreground":"#eee"},"rules":[{"scope":"keyword","foreground":"red"}]}"##).unwrap()).unwrap();
        let scheme = source.compile().unwrap();
        assert!(!scheme.warnings().is_empty(), "{bad}");
        assert_eq!(scheme.default_style().background, Color::rgb(17, 34, 51));
        let mut highlighter = Highlighter::new(Arc::new(scheme));
        assert_eq!(
            highlighter
                .style_for(&"keyword".parse().unwrap())
                .foreground,
            Color::rgb(255, 0, 0)
        );
    }
    for bad_rule in [
        json!({"scope":"keyword","foreground":"bad color"}),
        json!({"scope":"keyword","font_style":"unknown"}),
        json!({"scope":"keyword","foreground_adjust":"alpha(0.5)"}),
        json!({"scope":"keyword","foreground":["bad color","red"]}),
        json!({"scope":9}),
        json!(null),
    ] {
        let value = json!({"rules":[bad_rule,{"scope":"string","foreground":"blue"}]});
        let scheme = ColorScheme::parse(&value.to_string()).unwrap();
        assert!(!scheme.warnings().is_empty(), "{value}");
        let mut highlighter = Highlighter::new(Arc::new(scheme));
        assert_eq!(
            highlighter.style_for(&"string".parse().unwrap()).foreground,
            Color::rgb(0, 0, 255)
        );
    }
    assert!(ColorScheme::parse("{").is_err());
    assert!(!ColorScheme::parse("[]").unwrap().warnings().is_empty());
}

#[test]
fn noncolor_variables_geometry_and_legacy_numeric_strings_compile() {
    let scheme=ColorScheme::parse(r##"{"variables":{"css":"p { color: red; }","ink":"#abc"},"globals":{"foreground":"var(ink)","popup_css":"var(css)","selection_corner_style":"cut","selection_corner_radius":"2","block_caret_corner_style":"round","block_caret_corner_radius":3.5,"line_diff_width":"2","brackets_options":"foreground bold italic","tags_options":"foreground"}}"##).unwrap();
    assert!(scheme.warnings().is_empty(), "{:?}", scheme.warnings());
    assert_eq!(scheme.selection_corner_style(), CornerStyle::Cut);
    assert_eq!(scheme.selection_corner_radius(), 2.);
    assert_eq!(scheme.block_caret_corner_style(), CornerStyle::Round);
    assert_eq!(scheme.block_caret_corner_radius(), 3.5);
    assert_eq!(scheme.globals.number("line_diff_width"), Some(2.));
    assert!(
        scheme
            .brackets_options()
            .unwrap()
            .contains(DecorationOptions::FOREGROUND)
    );
    assert!(
        scheme
            .brackets_options()
            .unwrap()
            .contains(DecorationOptions::BOLD)
    );
    assert!(scheme.bracket_contents_options().is_none());
    assert_eq!(scheme.tags_options(), Some(DecorationOptions::FOREGROUND));
    let plist = r##"<plist version="1.0"><dict><key>settings</key><array><dict><key>settings</key><dict><key>shadowWidth</key><string>6</string><key>foreground</key><string>#eee</string></dict></dict></array></dict></plist>"##;
    let legacy = vel_color_scheme::load_resources(
        [("Packages/User/T.hidden-tmTheme", plist)],
        "T.hidden-tmTheme",
    )
    .unwrap();
    assert!(legacy.hidden);
    assert_eq!(legacy.globals.number("shadow_width"), Some(6.));
    assert!(legacy.warnings().is_empty());
}

#[test]
fn minimal_scheme_derived_globals_table() {
    let scheme = ColorScheme::parse(
        r##"{"globals":{"background":"#000","foreground":"#fff","selection":"#456"}}"##,
    )
    .unwrap();
    for (key, expected) in [
        ("invisibles", [255, 255, 255, 89]),
        ("gutter", [0, 0, 0, 255]),
        ("gutter_foreground", [128, 128, 128, 255]),
        ("line_highlight", [255, 255, 255, 13]),
        ("rulers", [255, 255, 255, 38]),
        ("guide", [255, 255, 255, 38]),
        ("active_guide", [255, 255, 255, 102]),
        ("stack_guide", [255, 255, 255, 64]),
        ("find_highlight", [77, 77, 77, 255]),
        ("find_highlight_foreground", [255, 255, 255, 255]),
        ("misspelling", [255, 255, 255, 179]),
        ("line_diff_added", [153, 153, 153, 255]),
        ("line_diff_modified", [115, 115, 115, 255]),
        ("line_diff_deleted", [204, 204, 204, 255]),
        ("selection", [68, 85, 102, 255]),
        ("fold_marker", [255, 255, 255, 255]),
        ("block_caret_underline", [255, 255, 255, 255]),
    ] {
        assert_eq!(
            scheme.globals.color(key).unwrap().rgba8(),
            expected,
            "{key}"
        );
    }
    for key in [
        "selection_border",
        "inactive_selection_border",
        "minimap_border",
        "block_caret_border",
        "shadow",
    ] {
        assert_eq!(scheme.globals.color(key).unwrap().a, 0, "{key}");
    }
}

#[test]
fn blend_and_blenda_weight_the_base_in_both_directions() {
    let vars = BTreeMap::new();
    for (expr, expected) in [
        ("color(black blend(white 0%))", [255, 255, 255, 255]),
        ("color(black blend(white 25%))", [191, 191, 191, 255]),
        ("color(black blend(white 75%))", [64, 64, 64, 255]),
        ("color(black blend(white 100%))", [0, 0, 0, 255]),
        ("color(white blend(black 25%))", [64, 64, 64, 255]),
        ("color(white blend(black 75%))", [191, 191, 191, 255]),
        ("color(#00000040 blend(#ffffffc0 25%))", [191, 191, 191, 64]),
        (
            "color(#00000040 blenda(#ffffffc0 25%))",
            [191, 191, 191, 160],
        ),
        ("color(#ffffffc0 blenda(#00000040 25%))", [64, 64, 64, 96]),
        ("color(red blend(yellow 25% hsl))", [255, 191, 0, 255]),
        ("color(yellow blend(red 25% hsl))", [255, 64, 0, 255]),
    ] {
        assert_eq!(
            Color::parse(expr, &vars).unwrap().rgba8(),
            expected,
            "{expr}"
        );
    }
}

#[test]
fn absent_drawing_colors_preserve_ink_and_only_borders_become_optional() {
    let scheme =
        ColorScheme::parse(r##"{"globals":{"background":"#fff","foreground":"#123456"}}"##)
            .unwrap();
    // Before derivation every absent color requested by drawing fell back to foreground.
    // Borders intentionally become opt-in; glyphs, fills and diagnostic lines retain ink.
    for (key, base_drawn, now_drawn) in [
        ("fold_marker", true, true),
        ("caret", true, true),
        ("block_caret", true, true),
        ("block_caret_underline", true, true),
        ("invisibles", true, true),
        ("gutter", true, true),
        ("gutter_foreground", true, true),
        ("line_highlight", true, true),
        ("rulers", true, true),
        ("guide", true, true),
        ("active_guide", true, true),
        ("stack_guide", true, true),
        ("find_highlight", true, true),
        ("find_highlight_foreground", true, true),
        ("misspelling", true, true),
        ("line_diff_added", true, true),
        ("line_diff_modified", true, true),
        ("line_diff_deleted", true, true),
        ("selection", true, true),
        ("inactive_selection", true, true),
        ("selection_border", true, false),
        ("inactive_selection_border", true, false),
        ("minimap_border", true, false),
        ("block_caret_border", true, false),
        ("shadow", true, false),
    ] {
        assert_eq!(
            scheme.default_style().foreground.a != 0,
            base_drawn,
            "base {key}"
        );
        assert_eq!(
            scheme.globals.color(key).unwrap().a != 0,
            now_drawn,
            "now {key}"
        );
    }
}
