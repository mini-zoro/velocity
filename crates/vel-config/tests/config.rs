use proptest::prelude::*;
use vel_config::{
    Value, json,
    json::Document,
    keymap::{ContextProvider, Key, Keymap, Resolution},
    resources::Resources,
    settings::{Layer, Settings},
    ui::{self, MenuItem, MouseBinding, PaletteItem},
};
#[test]
fn spans_and_lossless_edits_preserve_bom_comments_crlf_and_layout() {
    let source = "\u{feff}{\r\n  // tab width\r\n  \"tab_size\" : 4, /* retained */\r\n  \"nested\": [\"https://x/*y*/\", {\"a/b\":true,},],\r\n}\r\n";
    let doc = Document::parse(source).unwrap();
    assert_eq!(&source[doc.spans["/nested/1/a~1b"].clone()], "true");
    let edited = doc.set("tab_size", &json!(8)).unwrap();
    assert_eq!(edited, source.replacen(" : 4", " : 8", 1));
    let inserted = Document::parse(&edited)
        .unwrap()
        .set("new", &json!([1, 2]))
        .unwrap();
    assert!(inserted.contains("/* retained */"));
    assert_eq!(
        Document::parse(&inserted).unwrap().value["new"],
        json!([1, 2])
    );
    for invalid in ["/*", "{\"a\":}", "{\"a\":1,,}", "[1 2]", "\"unterminated"] {
        assert!(Document::parse(invalid).is_err(), "{invalid}");
    }
}
proptest! {
    #[test]
    fn comments_and_unrelated_tokens_survive_any_string_update(value in any::<String>()) {
        let source="{ /* retain me */ \"text\": \"old\", // end\n \"other\": [1,true,null,], }";
        let edited=Document::parse(source).unwrap().set("text",&json!(value)).unwrap();
        prop_assert!(edited.starts_with("{ /* retain me */ \"text\": "), "prefix changed");
        prop_assert!(edited.ends_with(", // end\n \"other\": [1,true,null,], }"), "suffix changed");
        prop_assert_eq!(&Document::parse(&edited).unwrap().value["text"],&json!(value));
    }
}
#[test]
fn layers_observe_only_effective_changes_and_keep_wrong_types_local() {
    use std::sync::{Arc, Mutex};
    let observed = Arc::new(Mutex::new(Vec::new()));
    let log = observed.clone();
    let mut settings = Settings::default();
    settings.observe("test", move |key, value| {
        log.lock().unwrap().push((key.to_string(), value.cloned()))
    });
    for (i, layer) in [
        Layer::Default,
        Layer::Platform,
        Layer::User,
        Layer::Project,
        Layer::Syntax,
        Layer::View,
    ]
    .into_iter()
    .enumerate()
    {
        settings.replace(layer, json!({"tab_size":i+1})).unwrap();
        assert_eq!(settings.usize("tab_size", 0), i + 1);
    }
    let n = observed.lock().unwrap().len();
    settings.set(Layer::Default, "tab_size", json!(20));
    assert_eq!(observed.lock().unwrap().len(), n);
    settings.replace(Layer::View, json!({})).unwrap();
    assert_eq!(settings.usize("tab_size", 0), 5);
    settings.set(Layer::View, "font_size", json!(18));
    settings.set(Layer::View, "line_numbers", json!("bad"));
    assert_eq!(settings.editor().font_size, 18.);
    assert!(settings.editor().line_numbers);
}
struct Context;
struct PendingScopes;
impl ContextProvider for PendingScopes {
    fn scopes_ready(&self) -> bool {
        false
    }
    fn values(&self, _: &str) -> Vec<Value> {
        vec![]
    }
    fn selector_matches(&self, _: &str) -> Vec<bool> {
        vec![false]
    }
}
#[test]
fn asynchronous_scopes_do_not_satisfy_negative_or_positive_selectors() {
    for operator in ["equal", "not_equal"] {
        let context: vel_config::keymap::Context = serde_json::from_value(
            json!({"key":"selector","operator":operator,"operand":"string, comment"}),
        )
        .unwrap();
        assert!(!context.matches(&PendingScopes));
    }
}
impl ContextProvider for Context {
    fn values(&self, key: &str) -> Vec<Value> {
        match key {
            "text" => vec![json!("one"), json!("two")],
            "selection_empty" => vec![json!(false), json!(true)],
            _ => vec![],
        }
    }
    fn selector_matches(&self, s: &str) -> Vec<bool> {
        vec![s == "text.plain", s == "text.plain"]
    }
}
fn resolve(k: &mut Keymap, key: &str) -> Resolution {
    k.resolve(&Key::parse(key).unwrap(), &Context)
}
#[test]
fn priority_regex_any_all_and_timeout_free_sequences() {
    let mut k = Keymap::default();
    k.append(r#"[
      {"keys":["super+k","super+b"],"command":"gutter"},
      {"keys":["shift","shift"],"command":"tap"},
      {"keys":["x"],"command":"base"},
      {"keys":["x"],"command":"any","context":[{"key":"text","operator":"regex_match","operand":"one"}]},
      {"keys":["x"],"command":"all","context":[{"key":"text","operator":"regex_match","operand":"one","match_all":true}]}
    ]"#).unwrap();
    assert!(matches!(resolve(&mut k,"x"),Resolution::Command(i) if i.command=="any"));
    assert_eq!(resolve(&mut k, "super+k"), Resolution::Pending);
    assert!(matches!(resolve(&mut k,"super+b"),Resolution::Command(i) if i.command=="gutter"));
    assert_eq!(resolve(&mut k, "shift"), Resolution::Pending);
    assert!(matches!(resolve(&mut k,"shift"),Resolution::Command(i) if i.command=="tap"));
    assert_eq!(resolve(&mut k, "super+k"), Resolution::Pending);
    assert!(matches!(resolve(&mut k,"x"),Resolution::Command(i) if i.command=="any"));
    for (operator, operand, expected) in [
        ("equal", json!("one"), true),
        ("not_equal", json!("one"), true),
        ("regex_match", json!("on"), false),
        ("regex_contains", json!("on"), true),
        ("not_regex_match", json!("one"), true),
        ("not_regex_contains", json!("on"), true),
    ] {
        let mut k = Keymap::default();
        k.append(&json!([{"keys":["x"],"command":"yes","context":[{"key":"text","operator":operator,"operand":operand}]}]).to_string()).unwrap();
        assert_eq!(
            matches!(resolve(&mut k, "x"), Resolution::Command(_)),
            expected,
            "{operator}"
        );
    }
    assert_eq!(Key::parse("super+shift+p").unwrap().modifiers, 12);
    assert_eq!(Key::parse("ctrl+alt+keypad_plus").unwrap().modifiers, 3);
    assert_eq!(
        Key::parse("super++").unwrap(),
        Key::parse("super+shift+=").unwrap()
    );
    assert_eq!(Key::parse("(").unwrap(), Key::parse("shift+9").unwrap());
}
#[test]
fn loose_overrides_zip_and_user_is_last() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Installed Packages")).unwrap();
    std::fs::create_dir_all(dir.path().join("Packages/Alpha")).unwrap();
    std::fs::create_dir_all(dir.path().join("Packages/User")).unwrap();
    let zip =
        std::fs::File::create(dir.path().join("Installed Packages/Alpha.sublime-package")).unwrap();
    let mut zip = zip::ZipWriter::new(zip);
    zip.start_file(
        "test.sublime-keymap",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.write_all(b"[1]").unwrap();
    zip.finish().unwrap();
    std::fs::write(dir.path().join("Packages/Alpha/test.sublime-keymap"), "[2]").unwrap();
    std::fs::write(dir.path().join("Packages/User/test.sublime-keymap"), "[3]").unwrap();
    let mut resources = Resources::default();
    resources.load(dir.path()).unwrap();
    let found = resources.matching(".sublime-keymap");
    assert_eq!(found[0].text().unwrap(), "[2]");
    assert!(found[1].path.contains("User"));
}
#[test]
fn parses_other_resource_formats_and_merges_submenus() {
    let mut menu: Vec<MenuItem> =
        ui::parse(r#"[{"id":"file","caption":"File","children":[]}]"#).unwrap();
    ui::merge_menu(
        &mut menu,
        ui::parse(r#"[{"id":"file","children":[{"caption":"Save","command":"save"}]}]"#).unwrap(),
    );
    assert_eq!(menu.len(), 1);
    assert_eq!(menu[0].children[0].command.as_deref(), Some("save"));
    let mouse: Vec<MouseBinding> =
        ui::parse(r#"[{"button":1,"count":2,"press_command":"select_word","press_args":{},}]"#)
            .unwrap();
    assert_eq!(mouse[0].count, 2);
    let palette: Vec<PaletteItem> =
        ui::parse(r#"[{"caption":"Save All","command":"save_all"}]"#).unwrap();
    assert_eq!(palette[0].invocation.command, "save_all");
}

#[test]
fn cloned_keymaps_keep_chords_and_appended_overrides_independent() {
    use vel_config::keymap::{Key, Keymap, Resolution, SettingsContext};
    let settings = vel_config::settings::Settings::default();
    let context = SettingsContext(&settings);
    let mut original = Keymap::default();
    original
        .append(r#"[{"keys":["ctrl+k","ctrl+x"],"command":"original"}]"#)
        .unwrap();
    let mut first = original.clone();
    let mut second = original.clone();
    assert_eq!(
        first.resolve(&Key::parse("ctrl+k").unwrap(), &context),
        Resolution::Pending
    );
    assert_eq!(
        second.resolve(&Key::parse("ctrl+x").unwrap(), &context),
        Resolution::Unbound
    );
    second
        .append(r#"[{"keys":["ctrl+k","ctrl+x"],"command":"override"}]"#)
        .unwrap();
    let resolve = |map: &mut Keymap| {
        assert_eq!(
            map.resolve(&Key::parse("ctrl+k").unwrap(), &context),
            Resolution::Pending
        );
        match map.resolve(&Key::parse("ctrl+x").unwrap(), &context) {
            Resolution::Command(i) => i.command,
            other => panic!("{other:?}"),
        }
    };
    match first.resolve(&Key::parse("ctrl+x").unwrap(), &context) {
        Resolution::Command(i) => assert_eq!(i.command, "original"),
        other => panic!("{other:?}"),
    }
    assert_eq!(resolve(&mut original), "original");
    assert_eq!(resolve(&mut first), "original");
    assert_eq!(resolve(&mut second), "override");
}

#[test]
fn projected_settings_keep_values_and_revisions_distinguish_independent_configs() {
    use vel_config::settings::{Layer, Settings};
    let mut first = Settings::default();
    let mut second = Settings::default();
    first.set(
        Layer::User,
        "overlay_scroll_bars",
        serde_json::json!("enabled"),
    );
    second.set(
        Layer::User,
        "overlay_scroll_bars",
        serde_json::json!("disabled"),
    );
    assert_eq!(first.revision(), second.revision());
    assert_ne!(first.cache_identity(), second.cache_identity());
    let snapshot = first.project(["overlay_scroll_bars", "absent"]);
    assert_eq!(snapshot.revision(), first.revision());
    assert_ne!(snapshot.cache_identity(), first.cache_identity());
    assert_eq!(
        snapshot.get("overlay_scroll_bars"),
        first.get("overlay_scroll_bars")
    );
    assert_eq!(snapshot.get("absent"), None);
    let copy = first.clone();
    assert_eq!(copy.revision(), first.revision());
    first.set(
        Layer::View,
        "overlay_scroll_bars",
        serde_json::json!("disabled"),
    );
    assert_ne!(copy.revision(), first.revision());
    assert_eq!(copy.string("overlay_scroll_bars", ""), "enabled");
}

#[test]
fn syntax_specific_settings_override_project_settings() {
    let mut settings = Settings::default();
    settings
        .replace(Layer::User, json!({"tab_size": 4}))
        .unwrap();
    settings
        .replace(Layer::Syntax, json!({"tab_size": 8}))
        .unwrap();
    settings
        .replace(Layer::Project, json!({"tab_size": 2, "rulers": [80]}))
        .unwrap();
    assert_eq!(settings.usize("tab_size", 0), 8);
    assert_eq!(settings.get("rulers"), Some(&json!([80])));
    let defaults = vel_config::settings::EditorSettings::default();
    assert_eq!(defaults.default_line_ending, "system");
    assert!(!defaults.atomic_save);
}
