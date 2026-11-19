//! Frozen compiled colors from main 99d1bb4, before correcting blend direction.
use super::*;

#[test]
fn shipped_scheme_compiled_colors_stay_identical() {
    let mut colors = BTreeMap::new();
    for source in [
        include_str!(
            "../../velocity/resources/Packages/Color Scheme - Velocity/Velocity Dark.sublime-color-scheme"
        ),
        include_str!(
            "../../velocity/resources/Packages/Color Scheme - Velocity/Velocity Light.sublime-color-scheme"
        ),
    ] {
        let raw = SchemeSource::parse(source).unwrap();
        let scheme = raw.compile().unwrap();
        // Only explicit globals belong to the old palette; derived globals are tested separately.
        for key in raw.0["globals"].as_object().unwrap().keys() {
            if let Some(color) = scheme.globals.color(key) {
                colors.insert(format!("{}/global/{key}", scheme.name), color.rgba8());
            }
        }
        for (index, rule) in scheme.rules.iter().enumerate() {
            for (field, color) in [
                ("foreground", rule.foreground),
                ("background", rule.background),
                ("selection_foreground", rule.selection_foreground),
            ] {
                if let Some(color) = color {
                    colors.insert(
                        format!("{}/rule/{index}/{field}", scheme.name),
                        color.rgba8(),
                    );
                }
            }
        }
        for (index, gradient) in scheme.gradients.iter().enumerate() {
            for (at, color) in gradient.iter().enumerate() {
                colors.insert(
                    format!("{}/gradient/{index}/{at}", scheme.name),
                    color.rgba8(),
                );
            }
        }
    }
    let actual = serde_json::to_string(&colors).unwrap() + "\n";
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/resource-colors.json");
    // Deliberate scheme changes regenerate the fixture, like the other structural goldens.
    if std::env::var_os("VEL_UPDATE_GOLDENS").is_some() {
        std::fs::write(&path, &actual).unwrap();
    }
    assert_eq!(actual, std::fs::read_to_string(path).unwrap());
}
