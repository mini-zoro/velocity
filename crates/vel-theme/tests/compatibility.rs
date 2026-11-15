use serde_json::json;
use std::sync::Arc;
use vel_theme::*;
#[test]
fn null_resets_color_aliases_and_css_tint_modifiers() {
    let theme=Theme::parse(r##"[{"class":"x","fg":"red","layer0.tint":"red"},{"class":"x","color":"blue","layer0.tint":null,"tint_modifier":"#1234","tint_index":-1}]"##,&Palette::default()).unwrap();
    let mut r = Resolver::new(Arc::new(theme));
    let s = r.resolve(
        &ElementPath(&[Element::new("x", &[])]),
        &Settings::default(),
        Platform::Mac,
    );
    assert_eq!(s.color(&Property::Fg), Some(Color::rgb(0, 0, 255)));
    assert_eq!(s.get(&Property::Layer(0, LayerProperty::Tint)), None);
    assert_eq!(
        s.get(&Property::TintModifier),
        Some(&PropertyValue::Modifier([17, 34, 51, 68]))
    );
    assert_eq!(s.number(&Property::TintIndex), Some(-1.));
}
#[test]
fn all_property_names_and_layer_fields_are_typed() {
    for name in PROPERTIES {
        assert!(
            !matches!(Property::named(name), Property::Extension(_)),
            "{name}"
        );
    }
    for layer in 0..4 {
        for key in [
            "tint",
            "opacity",
            "texture",
            "inner_margin",
            "draw_center",
            "repeat",
            "tint_modifier",
        ] {
            assert!(matches!(
                Property::named(&format!("layer{layer}.{key}")),
                Property::Layer(_, _)
            ));
        }
    }
}
#[test]
fn predefined_scheme_overrides_and_appearance_thresholds() {
    let p=Palette::from_scheme_json(r##"{"variables":{"ink":"#123456","--redish":"var(ink)"},"globals":{"background":"white","foreground":"black"}}"##).unwrap();
    assert_eq!(p.color("var(--redish)").unwrap(), Color::rgb(18, 52, 86));
    assert!(!p.is_dark());
    assert_eq!(luminosity(Color::rgb(0, 0, 0)), "file_dark");
    assert_eq!(luminosity(Color::rgb(26, 0, 0)), "file_medium_dark");
    assert_eq!(luminosity(Color::rgb(77, 0, 0)), "file_medium");
    assert_eq!(luminosity(Color::rgb(153, 0, 0)), "file_light");
}
#[test]
fn original_setting_states_and_direct_lane_a_widgets() {
    let mut r = Resolver::new(Arc::new(Theme::velocity(
        Appearance::Dark,
        &Palette::default(),
    )));
    let mut settings = Settings::default();
    settings.set(
        vel_config::settings::Layer::User,
        "show_tab_close_buttons",
        json!(true),
    );
    let close = ElementPath(&[Element::new("tab_close_button", &[])]);
    assert_eq!(
        r.resolve(&close, &settings, Platform::Mac)
            .number(&Property::Layer(0, LayerProperty::Opacity)),
        Some(1.)
    );
    settings.set(
        vel_config::settings::Layer::User,
        "show_tab_close_buttons",
        json!(false),
    );
    assert_eq!(
        r.resolve(&close, &settings, Platform::Mac)
            .number(&Property::Layer(0, LayerProperty::Opacity)),
        Some(0.)
    );
    for class in ["tab_control", "open_file"] {
        let path = ElementPath(&[Element::new(class, &["transient"])]);
        assert_eq!(
            r.resolve(&path, &settings, Platform::Mac)
                .boolean(&Property::FontItalic),
            Some(true)
        );
    }
    let path = ElementPath(&[
        Element::new("kind_container kind_function", &[]),
        Element::new("kind_label", &[]),
    ]);
    assert_eq!(
        r.resolve(&path, &settings, Platform::Mac)
            .color(&Property::Fg),
        Some(Color::rgb(255, 155, 147))
    );
}
#[test]
fn png_rgb_gray_alpha_palette_and_decode_errors() {
    for (kind, data, expected) in [
        (png::ColorType::Rgb, vec![1, 2, 3], vec![1, 2, 3, 255]),
        (png::ColorType::Grayscale, vec![9], vec![9, 9, 9, 255]),
        (
            png::ColorType::GrayscaleAlpha,
            vec![9, 64],
            vec![9, 9, 9, 64],
        ),
        (png::ColorType::Indexed, vec![0], vec![1, 2, 3, 17]),
    ] {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(kind);
            encoder.set_depth(png::BitDepth::Eight);
            if kind == png::ColorType::Indexed {
                encoder.set_palette(vec![1, 2, 3]);
                encoder.set_trns(vec![17]);
            }
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&data)
                .unwrap();
        }
        let t = load_texture("p/x.png", 1., Insets::default(), true, false, |_| {
            Ok(Some(bytes.clone()))
        })
        .unwrap();
        assert_eq!(t.rgba, expected);
    }
    assert!(
        load_texture("p/x.png", 1., Insets::default(), true, false, |_| Ok(Some(
            vec![0; 5]
        )))
        .is_err()
    );
}
