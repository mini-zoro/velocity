//! B7: apply one User override at a time and compare rendered and operational output.
use super::*;
use std::{
    collections::BTreeSet,
    fs,
    hash::{Hash, Hasher},
};

const REFERENCE: &str = include_str!("../../../docs/settings.md");
const DEFAULTS: &str = include_str!("../resources/Packages/Default/Preferences.sublime-settings");
// A native window changes policy on its next full-screen transition. There is
// no such transition in the headless sweep; the reference must say when it applies.
const NEXT_WINDOW_TRANSITION: &[&str] = &["use_simple_full_screen"];
const SOURCE: &str = "// glorfword is intentionally misspelled\nfn brace() {\n\tcall(some_really_long_argument, other_argument);\n\tif ready {\n\t\tcall(1);\n\t}\n}\n\n[<x>]\n// \u{200b}\u{00a0}\u{202e}\u{202c}\u{7}\n";

/// Pinned to lane C's reference: its --check independently regenerates these
/// rows from runtime read sites (including indirect reads and theme rules).
fn shipped_readers() -> BTreeSet<&'static str> {
    REFERENCE
        .split_once("<!-- BEGIN GENERATED SETTINGS -->")
        .unwrap()
        .1
        .lines()
        .filter_map(|line| {
            let (key, rest) = line.strip_prefix("| `")?.split_once("` | ")?;
            (!rest.starts_with("Not shipped") && rest.contains("](../crates/")).then_some(key)
        })
        .collect()
}

fn alternate(key: &str, default: &Value, root: &Path) -> Value {
    // Boolean readers get their opposite. All other values are deliberately
    // chosen to exercise the consumer (not just a different JSON encoding).
    match key {
        "added_words" | "ignored_words" => json!(["glorfword"]),
        "auto_complete_delay" => json!(60000),
        "auto_complete_size_limit" => json!(1),
        "auto_complete_preserve_order" => json!("strict"),
        "auto_complete_selector" => json!("comment"),
        "auto_complete_triggers" => json!([{"selector":"text.plain","characters":"."}]),
        "binary_file_patterns" => json!(["*.rs"]),
        "build_env" => json!({"EFFECT_PROBE":"set-by-build"}),
        "caret_extra_bottom" | "caret_extra_top" | "caret_extra_width" => json!(3),
        "caret_style" => json!("underline"),
        "light_color_scheme" => json!("Velocity Dark.sublime-color-scheme"),
        "color_scheme" | "dark_color_scheme" => json!("Velocity Light.sublime-color-scheme"),
        "dark_theme" | "theme" => json!("Velocity Light.sublime-theme"),
        "light_theme" => json!("Velocity.sublime-theme"),
        "default_completions_selector" => json!("comment"),
        "default_dir" | "result_base_dir" => json!(root.join("alternate")),
        "default_encoding" => json!("UTF-16 LE"),
        "default_line_ending" => json!("windows"),
        "dictionary" => json!(root.join("words.dic")),
        "draw_unicode_white_space" => json!("all"),
        "draw_white_space" => json!(["all"]),
        "fallback_encoding" => json!("Cyrillic (Windows 1251)"),
        "file_exclude_patterns" | "index_exclude_patterns" => json!(["*.rs"]),
        "file_tab_style" => json!("rounded"),
        "folder_exclude_patterns" => json!(["subdir"]),
        "font_face" => json!("Monaco"),
        "font_options" => json!(["no_antialias", "bold"]),
        "font_size" => json!(21),
        "git_diff_target" => json!("head"),
        "hot_exit" => json!("disabled"),
        "ignored_packages" => json!(["Vintage", "EffectFixture"]),
        "image_file_patterns" => json!([]),
        "indent_guide_options" => json!(["draw_active"]),
        "index_include_patterns" => json!(["*.log"]),
        "index_workers" => json!(2),
        "large_file.checkpoint_interval" => json!(2),
        "large_file.max_checkpoints"
        | "large_file.max_lookback_lines"
        | "large_file.max_viewport_lines"
        | "large_file.minimap_lines" => json!(2),
        k if k.starts_with("large_file.") => json!(2),
        "line_padding_bottom" | "line_padding_top" => json!(5),
        "margin" => json!(23),
        "new_window_settings" => json!({"font_size":22}),
        "open_files_in_new_window" => json!("always"),
        "overlay_scroll_bars" => json!("disabled"),
        "package_index_url" => json!("https://example.invalid/index.json"),
        "result_file_regex" => json!(r"^(.*):(\d+):(\d+):(.*)$"),
        "result_line_regex" => json!(r"^\s+(\d+):(\d+):(.*)$"),
        "ruler_style" => json!("stippled"),
        "ruler_width" => json!(3.0),
        "rulers" => json!([12, 25]),
        "scroll_context_lines" => json!(0),
        "scroll_speed" => json!(2.0),
        "show_line_column" => json!("disabled"),
        "spelling_selector" => json!("string"),
        "sub_word_separators" => json!("-"),
        "sublime_merge_path" => json!(root.join("alternate/smerge")),
        "tab_size" => json!(8),
        "ui_font_face" => json!("Times New Roman"),
        "ui_font_size" => json!(18),
        "ui_language" => json!("zh-Hans"),
        "ui_scale" => json!(1.25),
        "vintage_ctrl_keys" => json!(false),
        "word_separators" => json!("_"),
        "wrap_width" => json!(16),
        "wrap_width_style" => json!("fixed"),
        "word_wrap" => json!(true),
        _ => json!(
            !default
                .as_bool()
                .unwrap_or_else(|| panic!("missing alternative for {key}"))
        ),
    }
}

