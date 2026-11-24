use proptest::prelude::*;
use serde_json::json;
use std::{path::Path, sync::Arc};
use vel_config::settings::Layer;
use vel_theme::*;

fn resolve(
    source: &str,
    chain: &[Element<'_>],
    settings: &Settings,
    platform: Platform,
) -> ResolvedStyle {
    Resolver::new(Arc::new(Theme::parse(source, &Palette::default()).unwrap()))
        .resolve(&ElementPath(chain), settings, platform)
        .clone()
}
#[test]
fn documentation_matching_corpus() {
    let source = r##"{ // later, less-specific rules still win by property
      "rules":[
        {"class":"label_control", "fg":"red", "font.bold":true},
        {"class":"label_control", "parents":[{"class":"window"},{"class":"button_control","attributes":["hover","!disabled"]}],"fg":"blue"},
        {"class":"label_control hint", "settings":["enabled","!off"], "platforms":["osx"],"font.italic":true},
        {"class":"label_control", "settings":{"mode":["round","square"],"missing":"","enabled":true},"bg":"yellow"},
        {"class":"label_control", "fg":"green"}
      ]
    }"##;
    let mut settings = Settings::default();
    settings.set(Layer::User, "enabled", json!(true));
    settings.set(Layer::User, "mode", json!("round"));
    let chain = [
        Element::new("window", &[]),
        Element::new("spacer", &[]),
        Element::new("button_control", &["hover"]),
        Element::new("label_control hint", &[]),
    ];
    let s = resolve(source, &chain, &settings, Platform::Mac);
    assert_eq!(s.color(&Property::Fg), Some(Color::rgb(0, 128, 0)));
    assert_eq!(s.color(&Property::Bg), Some(Color::rgb(255, 255, 0)));
    assert_eq!(s.boolean(&Property::FontBold), Some(true));
    assert_eq!(s.boolean(&Property::FontItalic), Some(true));
    assert_eq!(
        resolve(source, &chain, &settings, Platform::Linux).boolean(&Property::FontItalic),
        None
    );
    let chain = [Element::new("label_control", &[])];
    assert_eq!(
        resolve(source, &chain, &settings, Platform::Mac).boolean(&Property::FontItalic),
        None
    );
}
#[test]
fn ordered_distinct_parents_and_negation() {
    let source = r#"[{"class":"leaf","parents":[{"class":"outer"},{"class":"inner","attributes":["!disabled"]}],"font.bold":true}]"#;
    let settings = Settings::default();
    for (chain, expected) in [
        (
            vec![
                Element::new("outer", &[]),
                Element::new("gap", &[]),
                Element::new("inner", &[]),
                Element::new("leaf", &[]),
            ],
            Some(true),
        ),
        (
            vec![
                Element::new("inner", &[]),
                Element::new("outer", &[]),
                Element::new("leaf", &[]),
            ],
            None,
        ),
        (
            vec![Element::new("outer inner", &[]), Element::new("leaf", &[])],
            None,
        ),
        (
            vec![
                Element::new("outer", &[]),
                Element::new("inner", &["disabled"]),
                Element::new("leaf", &[]),
            ],
            None,
        ),
    ] {
        assert_eq!(
            resolve(source, &chain, &settings, Platform::Mac).boolean(&Property::FontBold),
            expected
        );
    }
}
#[test]
fn inheritance_user_rebinding_and_cycles() {
    let resources = [
        Resource {
            path: "Packages/Base/Base.sublime-theme",
            contents: r#"{"variables":{"ink":"red","pad":[2,4]},"rules":[{"class":"label_control","fg":"var(ink)","content_margin":"var(pad)"}]}"#,
        },
        Resource {
            path: "Packages/Child/Child.sublime-theme",
            contents: r#"{"extends":"Base.sublime-theme","variables":{"ink":"blue"},"rules":[{"class":"label_control","font.bold":true}]}"#,
        },
        Resource {
            path: "Packages/User/Child.sublime-theme",
            contents: r#"{"variables":{"ink":"green"},"rules":[{"class":"label_control","font.bold":false}]}"#,
        },
    ];
    let theme = load_resources(&resources, "Child.sublime-theme", &Palette::default()).unwrap();
    let mut r = Resolver::new(Arc::new(theme));
    let s = r.resolve(
        &ElementPath(&[Element::new("label_control", &[])]),
        &Settings::default(),
        Platform::Mac,
    );
    assert_eq!(s.color(&Property::Fg), Some(Color::rgb(0, 128, 0)));
    assert_eq!(s.boolean(&Property::FontBold), Some(false));
    assert_eq!(
        s.insets(&Property::ContentMargin),
        Some(Insets {
            left: 2.,
            right: 2.,
            top: 4.,
            bottom: 4.
        })
    );
    let cyclic = [
        Resource {
            path: "A",
            contents: r#"{"extends":"B"}"#,
        },
        Resource {
            path: "B",
            contents: r#"{"extends":"A"}"#,
        },
    ];
    assert!(load_resources(&cyclic, "A", &Palette::default()).is_err());
    for source in [
        r#"{"variables":{"a":"var(b)","b":"var(a)"}}"#,
        r#"{"rules":[{"class":"x","fg":"var(missing)"}]}"#,
    ] {
        assert!(Theme::parse(source, &Palette::default()).is_err());
    }
}
#[test]
fn legacy_css_colors_units_and_transitions() {
    let source = r##"{"variables":{"base":[64,128,192],"anim":{"target":0.8,"speed":2,"interpolation":"smoothstep"}},"rules":[{"class":"x",
       "fg":"color(var(base) a(50%))","bg":["hsl",120,100,50,0.25],"color_scheme_tint":["foreground",0.5],
       "color_scheme_tint_2":["background",255,255,255,0.1],"match_fg":["accent","grayscale",0],
       "layer0.opacity":"var(anim)","layer1.tint":{"target":"red","speed":2},
       "layer2.texture":{"keyframes":["a.png","b.png"],"loop":true,"frame_time":0.1},
       "font.size":"1.2rem","content_margin":[1,2,3,4],"shadow_offset":[1,-2],"future_property":{"a":true}}]}"##;
    let theme = Theme::parse(source, &Palette::default()).unwrap();
    assert_eq!(theme.diagnostics.len(), 1);
    let s = resolve(
        source,
        &[Element::new("x", &[])],
        &Settings::default(),
        Platform::Mac,
    );
    assert_eq!(s.color(&Property::Fg).unwrap().rgba8(), [64, 128, 192, 128]);
    assert_eq!(s.color(&Property::Bg).unwrap().rgba8(), [0, 255, 0, 64]);
    let Some(PropertyValue::FontSize(size)) = s.get(&Property::FontSize) else {
        panic!()
    };
    assert_eq!(size.pixels(10.), 12.);
    let Some(PropertyValue::Number(a)) = s.get(&Property::Layer(0, LayerProperty::Opacity)) else {
        panic!()
    };
    assert_eq!(a.sample(0., 0.25), 0.4);
    assert_eq!(a.sample(0., 1.), 0.8);
    let Some(PropertyValue::Color(a)) = s.get(&Property::Layer(1, LayerProperty::Tint)) else {
        panic!()
    };
    assert_eq!(a.sample(Color::rgb(0, 0, 0), 0.25), Color::rgb(128, 0, 0));
    let Some(PropertyValue::Texture(texture)) = s.get(&Property::Layer(2, LayerProperty::Texture))
    else {
        panic!()
    };
    assert_eq!(texture.frame(0.15), "b.png");
    assert_eq!(texture.frame(0.25), "a.png");
    for (key, value) in [
        ("opacity", json!(2)),
        ("font.size", json!("NaNrem")),
        ("tint_index", json!(4)),
        ("min_size", json!([1])),
        ("font.bold", json!("yes")),
        ("layer0.opacity", json!({"target":1,"speed":0})),
        ("layer1.texture", json!({"keyframes":[]})),
    ] {
        let mut rule = json!({"class":"x"});
        rule[key] = value;
        assert!(
            Theme::parse(&json!({"rules":[rule]}).to_string(), &Palette::default()).is_err(),
            "{key}"
        );
    }
}
#[test]
fn cache_settings_platform_state_and_replacement() {
    let theme=Arc::new(Theme::parse(r#"[{"class":"x","fg":"red"},{"class":"x","settings":["on"],"fg":"blue"},{"class":"x","attributes":["hover"],"fg":"green"},{"class":"x","platforms":["linux"],"fg":"yellow"}]"#,&Palette::default()).unwrap());
    let mut r = Resolver::with_capacity(theme, 2);
    let path = ElementPath(&[Element::new("x", &[])]);
    let mut a = Settings::default();
    a.set(Layer::User, "on", json!(true));
    let mut b = Settings::default();
    b.set(Layer::User, "on", json!(false));
    assert_eq!(a.revision(), b.revision());
    assert_eq!(
        r.resolve(&path, &a, Platform::Mac).color(&Property::Fg),
        Some(Color::rgb(0, 0, 255))
    );
    assert_eq!(
        r.resolve(&path, &b, Platform::Mac).color(&Property::Fg),
        Some(Color::rgb(255, 0, 0))
    );
    a.set(Layer::User, "on", json!(false));
    assert_eq!(
        r.resolve(&path, &a, Platform::Mac).color(&Property::Fg),
        Some(Color::rgb(255, 0, 0))
    );
    assert_eq!(
        r.resolve(&path, &a, Platform::Linux).color(&Property::Fg),
        Some(Color::rgb(255, 255, 0))
    );
    let hover = ElementPath(&[Element::new("x", &["hover"])]);
    assert_eq!(
        r.resolve(&hover, &a, Platform::Mac).color(&Property::Fg),
        Some(Color::rgb(0, 128, 0))
    );
    r.replace_theme(Arc::new(Theme::parse("[]", &Palette::default()).unwrap()));
    assert_eq!(r.resolve(&path, &a, Platform::Mac).iter().count(), 0);
}
#[test]
fn original_theme_registry_and_lane_a_coverage() {
    for appearance in [Appearance::Dark, Appearance::Light] {
        let scheme = if appearance == Appearance::Dark {
            vel_color_scheme::ColorScheme::velocity_dark()
        } else {
            vel_color_scheme::ColorScheme::velocity_light()
        };
        let palette = Palette::from_scheme(&scheme);
        let theme = Theme::velocity(appearance, &palette);
        assert!(theme.diagnostics.is_empty(), "{:?}", theme.diagnostics);
        let mut resolver = Resolver::new(Arc::new(theme));
        let settings = Settings::default();
        for e in ELEMENTS {
            let path = ElementPath(&[Element::new(e.class, &[])]);
            assert!(
                resolver
                    .resolve(&path, &settings, Platform::Mac)
                    .color(&Property::Fg)
                    .is_some(),
                "{}",
                e.class
            );
        }
        for (alias, _) in WIDGET_ALIASES {
            let path = ElementPath(&[Element::new(alias, &[])]);
            assert!(
                resolver
                    .resolve(&path, &settings, Platform::Mac)
                    .color(&Property::Fg)
                    .is_some(),
                "{alias}"
            );
        }
        let path = ElementPath(&[
            Element::new("quick_panel_row", &["selected"]),
            Element::new("quick_panel_label", &[]),
        ]);
        let fg = resolver
            .resolve(&path, &settings, Platform::Mac)
            .color(&Property::Fg)
            .unwrap();
        // The selected row is a translucent accent pill over the quick panel: the label is
        // checked against the color actually drawn under it.
        let row = ElementPath(&[Element::new("quick_panel_row", &["selected"])]);
        let row = resolver.resolve(&row, &settings, Platform::Mac);
        let mut tint = row.color(&Property::Layer(0, LayerProperty::Tint)).unwrap();
        let opacity = row
            .number(&Property::Layer(0, LayerProperty::Opacity))
            .unwrap_or(1.);
        tint.a = (f32::from(tint.a) * opacity).round() as u8;
        let panel = ElementPath(&[Element::new("overlay_control", &[])]);
        let panel = resolver
            .resolve(&panel, &settings, Platform::Mac)
            .color(&Property::Layer(0, LayerProperty::Tint))
            .unwrap();
        assert_eq!(panel.a, 255);
        let bg = tint.over(panel);
        assert!(
            fg.contrast(bg) >= 4.5,
            "{appearance:?}: {}",
            fg.contrast(bg)
        );
    }
}
#[test]
fn file_icons_precedence_and_compound_extensions() {
    let mut icons=FileIcons::parse(r#"{"icons":{"txt":"text","Makefile":"make","ts":"ts","d.ts":"declaration","src/main.rs":"entry","rs":"rust","*":"generic"}}"#).unwrap();
    for (path, icon) in [
        ("a.txt", "text"),
        ("build/Makefile", "make"),
        ("a.d.ts", "declaration"),
        ("x/src/main.rs", "entry"),
        ("othermain.rs", "rust"),
        ("x.unknown", "generic"),
        ("x\\src\\main.rs", "entry"),
    ] {
        assert_eq!(icons.resolve(Path::new(path)), Some(icon));
    }
    icons.merge(&FileIcons::parse(r#"{"icons":{"txt":"user"}}"#).unwrap());
    assert_eq!(icons.resolve(Path::new("a.txt")), Some("user"));
}
#[test]
fn png_densities_rgba_and_nine_slices() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../velocity/resources/Packages");
    let t = load_texture(
        "Theme - Velocity/folder.png",
        2.,
        Insets::all(2.),
        false,
        false,
        |name| Ok(std::fs::read(root.join(name)).ok()),
    )
    .unwrap();
    assert_eq!((t.width, t.height, t.scale), (32, 32, 2));
    assert_eq!(t.logical_size(), [16., 16.]);
    assert_eq!(t.rgba.len(), 4096);
    assert_eq!(t.patches().count(), 8);
    assert_eq!(t.patches().next().unwrap().source, [0., 0., 4., 4.]);
    assert!(
        load_texture(
            "../bad.png",
            1.,
            Insets::default(),
            true,
            false,
            |_| panic!()
        )
        .is_err()
    );
    for scale in [1., 3.] {
        let t = load_texture(
            "Theme - Velocity/close.png",
            scale,
            Insets::default(),
            true,
            true,
            |name| Ok(std::fs::read(root.join(name)).ok()),
        )
        .unwrap();
        assert_eq!(f32::from(t.scale), scale);
    }
    let fallback = load_texture(
        "Theme - Velocity/file.png",
        3.,
        Insets::default(),
        true,
        false,
        |name| {
            if name.contains('@') {
                Ok(None)
            } else {
                Ok(std::fs::read(root.join(name)).ok())
            }
        },
    )
    .unwrap();
    assert_eq!(fallback.scale, 1);
}
proptest! {
    #![proptest_config(ProptestConfig { failure_persistence: Some(Box::new(proptest::test_runner::FileFailurePersistence::Direct(concat!(env!("CARGO_MANIFEST_DIR"),"/tests/proptest-regressions.txt")))), ..ProptestConfig::default() })]
    #[test]
    fn later_rule_wins(values in prop::collection::vec(any::<u8>(),1..100)){
        let rules:Vec<_>=values.iter().map(|n|json!({"class":"x","fg":[n,0,0]})).collect();
        let s=resolve(&json!({"rules":rules}).to_string(),&[Element::new("x",&[])],&Settings::default(),Platform::Mac);
        prop_assert_eq!(s.color(&Property::Fg).unwrap().r,*values.last().unwrap());
    }
    #[test]
    fn parents_are_ordered_subsequences(noise in prop::collection::vec(0u8..3,0..40)){
        let mut chain=vec![Element::new("outer",&[])];chain.extend(noise.iter().map(|_|Element::new("gap",&[])));chain.push(Element::new("inner",&[]));chain.push(Element::new("leaf",&[]));
        let s=resolve(r#"[{"class":"leaf","parents":[{"class":"outer"},{"class":"inner"}],"font.bold":true}]"#,&chain,&Settings::default(),Platform::Mac);
        prop_assert_eq!(s.boolean(&Property::FontBold),Some(true));chain.swap(0,noise.len()+1);
        let s=resolve(r#"[{"class":"leaf","parents":[{"class":"outer"},{"class":"inner"}],"font.bold":true}]"#,&chain,&Settings::default(),Platform::Mac);prop_assert_eq!(s.boolean(&Property::FontBold),None);
    }
}
