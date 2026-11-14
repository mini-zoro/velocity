use std::{collections::BTreeMap, sync::Arc};
use vel_color_scheme::{Color, ColorScheme, FontStyle, Highlighter, SchemeSource, selected_scheme};
fn color(s: &str) -> Color {
    Color::parse(s, &BTreeMap::new()).unwrap()
}
#[test]
fn css_and_recursive_color_mod() {
    for (s, expected) in [
        ("#123", [17, 34, 51, 255]),
        ("#1234", [17, 34, 51, 68]),
        ("#11223344", [17, 34, 51, 68]),
        ("rgba(100%, 0%, 0%, 0.5)", [255, 0, 0, 128]),
        ("hsl(120,100%,50%)", [0, 255, 0, 255]),
        ("hsla(240,100%,50%,0.5)", [0, 0, 255, 128]),
        ("hwb(0,0%,0%)", [255, 0, 0, 255]),
        ("rebeccapurple", [102, 51, 153, 255]),
        ("transparent", [0, 0, 0, 0]),
    ] {
        assert_eq!(color(s).rgba8(), expected, "{s}");
    }
    let vars = BTreeMap::from([
        ("base".into(), "var(other)".into()),
        ("other".into(), "#804020".into()),
    ]);
    assert_eq!(
        Color::parse("color(var(base) a(50%) alpha(* 50%))", &vars)
            .unwrap()
            .a,
        64
    );
    assert_eq!(color("color(red s(0%) l(25%))"), Color::rgb(64, 64, 64));
    assert_eq!(color("color(red blend(blue 50%))"), Color::rgb(128, 0, 128));
    assert_eq!(
        color("color(red blend(blue 50% hsl))"),
        Color::rgb(0, 255, 0)
    );
    assert_eq!(color("color(#ff000080 blenda(#0000ff00 50%))").a, 64);
    assert!(color("color(#888 min-contrast(white 7))").contrast(Color::rgb(255, 255, 255)) >= 7.);
    assert!(Color::parse("var(a)", &BTreeMap::from([("a".into(), "var(a)".into())])).is_err());
    assert!(Color::parse("color(red mystery(3))", &vars).is_err());
    assert!(Color::parse("rgb(NaN,0,0)", &vars).is_err());
}
#[test]
fn overrides_rebind_base_variables_and_merge_properties() {
    let mut base=SchemeSource::parse(r##"{// comment
        "variables":{"ink":"red"},"globals":{"foreground":"var(ink)","background":"white","brackets_options":"underline bold","shadow_width":5},
        "rules":[{"scope":"keyword","foreground":"var(ink)","font_style":"bold"},{"scope":"source.rust keyword.control","foreground":"blue"}],
    }"##).unwrap();
    base.merge(&SchemeSource::parse(r#"{"variables":{"ink":"green"},"rules":[{"scope":"keyword","background":"yellow","font_style":""},{"scope":"keyword.control","selection_foreground":"black"}]}"#).unwrap()).unwrap();
    let scheme = base.compile().unwrap();
    assert_eq!(scheme.globals.color("foreground"), Some(color("green")));
    assert_eq!(
        scheme.globals.value("shadow_width"),
        Some(&serde_json::json!(5))
    );
    let mut h = Highlighter::new(Arc::new(scheme));
    let stack = "source.rust keyword.control.rust".parse().unwrap();
    let style = h.style_for(&stack);
    assert_eq!(style.foreground, color("blue"));
    assert_eq!(style.background, color("yellow"));
    assert_eq!(style.font_style, FontStyle::default());
    assert_eq!(style.selection_foreground, Some(color("black")));
    let id = h.intern(&stack);
    assert_eq!(h.style_for_id(id), h.style_for(&stack));
}
#[test]
fn gradients_adjustments_and_tie_breaking() {
    let scheme=Arc::new(ColorScheme::parse(r##"{"globals":{"background":"black","foreground":"#888"},"rules":[{"scope":"variable","foreground":["red","blue"]},{"scope":"meta.mark","background":"white","foreground_adjust":"min-contrast(white 7)"},{"scope":"keyword","foreground":"red"},{"scope":"keyword","foreground":"green","font_style":"glow squiggly_underline"}]}"##).unwrap());
    let mut h = Highlighter::new(scheme.clone());
    let style = h.style_for(&"meta.mark variable.other".parse().unwrap());
    assert!(style.foreground.contrast(color("white")) >= 7.);
    let a = scheme.style_for_token(style, "alpha");
    assert_eq!(a, scheme.style_for_token(style, "alpha"));
    assert_ne!(
        a.foreground,
        scheme.style_for_token(style, "beta").foreground
    );
    assert!(a.foreground.contrast(color("white")) >= 7.);
    let keyword = h.style_for(&"keyword".parse().unwrap());
    assert_eq!(keyword.foreground, color("green"));
    assert!(keyword.font_style.contains(FontStyle::GLOW));
    assert!(keyword.font_style.contains(FontStyle::SQUIGGLY_UNDERLINE));
}
#[test]
fn legacy_plist_and_auto_pair() {
    let scheme=SchemeSource::from_tm_theme(r##"<?xml version="1.0"?><plist version="1.0"><dict><key>name</key><string>Fixture</string><key>settings</key><array><dict><key>settings</key><dict><key>background</key><string>#123</string><key>lineHighlight</key><string>#345</string></dict></dict><dict><key>scope</key><string>comment</string><key>settings</key><dict><key>foreground</key><string>gray</string><key>fontStyle</key><string>italic underline</string></dict></dict></array></dict></plist>"##).unwrap().compile().unwrap();
    assert_eq!(scheme.globals.color("line_highlight"), Some(color("#345")));
    let mut h = Highlighter::new(Arc::new(scheme));
    assert!(
        h.style_for(&"comment.line".parse().unwrap())
            .font_style
            .contains(FontStyle::ITALIC)
    );
    let settings =
        serde_json::json!({"color_scheme":"auto","dark_color_scheme":"D","light_color_scheme":"L"});
    assert_eq!(selected_scheme(&settings, true), "D");
    assert_eq!(selected_scheme(&settings, false), "L");
}
#[test]
fn original_palettes_meet_contrast_and_cover_scope_families() {
    for scheme in [ColorScheme::velocity_dark(), ColorScheme::velocity_light()] {
        let background = scheme.default_style().background;
        let mut h = Highlighter::new(Arc::new(scheme));
        for scope in [
            "comment.line.rust",
            "constant.numeric",
            "entity.name.function",
            "entity.name.class",
            "entity.other.attribute-name",
            "keyword.control",
            "storage.type",
            "string.quoted",
            "support.function",
            "variable.parameter",
            "markup.heading",
            "markup.raw",
            "invalid.illegal",
            "punctuation",
        ] {
            let stack = scope.parse().unwrap();
            let style = h.style_for(&stack);
            assert!(
                style.foreground.contrast(background)
                    >= if scope.starts_with("comment") {
                        3.
                    } else {
                        4.5
                    },
                "{scope}: {:?}",
                style.foreground
            );
        }
    }
}

#[test]
fn hidden_resources_merge_in_package_order_and_keep_cdata() {
    let scheme = vel_color_scheme::load_resources(
        [
            (
                "Packages/Fixture/Fixture.hidden-color-scheme",
                r#"{"variables":{"ink":"red"},"globals":{"foreground":"var(ink)"}}"#,
            ),
            (
                "Packages/User/Fixture.hidden-color-scheme",
                r#"{"variables":{"ink":"blue"}}"#,
            ),
        ],
        "Fixture.hidden-color-scheme",
    )
    .unwrap();
    assert!(scheme.hidden);
    assert_eq!(scheme.default_style().foreground, color("blue"));
    let source=SchemeSource::from_tm_theme(r#"<plist version="1.0"><dict><key>settings</key><array><dict><key>scope</key><string><![CDATA[source & keyword]]></string><key>settings</key><dict><key>fontStyle</key><string><![CDATA[italic]]></string></dict></dict></array></dict></plist>"#).unwrap();
    let mut highlighter = Highlighter::new(Arc::new(source.compile().unwrap()));
    assert!(
        highlighter
            .style_for(&"source.rust keyword".parse().unwrap())
            .font_style
            .contains(FontStyle::ITALIC)
    );
}