fn user_layer(app: &mut App, key: &str, variant: Option<Value>) {
    let values = variant.map_or_else(|| json!({}), |value| json!({key: value}));
    app.workspace
        .settings
        .replace(Layer::User, values.clone())
        .unwrap();
    app.workspace.sync_ui_language();
    for t in &mut app.workspace.tabs {
        t.editor
            .settings
            .replace(Layer::User, values.clone())
            .unwrap();
        t.editor.sync();
    }
    app.workspace.refresh_window_layers().unwrap();
    app.panels.side_visible = app.workspace.window_visible("show_side_bar");
    app.panels.chrome = app.workspace.window_visible("show_status_bar");
    app.panels.message.clear();
    app.workspace.messages.clear();
}

fn replace_text(editor: &mut crate::editor::Editor, text: &str) {
    editor.run("select_all", &json!({})).unwrap();
    editor.run("insert", &json!({"characters":text})).unwrap();
    editor.set_caret(0, false);
}

fn fixture_document<T>(
    app: &mut App,
    path: &Path,
    bytes: &[u8],
    f: impl FnOnce(&mut App) -> T,
) -> T {
    fs::write(path, bytes).unwrap();
    let prev = app.workspace.active;
    app.workspace
        .open(crate::cli::FileLocation {
            path: path.to_owned(),
            line: 1,
            column: 1,
        })
        .unwrap();
    let index = app.workspace.active;
    app.workspace.prepare_sheet(index).unwrap();
    let sheet = app.workspace.tabs[index].sheet;
    let until = Instant::now() + Duration::from_secs(5);
    while app.workspace.tabs[index].loading.is_some()
        || app.workspace.tabs[index].binary_loading.is_some()
    {
        app.workspace.poll();
        assert!(
            Instant::now() < until,
            "file did not open: {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let observation = f(app);
    if let Some(index) = app.workspace.sheet_index(sheet) {
        app.workspace.close(index).unwrap();
    }
    app.workspace.select(prev);
    observation
}

fn save_probe(app: &mut App, key: &str, root: &Path) -> String {
    use std::os::unix::fs::MetadataExt;
    let path = root.join("save-probe.txt");
    fixture_document(app, &path, b"untouched  \noriginal", |app| {
        let before = fs::metadata(&path).unwrap().ino();
        let editor = app.workspace.editor_mut();
        if key == "trim_only_modified_white_space" {
            // Enable the parent operation on both runs; this setting controls its scope.
            editor.settings.set(
                Layer::View,
                "trim_trailing_white_space_on_save",
                json!(true),
            );
            editor.sync();
        }
        editor.set_caret(editor.text.len_bytes(), false);
        editor
            .run("insert", &json!({"characters":" changed   "}))
            .unwrap();
        let i = app.workspace.active;
        if key == "save_on_focus_lost" {
            app.save_on_focus_lost(None);
        } else {
            app.workspace.save(i, None).unwrap();
        }
        let replaced = before != fs::metadata(&path).unwrap().ino();
        format!("{:?}|{replaced}", fs::read(&path).unwrap())
    })
}

fn build_probe(app: &mut App, key: &str, root: &Path) -> String {
    let path = root.join("build-probe.rs");
    fixture_document(app, &path, b"original", |app| {
        app.workspace
            .editor_mut()
            .run("insert", &json!({"characters":"changed"}))
            .unwrap();
        app.panels.panel = None;
        let mut args = match key {
            "build_env" => json!({"cmd":["/usr/bin/printenv","EFFECT_PROBE"]}),
            "save_all_on_build" => json!({"build_system":"Settings Effect Fixture"}),
            _ => {
                json!({"cmd":["/bin/sh","-c",r#"printf '%s\n' 'probe.rs:2:3:error: problem' '  4:5:extra'"#],"working_dir":root})
            }
        };
        // A continuation diagnostic needs a primary diagnostic; a base-dir
        // override needs at least one parsed relative path.
        if matches!(key, "result_line_regex" | "result_base_dir") {
            args["file_regex"] = json!(r"^(.*):(\d+):(\d+):(.*)$");
        }
        app.apply_build_action(
            None,
            if key == "save_all_on_build" {
                "build"
            } else {
                "exec"
            },
            &args,
        )
        .unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while app.builds.task.is_some() {
            app.builds.poll(&mut app.panels);
            assert!(Instant::now() < until, "build timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
        let result = format!(
            "{:?}|{:?}|{:?}|{:?}",
            app.panels
                .output
                .lines()
                .filter(|line| !line.starts_with("[Finished"))
                .collect::<Vec<_>>(),
            app.panels.panel,
            app.builds.diagnostics,
            fs::read(&path).unwrap(),
        );
        app.panels.panel = None;
        app.panels.clear_output();
        result
    })
}

fn document_probe(app: &mut App, key: &str, root: &Path) -> String {
    if matches!(key, "default_encoding" | "default_line_ending") {
        let prev = app.workspace.active;
        app.workspace.new_file().unwrap();
        let i = app.workspace.active;
        let output = format!("{:?}", app.workspace.tabs[i].format);
        app.workspace.close(i).unwrap();
        app.workspace.select(prev);
        return output;
    }
    let (file, bytes): (&str, &[u8]) = match key {
        "fallback_encoding" => ("decode.txt", b"one\xE0two\n"),
        "detect_indentation" => ("indent.txt", b"a\n  b\n    c\n  d\n    e\n"),
        "image_file_patterns" => (
            "preview.png",
            include_bytes!("../resources/Packages/Theme - Velocity/palette-edit.png"),
        ),
        _ => ("load.txt", b"aaaa\n  bbbb\ncccc\n"),
    };
    fixture_document(app, &root.join(file), bytes, |app| {
        let tab = &app.workspace.tabs[app.workspace.active];
        format!(
            "{:?}|{}|{}|{:?}|{}|{}",
            tab.format,
            tab.editor.text,
            tab.editor.binary.is_some(),
            tab.editor.features,
            tab.editor.preferences.tab_size,
            tab.is_image_file()
        )
    })
}

fn session_probe(app: &mut App, key: &str, root: &Path) -> String {
    fixture_document(app, &root.join("session-probe.txt"), b"on disk", |app| {
        if key == "hot_exit" {
            app.workspace
                .editor_mut()
                .run("insert", &json!({"characters":"dirty"}))
                .unwrap();
        }
        let profile = tempfile::tempdir_in(root).unwrap();
        let sessions = std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(
            Some(profile.path()),
        )));
        let previous = std::mem::replace(&mut app.sessions, sessions);
        let previous_id = app.session_id;
        app.session_id = app.sessions.borrow_mut().register();
        // Remembering clean files is only relevant when Hot Exit is disabled.
        let previous_user = app.workspace.settings.layer(Layer::User).clone();
        if key == "remember_open_files" {
            app.workspace
                .settings
                .set(Layer::User, "hot_exit", json!("disabled"));
        }
        app.checkpoint_reason(true, true).unwrap();
        app.workspace
            .settings
            .replace(Layer::User, Value::Object(previous_user))
            .unwrap();
        app.sessions.borrow().flush().unwrap();
        let session =
            vel_session::load(&profile.path().join("Sessions/Session.sublime-session")).session;
        app.sessions = previous;
        app.session_id = previous_id;
        let docs: Vec<_> = session
            .documents
            .iter()
            .map(|d| (d.path.as_ref(), d.dirty))
            .collect();
        format!("{}|{docs:?}", session.windows.len())
    })
}

fn scene_digest(app: &mut App) -> u64 {
    let scene = app.scene();
    let mut hasher = std::hash::DefaultHasher::new();
    format!("{scene:?}").hash(&mut hasher);
    hasher.finish()
}

fn in_new_tab<T>(app: &mut App, run: impl FnOnce(&mut App) -> T) -> T {
    let prev = app.workspace.active;
    app.workspace.new_file().unwrap();
    let i = app.workspace.active;
    let output = run(app);
    app.workspace.close(i).unwrap();
    app.workspace.select(prev);
    output
}

fn edit_probe(app: &mut App, key: &str) -> String {
    in_new_tab(app, |app| {
        let (source, caret, command, args) = match key {
            "auto_close_tags" => ("<div>x<", 7, "insert", json!({"characters":"/"})),
            "auto_indent" => ("    item", 8, "insert", json!({"characters":"\n"})),
            "auto_match_enabled" => ("abc", 3, "type-paren", json!({})),
            "copy_with_empty_selection" => ("line\nother", 0, "copy", json!({})),
            "indent_to_bracket" => (
                "    let result = call(1,",
                24,
                "insert",
                json!({"characters":"\n"}),
            ),
            "move_to_limit_on_up_down" => (
                "first\nsecond",
                10,
                "move",
                json!({"by":"lines","forward":true}),
            ),
            "shift_tab_unindent" => ("\tfoo bar", 5, "shift+tab", json!({})),
            "smart_indent" => ("fn main() {", 11, "insert", json!({"characters":"\n"})),
            "sub_word_separators" => (
                "abc_def",
                0,
                "move",
                json!({"by":"subwords","forward":true}),
            ),
            "translate_tabs_to_spaces" => ("", 0, "tab", json!({})),
            "trim_automatic_white_space" => ("    item", 8, "insert", json!({"characters":"\n"})),
            "use_tab_stops" => ("    item", 4, "left_delete", json!({})),
            "word_separators" => ("abc.def", 0, "move", json!({"by":"words","forward":true})),
            _ => unreachable!("{key}"),
        };
        if key == "auto_close_tags" {
            app.workspace.set_file_type("HTML").unwrap();
        } else if matches!(key, "indent_to_bracket" | "smart_indent") {
            app.workspace.set_file_type("Rust").unwrap();
        }
        let ed = app.workspace.editor_mut();
        replace_text(ed, source);
        ed.set_caret(caret, false);
        if key == "use_tab_stops" {
            ed.settings
                .set(Layer::View, "translate_tabs_to_spaces", json!(true));
            ed.sync();
        }
        ed.clipboard.clear();
        if command == "type-paren" {
            let key = vel_config::keymap::Key::parse("(").unwrap();
            if !ed.key(&key).unwrap() {
                ed.text_input(TextInput::Insert {
                    text: "(".into(),
                    replacement: None,
                });
            }
        } else if matches!(command, "tab" | "shift+tab") {
            ed.key(&vel_config::keymap::Key::parse(command).unwrap())
                .unwrap();
        } else {
            ed.run(command, &args).unwrap();
        }
        if key == "auto_close_tags" {
            ed.finish_closing_tag_on_leave();
        }
        if key == "trim_automatic_white_space" {
            ed.run(command, &args).unwrap();
        }
        format!("{}|{}|{}", ed.text, ed.caret, ed.clipboard)
    })
}

fn find_probe(app: &mut App, key: &str) -> String {
    use vel_buffer::{Region, Selection};
    app.panels = crate::panels::Panels::new(&app.workspace, false);
    let e = app.workspace.editor_mut();
    let end = if key == "auto_find_in_selection" {
        e.text.line_to_byte(2)
    } else {
        12
    };
    e.set_selection(Selection::from_regions([Region::new(3, end)]));
    app.run_named(
        None,
        "show_panel",
        &json!({"panel":if key == "close_find_after_replace_all" { "replace" } else { "find" }}),
    );
    match key {
        "close_find_after_find_all" => app.run_named(None, "find_all", &json!({})),
        "close_find_after_replace_all" => app.run_named(None, "replace_all", &json!({})),
        _ => {}
    }
    let digest = scene_digest(app);
    let result = format!(
        "{:?}|{digest}|{:?}|{}",
        app.panels.panel,
        app.panels.input().map(|i| i.value()),
        app.panels.find_option_checked("in_selection")
    );
    app.run_named(None, "hide_panel", &json!({}));
    app.workspace.editor_mut().set_caret(0, false);
    result
}

fn observe_ui(app: &mut App, key: &str, root: &Path) -> String {
    match key {
        "default_dir" => format!("{:?}", app.save_dialog_directory(app.workspace.active)),
        "new_window_settings" => {
            user_file(
                root,
                &Value::Object(app.workspace.settings.layer(Layer::User).clone()),
            );
            let result = Workspace::new_window(Some(root.into()), false).unwrap();
            fs::remove_file(root.join("Packages/User/Preferences.sublime-settings")).unwrap();
            format!("{}", result.editor().preferences.font_size)
        }
        "open_files_in_new_window" => {
            let mut request = crate::instance::Request::new(&Options::default(), None).unwrap();
            request.files.push(crate::cli::FileLocation {
                path: root.join("other.rs"),
                line: 1,
                column: 1,
            });
            app.publish_open_route();
            format!("{:?}", app.route_open(&mut request))
        }
        "package_index_url" => app
            .command_enabled("package_check_upgrades", "{}")
            .to_string(),
        "show_full_path" | "show_rel_path" => {
            let path = root.join("alternate/title.rs");
            fixture_document(app, &path, b"// title fixture", |app| {
                app.workspace.window_title()
            })
        }
        "set_unsaved_view_name" => app.workspace.tabs[app.workspace.active].name(),
        "themed_title_bar" => format!("{:?}", app.title_bar_style()),
        "drag_text" => {
            let e = app.workspace.editor_mut();
            e.set_selection(vel_buffer::Selection::from_regions([
                vel_buffer::Region::new(0, 4),
            ]));
            let begin = crate::mouse_select::DragText::begin(e, Point::default(), 1).is_some();
            e.set_caret(0, false);
            begin.to_string()
        }
        "show_definitions" => {
            app.panels.hover_at(&app.workspace, Point::default(), 4);
            app.panels.has_pending_definition_hover().to_string()
        }
        "show_errors_inline" => {
            let mut scene = vel_render::scene::Scene::default();
            let diagnostic = vel_build::Diagnostic {
                path: root.join("input.rs"),
                line: 1,
                column: 3,
                message: "synthetic compiler error".into(),
                output_line: 0,
            };
            crate::html_ui::paint_build_errors(
                app.workspace.editor(),
                &app.view,
                std::iter::once(&diagnostic),
                &mut scene,
            );
            scene.layers.len().to_string()
        }
        "file_exclude_patterns"
        | "folder_exclude_patterns"
        | "binary_file_patterns"
        | "ignored_packages" => {
            let path = user_file(
                root,
                &Value::Object(app.workspace.settings.layer(Layer::User).clone()),
            );
            app.workspace.request_resource_reload();
            app.workspace.wait_resources();
            let folder = &app.workspace.folders[0];
            let file = folder.path.join("input.rs");
            let result = match key {
                "file_exclude_patterns" => folder.rules.allows(&folder.path, &file, false, false),
                "folder_exclude_patterns" => {
                    folder
                        .rules
                        .allows(&folder.path, &folder.path.join("subdir"), true, false)
                }
                "binary_file_patterns" => folder.rules.allows(&folder.path, &file, false, true),
                _ => app
                    .workspace
                    .palette
                    .iter()
                    .any(|p| p.caption == "Effect Fixture"),
            };
            fs::remove_file(path).unwrap();
            app.workspace.request_resource_reload();
            app.workspace.wait_resources();
            result.to_string()
        }
        "index_files" => {
            app.panels.intel.poll(&mut app.workspace);
            let deadline = Instant::now() + Duration::from_secs(5);
            while app.panels.intel.busy() {
                app.panels.intel.poll(&mut app.workspace);
                assert!(Instant::now() < deadline, "fixture index did not settle");
                std::thread::sleep(Duration::from_millis(1));
            }
            app.panels.intel.symbol_source().len().to_string()
        }
        "tree_animation_enabled" | "animation_enabled" | "preview_on_click" => {
            sidebar_probe(app, key)
        }
        "enable_tab_scrolling" | "hide_tab_scrolling_buttons" => {
            let orig = app.window_size;
            let prev = app.workspace.active;
            app.window_size.width = 530.;
            for i in 0..10 {
                app.workspace.new_file().unwrap();
                let index = app.workspace.active;
                app.workspace.tabs[index].suggested_name =
                    Some(format!("long setting fixture {i:04}.rs"));
            }
            scene_digest(app);
            let result = format!("{:?}|{:?}", app.tab_button_hits, app.tab_bars);
            while app.workspace.active != prev {
                let i = app.workspace.active;
                app.workspace.close(i).unwrap();
            }
            app.window_size = orig;
            result
        }
        "font_face" => {
            let fonts = Fonts::load_named(&app.workspace.editor().preferences.font_face).unwrap();
            format!("{:?}|{}|{}", fonts.metrics, fonts.cell, fonts.ascent)
        }
        "close_windows_when_empty" => {
            let mut empty = Workspace::new(Text::new(), None, None, false).unwrap();
            empty
                .editor_mut()
                .settings
                .replace(
                    Layer::User,
                    Value::Object(app.workspace.settings.layer(Layer::User).clone()),
                )
                .unwrap();
            let original = std::mem::replace(&mut app.workspace, empty);
            let closes = app.should_close_empty_window();
            app.workspace = original;
            closes.to_string()
        }
        "tab_completion" => in_new_tab(app, |app| {
            let e = app.workspace.editor_mut();
            replace_text(e, "prefix\npr");
            e.set_caret(e.text.len_bytes(), false);
            e.intelligence_command("insert_best_completion", &json!({}))
                .unwrap()
                .to_string()
        }),
        "scroll_speed" | "minimap_scroll_to_clicked_text" | "minimap_horizontal_scrolling" => {
            in_new_tab(app, |app| {
                let mut view = View::new(
                    Size {
                        width: 520.,
                        height: 300.,
                    },
                    8.,
                    16.,
                );
                let e = app.workspace.editor_mut();
                replace_text(e, &format!("{}\n", "wide ".repeat(40)).repeat(560));
                if key == "minimap_horizontal_scrolling" {
                    e.settings
                        .set(Layer::View, "large_file.viewport_bytes", json!(2));
                    e.settings.set(Layer::View, "word_wrap", json!(false));
                    e.sync();
                }
                let _ = view.scene(e);
                if key == "scroll_speed" {
                    let now = Instant::now();
                    let from = view.prepare_keyboard_scroll(e, now);
                    view.scroll(e, 0., 1600.);
                    view.animate_scroll_from(e, from, now, false);
                    view.advance_scroll(e, now + Duration::from_millis(40));
                    return view.scroll_y.to_string();
                }
                if key == "minimap_scroll_to_clicked_text" {
                    let point = Point {
                        x: view.right(e) + 5.,
                        y: 230.,
                    };
                    view.pointer(e, point, true, false, false).unwrap();
                    return view.scroll_y.to_string();
                }
                view.scroll(e, 250., 0.);
                format!("{:?}", view.scene(e))
            })
        }
        "sublime_merge_path" | "show_git_status_in_status_bar" | "show_git_status" => {
            fixture_document(app, &root.join("git-effect.txt"), b"Git fixture", |app| {
                let path = app.workspace.tabs[app.workspace.active]
                    .path
                    .clone()
                    .unwrap();
                app.workspace.git.insert_snapshot(vel_git::StatusSnapshot {
                    root: path.parent().unwrap().to_owned(),
                    generation: 1,
                    head: vel_git::Head::Branch {
                        name: "fixture-branch".into(),
                        unborn: false,
                    },
                    ahead_behind: None,
                    files: Default::default(),
                    folders: Default::default(),
                });
                if key == "sublime_merge_path" {
                    return app
                        .panels
                        .git_items(&app.workspace, &path, false)
                        .len()
                        .to_string();
                }
                if key == "show_git_status" {
                    app.workspace.poll();
                }
                scene_digest(app).to_string()
            })
        }
        "scroll_past_end" | "scroll_context_lines" => in_new_tab(app, |app| {
            let mut view = View::new(
                Size {
                    width: 240.,
                    height: 120.,
                },
                8.,
                16.,
            );
            let e = app.workspace.editor_mut();
            replace_text(e, &"line text\n".repeat(140));
            e.set_caret(e.text.line_to_byte(40), false);
            if key == "scroll_past_end" {
                view.scroll(e, 0., 10000.);
            } else {
                view.reveal(e);
            }
            view.scroll_y.to_string()
        }),
        "mouse_wheel_switches_tabs" => in_new_tab(app, |app| {
            app.scene();
            let r = app.group_bounds[0];
            app.handle_scroll_event(
                None,
                ScrollEvent {
                    position: Point {
                        x: r.x + 5.,
                        y: r.y + 5.,
                    },
                    delta_x: 0.,
                    delta_y: -1.,
                    precise: false,
                    phase: 0,
                    momentum_phase: 0,
                    modifiers: Default::default(),
                    timestamp: 0.,
                },
            );
            app.workspace.active.to_string()
        }),
        "update_system_recent_files" => {
            let enabled = app
                .workspace
                .global_bool("update_system_recent_files", true);
            let mut session = crate::session::Sessions::new(None);
            let id = session.register();
            session.sync_recents(
                id,
                [&mut vec![root.join("added.txt")], &mut vec![], &mut vec![]],
            );
            let mut published = Vec::new();
            session.update_system_recents(enabled, |p| published.push(p.map(Path::to_owned)));
            format!("{published:?}")
        }
        "vintage_enabled"
        | "vintage_start_in_command_mode"
        | "vintage_ctrl_keys"
        | "vintage_use_clipboard" => {
            // Fresh vi instance for start-in-command-mode; its live toggle is a separate behavior.
            in_new_tab(app, |app| {
                let e = app.workspace.editor_mut();
                if key != "vintage_enabled" {
                    e.settings.set(Layer::View, "vintage_enabled", json!(true));
                }
                if matches!(key, "vintage_ctrl_keys" | "vintage_use_clipboard") {
                    e.settings
                        .set(Layer::View, "vintage_start_in_command_mode", json!(true));
                }
                e.sync();
                replace_text(e, "yank me\n");
                if key == "vintage_ctrl_keys" {
                    e.key(&vel_config::keymap::Key::parse("ctrl+f").unwrap())
                        .unwrap();
                }
                if key == "vintage_use_clipboard" {
                    e.clipboard.clear();
                    for stroke in ["y", "y"] {
                        if let Some(inv) = e.vintage_text(stroke) {
                            e.run(&inv.command, &inv.args).unwrap();
                        }
                    }
                }
                format!("{}|{}|{}", e.vintage_status(), e.caret, e.clipboard)
            })
        }
        _ => String::new(),
    }
}

fn sidebar_probe(app: &mut App, key: &str) -> String {
    let mut panels = crate::panels::Panels::new(&app.workspace, true);
    panels.layout(app.window_size);
    let root = app.workspace.folders[0].path.clone();
    let previous = app.workspace.window_layers.normal.clone();
    let mut expand = |panels: &mut crate::panels::Panels, path: &Path, child: &Path| {
        panels
            .open_sidebar_directory(&mut app.workspace, path.to_owned())
            .unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        while !panels.sidebar_has_row(child) {
            panels.poll(&mut app.workspace, 0..20);
            assert!(
                Instant::now() < until,
                "sidebar did not publish {}",
                child.display()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    };
    let dir = root.join("subdir");
    let file = dir.join("child.txt");
    expand(&mut panels, &root, &dir);
    expand(&mut panels, &dir, &file);
    let result = if key == "preview_on_click" {
        let point = panels.test_sidebar_point(&app.workspace, &file);
        let prev = app.workspace.active;
        panels
            .mouse(&mut app.workspace, point, true, false, false, 0..20)
            .unwrap();
        let opened = app.workspace.active != prev;
        if opened {
            let index = app.workspace.active;
            app.workspace.close(index).unwrap();
            app.workspace.select(prev);
        }
        opened
    } else {
        panels.sidebar_animates()
    };
    app.workspace.window_layers.normal = previous;
    app.workspace.refresh_window_layers().unwrap();
    result.to_string()
}

// Compare the configuration actually handed to the worker, never the raw User
// value. The completion, index, highlighting and annotation suites cover the
// worker algorithms separately.
fn engine_configuration(app: &App, key: &str) -> Option<String> {
    let e = app.workspace.editor();
    if key == "auto_complete"
        || key.starts_with("auto_complete_")
        || matches!(
            key,
            "default_completions_selector" | "disable_default_completions"
        )
    {
        return Some(format!("{:?}", crate::completion::settings(e)));
    }
    if key.starts_with("index_") && key != "index_files" {
        return Some(format!(
            "{:?}",
            crate::intelligence::index_options(&app.workspace.settings)
        ));
    }
    if key.starts_with("large_file.") && key != "large_file.load_bytes" {
        if key.starts_with("large_file.max_") || key == "large_file.checkpoint_interval" {
            return Some(format!(
                "{:?}",
                crate::language::viewport_options(&e.settings)
            ));
        }
        let metrics = vel_largefile::DocumentMetrics {
            bytes: 2048,
            lines: 20,
            longest_line_bytes: 100,
        };
        return Some(format!(
            "{:?}",
            crate::large::FeatureConfig::new(&e.settings).evaluate(metrics)
        ));
    }
    if matches!(
        key,
        "added_words"
            | "ignored_words"
            | "spelling_selector"
            | "dictionary"
            | "git_diff_target"
            | "mini_diff"
            | "spell_check"
    ) {
        return Some(format!(
            "{:?}",
            crate::annotations::worker_configuration(&e.settings)
        ));
    }
    None
}

fn shaped_scene_probe(app: &mut App, key: &str) -> String {
    use vel_buffer::{Region, Selection};
    in_new_tab(app, |app| {
        let (syntax, source, anchor, caret) = match key {
            "match_tags" => ("HTML", "<div>tag</div>\n", 6, 6),
            "match_brackets_angle" => ("Rust", "type Pair = Vec<u32>;\n", 16, 16),
            "match_brackets_square" => ("Rust", "let v = [10, 20];\n", 8, 8),
            "match_brackets" | "match_brackets_braces" => ("Rust", "fn f() {\n    f();\n}\n", 7, 7),
            "match_brackets_content" => ("Rust", "fn f() {\n    f();\n}\n", 11, 11),
            "fold_buttons" => ("Rust", "fn f() {\n    f();\n}\n", 0, 0),
            "match_selection" => ("Plain Text", "token another token token\n", 0, 5),
            "word_wrap" => (
                "Rust",
                "let wide = \"wide wide wide wide wide wide wide wide wide\";\nend",
                0,
                0,
            ),
            _ => (
                "Plain Text",
                "    wide wide wide wide wide wide wide wide wide wide\nend",
                0,
                0,
            ),
        };
        app.workspace.set_file_type(syntax).unwrap();
        let e = app.workspace.editor_mut();
        replace_text(e, source);
        e.set_selection(Selection::from_regions([Region::new(anchor, caret)]));
        match key {
            "fold_buttons" => e
                .settings
                .set(Layer::View, "fade_fold_buttons", json!(false)),
            "inverse_caret_state" => e.settings.set(Layer::View, "caret_style", json!("blink")),
            "wrap_width" | "wrap_width_style" | "indent_subsequent_lines" => {
                e.settings.set(Layer::View, "word_wrap", json!(true));
                if key == "wrap_width_style" {
                    e.settings.set(Layer::View, "wrap_width", json!(40));
                } else if key == "indent_subsequent_lines" {
                    e.settings.set(Layer::View, "wrap_width", json!(16));
                }
            }
            _ => {}
        }
        e.sync();
        let mut view = View::new(
            Size {
                width: 285.,
                height: 190.,
            },
            8.,
            16.,
        );
        if syntax != "Plain Text" {
            let until = Instant::now() + Duration::from_secs(3);
            while e
                .language
                .borrow()
                .as_ref()
                .is_none_or(|l| l.analysis.is_none())
            {
                let _ = view.scene(e);
                e.poll_language();
                assert!(Instant::now() < until, "analysis did not arrive for {key}");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        view.reset_caret();
        format!("{:?}", view.scene(e))
    })
}

fn watched_file_probe(app: &mut App, key: &str, root: &Path) -> String {
    let directory = tempfile::tempdir_in(root).unwrap();
    let path = directory.path().join("watched.txt");
    fixture_document(app, &path, b"original", |app| {
        let sheet = app.workspace.tabs[app.workspace.active].sheet;
        if key == "close_deleted_files" {
            fs::remove_file(&path).unwrap();
        } else {
            fs::write(&path, "replacement content").unwrap();
        }
        let until = Instant::now() + Duration::from_secs(4);
        loop {
            app.workspace.poll();
            let Some(i) = app.workspace.sheet_index(sheet) else {
                return "closed".into();
            };
            let tab = &app.workspace.tabs[i];
            if tab.deleted || tab.external.is_some() || tab.editor.text == "replacement content" {
                return format!(
                    "{}|{}|{}",
                    tab.deleted,
                    tab.external.is_some(),
                    tab.editor.text
                );
            }
            assert!(
                Instant::now() < until,
                "file notification did not arrive: {key}"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    })
}

fn exercise(app: &mut App, key: &str, root: &Path) -> String {
    if let Some(options) = engine_configuration(app, key) {
        return options;
    }
    match key {
        "atomic_save"
        | "ensure_newline_at_eof_on_save"
        | "save_on_focus_lost"
        | "trim_only_modified_white_space"
        | "trim_trailing_white_space_on_save" => save_probe(app, key, root),
        "build_env"
        | "result_base_dir"
        | "result_file_regex"
        | "result_line_regex"
        | "save_all_on_build"
        | "show_panel_on_build" => build_probe(app, key, root),
        "default_encoding"
        | "default_line_ending"
        | "fallback_encoding"
        | "detect_indentation"
        | "image_file_patterns"
        | "large_file.load_bytes" => document_probe(app, key, root),
        "hot_exit" | "remember_open_files" => session_probe(app, key, root),
        "auto_close_tags"
        | "auto_indent"
        | "auto_match_enabled"
        | "copy_with_empty_selection"
        | "indent_to_bracket"
        | "move_to_limit_on_up_down"
        | "shift_tab_unindent"
        | "smart_indent"
        | "sub_word_separators"
        | "translate_tabs_to_spaces"
        | "trim_automatic_white_space"
        | "use_tab_stops"
        | "word_separators" => edit_probe(app, key),
        "auto_find_in_selection"
        | "find_selected_text"
        | "close_find_after_find_all"
        | "close_find_after_replace_all" => find_probe(app, key),
        "fold_buttons"
        | "inverse_caret_state"
        | "match_brackets"
        | "match_brackets_angle"
        | "match_brackets_braces"
        | "match_brackets_content"
        | "match_brackets_square"
        | "match_selection"
        | "match_tags"
        | "word_wrap"
        | "wrap_width"
        | "wrap_width_style"
        | "indent_subsequent_lines" => shaped_scene_probe(app, key),
        "close_deleted_files" | "reload_file_on_change" => watched_file_probe(app, key, root),
        _ => observe_ui(app, key, root),
    }
}

#[test]
fn shipped_user_settings_change_the_scene_or_operation() {
    let start = Instant::now();
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("alternate")).unwrap();
    fs::create_dir(root.path().join("subdir")).unwrap();
    fs::write(root.path().join("subdir/child.txt"), "child").unwrap();
    fs::create_dir_all(root.path().join("Packages/EffectFixture")).unwrap();
    fs::create_dir_all(root.path().join("Packages/User")).unwrap();
    fs::write(
        root.path()
            .join("Packages/User/Settings Effect Fixture.sublime-build"),
        r#"{"cmd":["/usr/bin/true"]}"#,
    )
    .unwrap();
    fs::write(
        root.path()
            .join("Packages/EffectFixture/Effect.sublime-commands"),
        r#"[{"caption":"Effect Fixture","command":"noop"}]"#,
    )
    .unwrap();
    let defaults = vel_config::json::Document::parse(DEFAULTS).unwrap().value;
    let project = root.path().join("effects.sublime-project");
    fs::write(&project, r#"{"folders":[{"path":"."}]}"#).unwrap();
    let mut app = app(None);
    app.workspace =
        Workspace::new(Text::new(), Some(root.path().into()), Some(project), true).unwrap();
    replace_text(app.workspace.editor_mut(), SOURCE);
    app.workspace.materialize_input();
    app.workspace.set_file_type("Rust").unwrap();
    let original = app.workspace.settings.clone();
    let mut unchanged = BTreeSet::new();
    let mut no_scene_diff = BTreeSet::new();
    let mut no_operation_diff = BTreeSet::new();
    for key in shipped_readers() {
        let value = alternate(key, &defaults[key], root.path());
        let mut candidate = json!({key:value.clone()}).as_object().unwrap().clone();
        assert!(
            vel_config::settings::validate_layer(&mut candidate, defaults.as_object().unwrap())
                .is_empty(),
            "invalid fixture value: {key}"
        );
        assert_ne!(
            &value,
            original.get(key).unwrap(),
            "alternative equals platform default: {key}"
        );
        let mut fingerprints = Vec::new();
        app.workspace
            .set_appearance(!matches!(key, "light_theme" | "light_color_scheme"))
            .unwrap();
        let mut authored = std::collections::BTreeMap::new();
        if matches!(key, "ruler_style" | "ruler_width") {
            authored.insert("rulers", json!([10, 20]));
        }
        if key == "wrap_width_style" {
            authored.insert("wrap_width", json!(16));
            authored.insert("word_wrap", json!(true));
        }
        for v in [None, Some(value)] {
            user_layer(&mut app, key, v);
            if app.workspace.editor().text != SOURCE {
                replace_text(app.workspace.editor_mut(), SOURCE);
            }
            app.workspace
                .editor_mut()
                .settings
                .replace(Layer::View, json!(authored))
                .unwrap();
            app.workspace.editor_mut().sync();
            if key.contains("color_scheme") {
                app.workspace
                    .configure_language(app.workspace.active, None, false)
                    .unwrap();
            }
            // Use an untouched fixture document for the render before running a
            // save, build, session or input operation that would change it.
            let baseline = scene_digest(&mut app);
            let action = exercise(&mut app, key, root.path());
            fingerprints.push((baseline, action));
        }
        if fingerprints[0].0 == fingerprints[1].0 {
            no_scene_diff.insert(key);
        }
        if fingerprints[0] == fingerprints[1] {
            unchanged.insert(key);
        }
        if matches!(
            key,
            "atomic_save"
                | "ensure_newline_at_eof_on_save"
                | "save_on_focus_lost"
                | "trim_only_modified_white_space"
                | "trim_trailing_white_space_on_save"
                | "build_env"
                | "result_base_dir"
                | "result_file_regex"
                | "result_line_regex"
                | "save_all_on_build"
                | "show_panel_on_build"
                | "hot_exit"
                | "remember_open_files"
        ) && fingerprints[0].1 == fingerprints[1].1
        {
            no_operation_diff.insert(key);
        }
        user_layer(&mut app, key, None);
    }
    eprintln!("No direct scene delta (including operational settings): {no_scene_diff:?}");
    eprintln!("No headless effect (documented native transition): {unchanged:?}");
    eprintln!("Settings effect sweep: {:?}", start.elapsed());
    let lifecycle = REFERENCE
        .split_once("## Live changes and when they apply")
        .unwrap()
        .1
        .split_once("<!-- BEGIN GENERATED SETTINGS -->")
        .unwrap()
        .0;
    for key in NEXT_WINDOW_TRANSITION {
        assert!(
            lifecycle
                .lines()
                .any(|line| line.contains(&format!("`{key}`"))
                    && line.contains("next native window"))
        );
    }
    let unexpected: Vec<_> = unchanged
        .iter()
        .filter(|key| !NEXT_WINDOW_TRANSITION.contains(key))
        .collect();
    assert!(
        unexpected.is_empty() && no_operation_diff.is_empty(),
        "unexpected missing effect: {unexpected:?}; missing operation delta: {no_operation_diff:?}"
    );
}
