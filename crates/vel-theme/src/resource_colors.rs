//! Every compiled color property, including the sheet modifier, frozen on main 99d1bb4.
use super::*;

#[test]
fn shipped_theme_compiled_colors_stay_identical() {
    let mut colors = BTreeMap::new();
    for (name, appearance, scheme) in [
        (
            "dark",
            Appearance::Dark,
            vel_color_scheme::ColorScheme::velocity_dark(),
        ),
        (
            "light",
            Appearance::Light,
            vel_color_scheme::ColorScheme::velocity_light(),
        ),
    ] {
        let palette = Palette::from_scheme(&scheme);
        let theme = Theme::velocity(appearance, &palette);
        for (index, rule) in theme.rules.iter().enumerate() {
            for (property, value) in &rule.values {
                let color = match value {
                    PropertyValue::Color(animated) => Some(animated.target),
                    PropertyValue::Text(modifier) if *property == Property::BackgroundModifier => {
                        Some(
                            palette
                                .color(&format!("color(var(--background) {modifier})"))
                                .unwrap(),
                        )
                    }
                    _ => None,
                };
                if let Some(color) = color {
                    colors.insert(format!("{name}/{index}/{property:?}"), color.rgba8());
                }
            }
        }
    }
    let actual = serde_json::to_string(&colors).unwrap() + "\n";
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/resource-colors.json");
    assert_eq!(actual, std::fs::read_to_string(path).unwrap());
}
