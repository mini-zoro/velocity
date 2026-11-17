use std::sync::Arc;
use vel_config::settings::Settings;
use vel_theme::{Color, Element, ElementPath, Palette, Platform, Property, Resolver, Theme};

#[test]
fn legacy_array_tints_keep_alpha_weighting_after_blend_direction_changes() {
    let mut palette = Palette::default();
    palette.set("--background", Color::rgb(0, 0, 0));
    let theme = Theme::parse(
        r#"{"rules":[{"class":"widget","bg":["background",255,255,255,0.25]}]}"#,
        &palette,
    )
    .unwrap();
    let mut resolver = Resolver::new(Arc::new(theme));
    let path = ElementPath(&[Element::new("widget", &[])]);
    let style = resolver.resolve(&path, &Settings::default(), Platform::Mac);
    assert_eq!(style.color(&Property::Bg), Some(Color::rgb(64, 64, 64)));
}
