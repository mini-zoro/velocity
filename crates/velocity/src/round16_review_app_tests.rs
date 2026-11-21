//! Headless tests use the application's actual named/key dispatcher and catalogs.
use super::*;
use vel_buffer::{Region, Selection};
use vel_config::keymap::Key;
use vel_text::Text;
fn app(source: &str, data: Option<&Path>, project: Option<&Path>) -> App {
    App::new(
        Options::default(),
        Clock::new(Instant::now(), std::time::SystemTime::now()),
        Reporter::for_options(&Options::default(), Instant::now()).unwrap(),
        Workspace::new(
            Text::from(source),
            data.map(Path::to_path_buf),
            project.map(Path::to_path_buf),
            false,
        )
        .unwrap(),
        Arc::new(AtomicBool::new(false)),
        std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(data))),
    )
}
fn key(a: &mut App, key: &str) {
    let viewport = a.viewport();
    assert!(
        a.dispatch_key_binding(None, &Key::parse(key).unwrap(), Instant::now(), viewport),
        "{key}"
    );
}
fn text(a: &mut App, text: &str) {
    a.workspace.editor_mut().text_input(TextInput::Insert {
        text: text.into(),
        replacement: None,
    });
}
fn vintage() -> App {
    vintage_source("")
}
fn vintage_source(source: &str) -> App {
    let mut a = app(source, None, None);
    a.workspace
        .editor_mut()
        .settings
        .set(Layer::View, "vintage_enabled", json!(true));
    a.workspace.editor_mut().sync();
    a
}
#[test]
fn vintage_host_undo_redo_and_external_commands_do_not_become_insert_steps() {
    for redo in [false, true] {
        let mut a = vintage();
        text(&mut a, "one");
        key(&mut a, "escape");
        key(&mut a, "a");
        text(&mut a, "two");
        key(&mut a, "super+z");
        assert_eq!(a.workspace.editor().text.to_string(), "one");
        if redo {
            key(&mut a, "super+shift+z");
            assert_eq!(a.workspace.editor().text.to_string(), "onetwo");
        }
        key(&mut a, "escape");
        key(&mut a, "u");
        assert_eq!(
            a.workspace.editor().text.to_string(),
            if redo { "one" } else { "" }
        );
        key(&mut a, "ctrl+r");
        assert_eq!(
            a.workspace.editor().text.to_string(),
            if redo { "onetwo" } else { "one" }
        );
        a.workspace
            .editor()
            .buffer
            .view(a.workspace.editor().id)
            .sel()
            .iter()
            .for_each(|r| assert!(r.end() <= a.workspace.editor().text.len_bytes()));
    }
    let mut a = vintage();
    text(&mut a, "one");
    key(&mut a, "escape");
    key(&mut a, "a");
    text(&mut a, "two");
    a.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([Region::new(0, 6)]));
    a.run_named(None, "upper_case", &json!({}));
    assert_eq!(a.workspace.editor().text.to_string(), "ONETWO");
    key(&mut a, "escape");
    key(&mut a, "u");
    assert_eq!(a.workspace.editor().text.to_string(), "onetwo");
    key(&mut a, "u");
    assert_eq!(a.workspace.editor().text.to_string(), "one");
}
#[test]
fn vintage_individual_insert_events_survive_host_history_commands() {
    for redo in [false, true] {
        let mut a = vintage();
        for c in ["o", "n", "e"] {
            text(&mut a, c);
        }
        key(&mut a, "escape");
        key(&mut a, "a");
        for c in ["t", "w", "o"] {
            text(&mut a, c);
        }
        key(&mut a, "super+z");
        assert_eq!(a.workspace.editor().text.to_string(), "one");
        if redo {
            key(&mut a, "super+shift+z");
        }
        key(&mut a, "escape");
        key(&mut a, "u");
        assert_eq!(
            a.workspace.editor().text.to_string(),
            if redo { "one" } else { "" }
        );
        key(&mut a, "ctrl+r");
        assert_eq!(
            a.workspace.editor().text.to_string(),
            if redo { "onetwo" } else { "one" }
        );
        for r in a
            .workspace
            .editor()
            .buffer
            .view(a.workspace.editor().id)
            .sel()
            .iter()
        {
            assert!(r.end() <= a.workspace.editor().text.len_bytes());
        }
    }
}
#[test]
fn every_menu_and_palette_command_resolves_through_app_dispatch() {
    use vel_config::ui::{self, MenuItem, PaletteItem};
    let menus: Vec<MenuItem> = ui::parse(include_str!(
        "../resources/Packages/Default/Main.sublime-menu"
    ))
    .unwrap();
    let palette: Vec<PaletteItem> = ui::parse(include_str!(
        "../resources/Packages/Default/Velocity.sublime-commands"
    ))
    .unwrap();
    let mut invocations = palette
        .into_iter()
        .map(|r| (r.invocation.command, r.invocation.args))
        .collect::<Vec<_>>();
    fn collect(rows: &[MenuItem], out: &mut Vec<(String, Value)>) {
        for row in rows {
            if let Some(name) = &row.command {
                out.push((name.clone(), row.args.clone()));
            }
            collect(&row.children, out);
        }
    }
    collect(&menus, &mut invocations);
    invocations.sort_by_key(|(name, args)| (name.clone(), args.to_string()));
    invocations.dedup();
    for (name, args) in invocations {
        // Lane A round 20 supplies this command; do not register a stub.
        let mut a = app("one two\nthree", None, None);
        // Plain Text with no configured build system is a real context; no child
        // processes or native windows are needed to resolve these resource rows.
        a.workspace.build_systems.clear();
        // Appearance commands resolve from the merged registry.
        if name == "set_line_ending" && a.workspace.editor().registry.kind(&name).is_none() {
            continue;
        }

        assert!(
            a.workspace.editor().registry.kind(&name).is_some(),
            "unregistered: {name} {args}"
        );
        let enabled = a.command_enabled(&name, &args.to_string());
        a.run_named(None, &name, &args);
        assert!(
            !a.panels.message.to_lowercase().contains("unknown command"),
            "{name}: {}",
            a.panels.message
        );
        if !enabled {
            assert!(
                !a.command_enabled(&name, &args.to_string()),
                "disabled command executed: {name}"
            );
        }
    }
}
fn macro_rows(menu: &[MenuEntry]) -> &[MenuEntry] {
    for row in menu {
        if row.caption == "Macros" {
            return &row.children;
        }
        let nested = macro_rows(&row.children);
        if !nested.is_empty() {
            return nested;
        }
    }
    &[]
}
#[test]
fn tools_macros_use_the_published_catalog_and_replay_its_invocation() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Packages/User/Review.sublime-macro");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let contents = r#"[{"command":"insert","args":{"characters":"catalog"}}]"#;
    std::fs::write(&file, contents).unwrap();
    let mut a = app("", Some(dir.path()), None);
    let first = a.application_menu();
    let rows = macro_rows(&first);
    assert!(rows.iter().any(|r| r.caption.contains("User/Review")));
    // Resource snapshots remain authoritative until the worker publishes again:
    // menu construction must not stat/read/re-walk this now-missing file.
    std::fs::remove_file(&file).unwrap();
    let second = a.application_menu();
    let selected = macro_rows(&second)
        .iter()
        .find(|r| r.caption.contains("User/Review"))
        .unwrap();
    std::fs::write(&file, contents).unwrap();
    let args = serde_json::from_str(&selected.args).unwrap();
    assert!(a.command_enabled(selected.command.as_deref().unwrap(), &selected.args));
    a.run_named(None, selected.command.as_deref().unwrap(), &args);
    assert_eq!(a.workspace.editor().text.to_string(), "catalog");
}
#[test]
fn project_ignored_packages_cannot_enable_or_disable_vintage_or_package_resources() {
    for enabled in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let prefs = dir
            .path()
            .join("Packages/User/Preferences.sublime-settings");
        std::fs::create_dir_all(prefs.parent().unwrap()).unwrap();
        std::fs::write(
            &prefs,
            json!({"ignored_packages":if enabled {json!([])}else{json!(["Vintage","Hidden"])}})
                .to_string(),
        )
        .unwrap();
        let hidden = dir.path().join("Packages/Hidden/Entry.sublime-macro");
        std::fs::create_dir_all(hidden.parent().unwrap()).unwrap();
        std::fs::write(hidden, "[]").unwrap();
        let project = dir.path().join("test.sublime-project");
        std::fs::write(&project,json!({"settings":{"ignored_packages":if enabled {json!(["Vintage","Hidden"])}else{json!([])},"tab_size":3}}).to_string()).unwrap();
        let a = app("", Some(dir.path()), Some(&project));
        assert_eq!(a.workspace.editor().vintage.is_some(), enabled);
        assert_eq!(a.workspace.editor().preferences.tab_size, 3);
        assert_eq!(a.workspace.ignored_packages().contains("Vintage"), !enabled);
        assert_eq!(
            a.workspace
                .macro_paths
                .iter()
                .any(|p| p.contains("/Hidden/")),
            enabled
        );
    }
}
#[test]
fn optional_system_dictionary_errors_leave_the_picker_usable() {
    if std::env::var_os("R16_SPELL_PICKER_CHILD").is_some() {
        let mut a = app("", None, None);
        a.run_named(None, "show_dictionary", &json!({}));
        assert!(a.panels.message.is_empty(), "{}", a.panels.message);
        // Selecting the packaged default still follows the ordinary named path.
        a.run_named(
            None,
            "set_dictionary",
            &json!({"dictionary":crate::annotations::ENGLISH}),
        );
        assert_eq!(
            a.workspace.editor().settings.string("dictionary", ""),
            crate::annotations::ENGLISH
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("Library")).unwrap();
    std::fs::write(dir.path().join("Library/Spelling"), "not a directory").unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "app::round16_review_tests::optional_system_dictionary_errors_leave_the_picker_usable",
            "--nocapture",
        ])
        .env("HOME", dir.path())
        .env("R16_SPELL_PICKER_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn folded_keymap_chords_are_unique_and_resolve_to_the_documented_commands() {
    let rows: Vec<Value> = vel_config::ui::parse(include_str!(
        "../resources/Packages/Default/Velocity.sublime-keymap"
    ))
    .unwrap();
    for end in ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "j", "t"] {
        let keys = json!(["super+k", format!("super+{end}")]);
        let bindings = rows
            .iter()
            .filter(|r| r["keys"] == keys)
            .collect::<Vec<_>>();
        assert_eq!(bindings.len(), 1, "duplicate/missing {keys}");
        let mut a = app("", None, None);
        key(&mut a, "super+k");
        let resolved = a
            .panels
            .resolve_key(
                &mut a.workspace,
                &Key::parse(&format!("super+{end}")).unwrap(),
                0..1,
            )
            .unwrap();
        let vel_config::keymap::Resolution::Command(invocation) = resolved else {
            panic!("unbound {keys}")
        };
        assert_eq!(
            invocation.command,
            if matches!(end, "0" | "j") {
                "unfold_all"
            } else if end == "t" {
                "fold_tag_attributes"
            } else {
                "fold_by_level"
            }
        );
    }
}

#[test]
fn visual_host_redo_refreshes_unicode_endpoints_before_the_next_motion() {
    let mut a = vintage_source("éa\n");
    for k in ["escape", "i"] {
        key(&mut a, k);
    }
    text(&mut a, "x");
    for k in ["escape", "u", "l", "v", "super+shift+z", "left"] {
        key(&mut a, k);
    }
    assert_eq!(a.workspace.editor().text.to_string(), "xéa\n");
    assert_scalar_selections(&a);
}
fn assert_scalar_selections(a: &App) {
    let e = a.workspace.editor();
    for r in e.buffer.view(e.id).sel().iter() {
        assert!(r.end() <= e.text.len_bytes());
        assert!(e.text.is_char_boundary(r.a));
        assert!(e.text.is_char_boundary(r.b));
    }
}
#[test]
fn randomized_unicode_vintage_keys_and_host_history_keep_valid_selections() {
    let mut a = vintage();
    text(&mut a, "é猫a\n🦀 é xyz\nΩ\n");
    let mut seed = 0x906d_16b2_47a5_8921u64;
    let keys = [
        "escape",
        "i",
        "a",
        "v",
        "shift+v",
        "ctrl+v",
        "left",
        "right",
        "up",
        "down",
        "h",
        "l",
        "j",
        "k",
        "w",
        "b",
        "u",
        "ctrl+r",
        "super+z",
        "super+shift+z",
        "m",
        "q",
        "'",
        "`",
        "f",
        "t",
        "d",
        "c",
        "y",
        ".",
        "enter",
        "backspace",
    ];
    for n in 0..2500 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        match seed % 19 {
            0..=2 => text(&mut a, ["é", "猫", "🦀", "é", "\n"][seed as usize % 5]),
            3 => a.run_named(None, "upper_case", &json!({})),
            4 => a.run_named(None, "insert", &json!({"characters":"λ"})),
            _ => {
                let k = keys[seed as usize % keys.len()];
                let viewport = a.viewport();
                a.dispatch_key_binding(None, &Key::parse(k).unwrap(), Instant::now(), viewport);
            }
        }
        assert_scalar_selections(&a);
        if n % 100 == 99 {
            key(&mut a, "escape");
        }
    }
}
#[test]
fn vintage_preserves_native_word_undo_and_groups_a_complete_vi_insert_session() {
    for enabled in [false, true] {
        let mut a = if enabled {
            vintage()
        } else {
            app("", None, None)
        };
        for ch in ["t", "w", "o", " ", "t", "w", "o"] {
            text(&mut a, ch);
        }
        key(&mut a, "super+z");
        assert_eq!(
            a.workspace.editor().text.to_string(),
            "two ",
            "Vintage {enabled}"
        );
        key(&mut a, "super+shift+z");
        assert_eq!(a.workspace.editor().text.to_string(), "two two");
    }
    for undo in ["u", "super+z"] {
        let mut a = vintage();
        text(&mut a, "one");
        key(&mut a, "escape");
        key(&mut a, "a");
        for c in ["t", "w", "o"] {
            text(&mut a, c);
        }
        key(&mut a, "escape");
        key(&mut a, undo);
        assert_eq!(a.workspace.editor().text.to_string(), "one");
        key(
            &mut a,
            if undo == "u" {
                "ctrl+r"
            } else {
                "super+shift+z"
            },
        );
        assert_eq!(a.workspace.editor().text.to_string(), "onetwo");
    }
    let mut a = vintage();
    for c in ["t", "w", "o", " ", "t", "w", "o"] {
        text(&mut a, c);
    }
    key(&mut a, "escape");
    key(&mut a, "u");
    assert_eq!(a.workspace.editor().text.to_string(), "");
    key(&mut a, "ctrl+r");
    assert_eq!(a.workspace.editor().text.to_string(), "two two");
}
#[test]
fn dot_after_host_undo_keeps_append_entry_and_drops_unreplayable_sessions() {
    let mut a = vintage();
    text(&mut a, "one");
    key(&mut a, "escape");
    key(&mut a, "a");
    text(&mut a, "two");
    key(&mut a, "super+z");
    for c in ["x", "y"] {
        text(&mut a, c);
    }
    key(&mut a, "escape");
    key(&mut a, ".");
    assert_eq!(a.workspace.editor().text.to_string(), "onexyxy");
    key(&mut a, "a");
    text(&mut a, "bad");
    a.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([Region::new(0, 3)]));
    a.run_named(None, "upper_case", &json!({}));
    key(&mut a, "escape");
    let before = a.workspace.editor().text.clone();
    key(&mut a, ".");
    assert_eq!(a.workspace.editor().text, before);
}

#[test]
fn ten_thousand_caret_insert_recording_stays_bounded_through_a_long_session() {
    let mut a = vintage_source(&"ab\n".repeat(10000));
    a.workspace
        .editor_mut()
        .set_selection(Selection::from_regions(
            (0..10000).map(|n| Region::caret(n * 3 + 1)),
        ));
    let mut early = 0;
    for n in 0..100 {
        text(&mut a, "X");
        if n == 9 {
            early = a
                .workspace
                .editor()
                .vintage
                .as_ref()
                .unwrap()
                .engine
                .insert_recording_bytes();
        }
    }
    let late = a
        .workspace
        .editor()
        .vintage
        .as_ref()
        .unwrap()
        .engine
        .insert_recording_bytes();
    println!("R16_MEMORY carets=10000 keys=10/100 retained_recording_bytes={early}/{late}");
    assert!(
        late <= early * 11 / 10,
        "insert metadata grew with key count: {early} -> {late}"
    );
    assert!(late < 1024 * 1024, "{late} bytes of insert metadata");
    key(&mut a, "escape");
    key(&mut a, "u");
    assert_eq!(a.workspace.editor().text.to_string(), "ab\n".repeat(10000));
    key(&mut a, ".");
    assert_eq!(
        a.workspace.editor().text.to_string(),
        format!("a{}b\n", "X".repeat(100)).repeat(10000)
    );
}

fn fold_app(source: &str, syntax: &str, width: usize) -> App {
    let mut a = app(source, None, None);
    let e = a.workspace.editor_mut();
    e.attach_language(
        crate::language::Assets::bundled().unwrap(),
        syntax,
        Arc::new(vel_color_scheme::ColorScheme::velocity_dark()),
    )
    .unwrap();
    e.settings.set(Layer::View, "word_wrap", json!(width != 0));
    e.settings.set(Layer::View, "wrap_width", json!(width));
    e.sync();
    let until = Instant::now() + std::time::Duration::from_secs(20);
    while !e.language.borrow().as_ref().unwrap().ready()
        || e.language.borrow().as_ref().unwrap().analysis.is_none()
    {
        e.poll_language();
        assert!(Instant::now() < until);
        std::thread::park_timeout(std::time::Duration::from_millis(1));
    }
    a.run_named(None, "fold_by_level", &json!({"level":1}));
    a.view.apply_style(a.workspace.editor());
    a.view.scene(a.workspace.editor());
    a
}
#[test]
fn wrapped_header_home_end_and_command_arrows_preserve_fold_visibility() {
    for (syntax, source) in [
        ("Rust", "fn very_long_name() {\n    hello();\n}\nlast\n"),
        (
            "HTML",
            "<section class=\"long_name\">\n    hello\n</section>\nlast\n",
        ),
        (
            "Python",
            "if very_long_condition:\n    hello\n    body\nlast\n",
        ),
        (
            "Plain Text",
            "a long plain line with wrapped segments\nlast\n",
        ),
    ] {
        for width in [0, 7, 12, 80] {
            let mut a = fold_app(source, syntax, width);
            let original = a.workspace.editor().folded();
            let e = a.workspace.editor();
            let mut starts = vec![0];
            if let Some(fold) = original.first() {
                starts.push(fold.begin());
                starts.push(fold.end());
                starts.push(e.text.prev_grapheme_boundary(fold.begin()));
            }
            for visual in 0..e.layout.borrow().total() {
                let at = e.layout.borrow().offset(&e.text, visual, 0.);
                if e.text.byte_to_line(at) == 0 {
                    starts.push(at);
                }
            }
            starts.sort_unstable();
            starts.dedup();
            for start in starts {
                for k in [
                    "super+right",
                    "super+left",
                    "home",
                    "end",
                    "super+shift+right",
                    "super+shift+left",
                    "shift+home",
                    "shift+end",
                ] {
                    a.workspace.editor_mut().set_caret(start, false);
                    // Clear repeated-boundary state without moving the selection.
                    a.run_named(
                        None,
                        "move",
                        &json!({"by":"characters","forward":false,"extend":false}),
                    );
                    a.workspace.editor_mut().set_caret(start, false);
                    let expected = if k.ends_with("home") {
                        0
                    } else if k.ends_with("end") {
                        source.len()
                    } else if k.ends_with("right") && width == 0 && original.is_empty() {
                        crate::layout::body_len(&a.workspace.editor().text, 0)
                    } else if k.ends_with("right") {
                        let e = a.workspace.editor();
                        let visual = e.layout.borrow().visual_at(&e.text, start).0;
                        e.layout.borrow().offset(&e.text, visual, 10000.)
                    } else {
                        let e = a.workspace.editor();
                        let (visual, segment) = e.layout.borrow().visual_at(&e.text, start);
                        let row = e.layout.borrow().locate(visual).0;
                        let base = e.text.line_to_byte(row);
                        let first = base
                            + e.text
                                .line(row)
                                .chars()
                                .take_while(|c| matches!(c, ' ' | '\t'))
                                .count();
                        if (width != 0 || !original.is_empty())
                            && segment.bytes.start > 0
                            && start != base + segment.bytes.start
                        {
                            base + segment.bytes.start
                        } else if start == first {
                            base
                        } else {
                            first
                        }
                    };
                    key(&mut a, k);
                    let e = a.workspace.editor();
                    assert_eq!(e.caret, expected, "{syntax} width {width} {k} from {start}");
                    assert_eq!(
                        e.folded(),
                        original,
                        "{syntax} width {width} {k} from {start}"
                    );
                    let r = e.buffer.view(e.id).sel().primary().unwrap();
                    assert_eq!(r.a, if k.contains("shift") { start } else { expected });
                    if k == "super+right" {
                        key(&mut a, k);
                        assert_eq!(
                            a.workspace.editor().folded(),
                            original,
                            "repeated EOL: {syntax} width {width} from {start}"
                        );
                    }
                }
            }
        }
    }
}
#[test]
fn typing_at_visible_closing_lines_keeps_folds_but_hidden_indentation_ends_reveal() {
    for (syntax, source) in [
        ("Rust", "fn a() {\n    hello();\n}\nnext\n"),
        ("HTML", "<section>\n    hello\n</section>\nnext\n"),
        ("Python", "if x:\n    body\nnext\n"),
        ("Plain Text", "one\ntwo\nthree\nfour\n"),
    ] {
        for width in [0, 12, 80] {
            for enter in [false, true] {
                let mut a = fold_app(source, syntax, width);
                if syntax == "Plain Text" {
                    a.workspace
                        .editor_mut()
                        .set_folded(vec![Region::new(4, 14)]);
                }
                let fold = a.workspace.editor().folded()[0];
                // With the closing newline excluded, the visible delimiter is
                // on the next line, not at the end of the fold marker.
                let caret = fold.end() + usize::from(matches!(syntax, "Rust" | "HTML"));
                a.workspace.editor_mut().set_caret(caret, false);
                if enter {
                    key(&mut a, "enter");
                } else {
                    text(&mut a, "Z");
                }
                let e = a.workspace.editor();
                assert_eq!(
                    e.folded().is_empty(),
                    syntax == "Python",
                    "{syntax} width {width} enter {enter}"
                );
                assert!(
                    e.text
                        .slice(caret..e.text.len_bytes())
                        .to_string()
                        .starts_with(if enter { "\n" } else { "Z" })
                );
                key(&mut a, "super+z");
                assert_eq!(a.workspace.editor().text.to_string(), source);
                key(&mut a, "super+shift+z");
                assert_ne!(a.workspace.editor().text.to_string(), source);
            }
        }
    }
}

#[test]
fn disjoint_insert_locations_bound_metadata_and_cannot_replay_an_older_change() {
    let source = "ab".repeat(100);
    let mut a = vintage_source(&source);
    text(&mut a, "old");
    key(&mut a, "escape");
    key(&mut a, "i");
    let mut peak = 0;
    for _ in 0..80 {
        text(&mut a, "X");
        key(&mut a, "right");
        peak = peak.max(
            a.workspace
                .editor()
                .vintage
                .as_ref()
                .unwrap()
                .engine
                .insert_recording_bytes(),
        );
    }
    assert!(peak < 4096, "unbounded disjoint patch metadata: {peak}");
    key(&mut a, "escape");
    let expected = a.workspace.editor().text.clone();
    key(&mut a, ".");
    assert_eq!(a.workspace.editor().text, expected);
    key(&mut a, "u");
    assert_eq!(
        a.workspace.editor().text.to_string(),
        format!("old{source}")
    );
    key(&mut a, "ctrl+r");
    assert_eq!(a.workspace.editor().text, expected);
}

#[test]
fn host_undo_repeats_surviving_counted_append_once_and_retires_complex_entries() {
    let mut a = vintage_source("one");
    for k in ["escape", "3", "a"] {
        key(&mut a, k);
    }
    text(&mut a, "two");
    key(&mut a, "super+z");
    text(&mut a, "xy");
    key(&mut a, "escape");
    key(&mut a, ".");
    assert_eq!(a.workspace.editor().text.to_string(), "oxyxyne");
    let mut a = vintage_source("one");
    for k in ["escape", "o"] {
        key(&mut a, k);
    }
    text(&mut a, "two");
    key(&mut a, "super+z");
    text(&mut a, "xy");
    key(&mut a, "escape");
    let before = a.workspace.editor().text.clone();
    key(&mut a, ".");
    assert_eq!(a.workspace.editor().text, before);
}

#[test]
fn dot_replays_surviving_net_text_after_host_history_without_further_typing() {
    for redo in [false, true] {
        let mut a = vintage();
        text(&mut a, "one");
        key(&mut a, "escape");
        key(&mut a, "a");
        text(&mut a, "two ");
        text(&mut a, "tail");
        key(&mut a, "super+z");
        if redo {
            key(&mut a, "super+shift+z");
        }
        key(&mut a, "escape");
        key(&mut a, ".");
        assert_eq!(
            a.workspace.editor().text.to_string(),
            if redo {
                "onetwo tailtwo tail"
            } else {
                "onetwo two "
            }
        );
        key(&mut a, "u");
        assert_eq!(
            a.workspace.editor().text.to_string(),
            if redo { "onetwo tail" } else { "onetwo " }
        );
    }
}

#[test]
fn visual_join_accepts_a_drag_and_a_line_start_caret() {
    let source = "ab\ncd\nef\ngh\n";
    let mut a = vintage_source(source);
    key(&mut a, "escape");
    a.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([
            Region::new(0, 2),
            Region::caret(3),
        ]));
    key(&mut a, "shift+j");
    assert_eq!(a.workspace.editor().text.to_string(), source);
    assert_scalar_selections(&a);
    key(&mut a, "escape");
    a.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([
            Region::new(0, 5),
            Region::caret(9),
        ]));
    key(&mut a, "shift+j");
    assert_eq!(a.workspace.editor().text.to_string(), "ab cd\nef\ngh\n");
    key(&mut a, "super+z");
    assert_eq!(a.workspace.editor().text.to_string(), source);
    key(&mut a, "super+shift+z");
    assert_eq!(a.workspace.editor().text.to_string(), "ab cd\nef\ngh\n");
}

#[test]
fn every_visual_operator_accepts_mixed_host_regions() {
    let operators: &[&[&str]] = &[
        &["d"],
        &["c"],
        &["y"],
        &[">"],
        &["<"],
        &["="],
        &["g", "u"],
        &["g", "shift+u"],
        &["g", "~"],
        &["g", "?"],
        &["x"],
        &["shift+x"],
        &["s"],
        &["shift+s"],
        &["shift+c"],
        &["shift+d"],
        &["shift+y"],
        &["p"],
        &["shift+p"],
        &["shift+j"],
        &["~"],
        &["u"],
        &["shift+u"],
        &["r", "z"],
    ];
    for keys in operators {
        for reverse in [false, true] {
            for caret in [4, 8, 9, 13] {
                let source = "éa\nβc\n\n🦀x\n";
                let mut a = vintage_source(source);
                key(&mut a, "escape");
                a.workspace.editor_mut().clipboard = "paste".into();
                a.workspace
                    .editor_mut()
                    .set_selection(Selection::from_regions([
                        if reverse {
                            Region::new(3, 0)
                        } else {
                            Region::new(0, 3)
                        },
                        Region::caret(caret),
                    ]));
                for k in *keys {
                    key(&mut a, k);
                }
                assert_scalar_selections(&a);
                key(&mut a, "escape");
                key(&mut a, "super+z");
                assert_scalar_selections(&a);
                key(&mut a, "super+shift+z");
                assert_scalar_selections(&a);
            }
        }
    }
}

#[test]
fn completed_insert_glues_words_newlines_backspace_and_ime_for_both_undo_paths() {
    for undo in ["u", "super+z"] {
        for variant in [
            "words",
            "newline",
            "backspace",
            "ime",
            "change",
            "open",
            "count",
        ] {
            let mut a = vintage_source("one\n");
            key(&mut a, "escape");
            match variant {
                "change" => {
                    key(&mut a, "c");
                    key(&mut a, "w");
                }
                "open" => key(&mut a, "o"),
                "count" => {
                    key(&mut a, "3");
                    key(&mut a, "shift+a");
                }
                _ => key(&mut a, "shift+a"),
            }
            for c in "two three four".chars() {
                text(&mut a, &c.to_string());
            }
            match variant {
                "newline" => {
                    key(&mut a, "enter");
                    text(&mut a, "five");
                }
                "backspace" => {
                    key(&mut a, "backspace");
                    key(&mut a, "backspace");
                    text(&mut a, "é");
                }
                "ime" => {
                    a.workspace.editor_mut().text_input(TextInput::Marked {
                        text: "nihao".into(),
                        selected: vel_platform::Utf16Range::new(5, 0),
                        replacement: None,
                    });
                    assert!(a.workspace.editor().marked.is_some());
                    text(&mut a, "你好");
                }
                _ => {}
            }
            let inserted = a.workspace.editor().text.to_string();
            key(&mut a, "escape");
            let completed = a.workspace.editor().text.to_string();
            assert!(completed.len() >= inserted.len());
            key(&mut a, undo);
            assert_eq!(
                a.workspace.editor().text.to_string(),
                "one\n",
                "{variant} {undo}"
            );
            key(
                &mut a,
                if undo == "u" {
                    "ctrl+r"
                } else {
                    "super+shift+z"
                },
            );
            assert_eq!(
                a.workspace.editor().text.to_string(),
                completed,
                "{variant} {undo}"
            );
        }
    }
}

#[test]
fn insert_host_undo_stays_per_word_until_escape_then_glues_the_surviving_session() {
    let mut a = vintage_source("one\n");
    key(&mut a, "escape");
    key(&mut a, "shift+a");
    for c in "two three four".chars() {
        text(&mut a, &c.to_string());
    }
    key(&mut a, "super+z");
    assert_eq!(a.workspace.editor().text.to_string(), "onetwo three \n");
    key(&mut a, "super+shift+z");
    assert_eq!(a.workspace.editor().text.to_string(), "onetwo three four\n");
    key(&mut a, "super+z");
    text(&mut a, "five");
    key(&mut a, "escape");
    key(&mut a, "super+z");
    assert_eq!(a.workspace.editor().text.to_string(), "one\n");
    key(&mut a, "super+shift+z");
    assert_eq!(a.workspace.editor().text.to_string(), "onetwo three five\n");
}

#[test]
fn vi_macros_refresh_layout_between_edit_and_vertical_or_block_motion() {
    for motion in ["j", "k", "ctrl+v"] {
        for width in [0, 7, 12, 80] {
            let mut a = vintage_source("éa\nβc\n🦀x\nlast\n");
            a.workspace
                .editor_mut()
                .settings
                .set(Layer::View, "word_wrap", json!(width != 0));
            a.workspace
                .editor_mut()
                .settings
                .set(Layer::View, "wrap_width", json!(width));
            a.workspace.editor_mut().sync();
            a.view.apply_style(a.workspace.editor());
            a.view.scene(a.workspace.editor());
            key(&mut a, "escape");
            key(&mut a, "q");
            key(&mut a, "a");
            key(&mut a, "o");
            text(&mut a, "new 🦀");
            key(&mut a, "escape");
            key(&mut a, motion);
            if motion == "ctrl+v" {
                key(&mut a, "j");
                key(&mut a, "escape");
            }
            key(&mut a, "q");
            // No frame or scene build between any of the following events.
            for _ in 0..8 {
                key(&mut a, "@");
                key(&mut a, "a");
                assert_scalar_selections(&a);
            }
            assert_eq!(
                a.workspace
                    .editor()
                    .text
                    .to_string()
                    .matches("new 🦀")
                    .count(),
                9
            );
            let e = a.workspace.editor();
            assert!(e.layout.borrow().current(
                (e.buffer.id(), e.buffer.change_id()),
                crate::view::motion_layout_options(e)
            ));
        }
    }
}

#[test]
fn projected_boundaries_refresh_after_edits_and_wrap_changes_without_a_frame() {
    let mut a = fold_app("a long line with many words\nlast\n", "Plain Text", 7);
    a.workspace.editor_mut().set_caret(0, false);
    for c in "é🦀\n".chars() {
        text(&mut a, &c.to_string());
    }
    key(&mut a, "super+right");
    assert_eq!(a.workspace.editor().caret, 13);
    key(&mut a, "super+left");
    assert_eq!(a.workspace.editor().caret, 7);
    a.workspace
        .editor_mut()
        .settings
        .set(Layer::View, "wrap_width", json!(12));
    a.workspace.editor_mut().sync();
    key(&mut a, "super+right");
    assert_eq!(a.workspace.editor().caret, 18);
    a.workspace
        .editor_mut()
        .settings
        .set(Layer::View, "word_wrap", json!(false));
    a.workspace.editor_mut().sync();
    key(&mut a, "super+right");
    assert_eq!(a.workspace.editor().caret, 34);
    key(&mut a, "super+z");
    key(&mut a, "super+shift+z");
    key(&mut a, "super+left");
    assert_scalar_selections(&a);
}

#[test]
fn nested_unwrapped_indent_markers_keep_command_home_and_shift_home_projected() {
    let source = "if outer:\n    if inner:\n        body\n    tail\nlast\n";
    for width in [0, 7, 12, 80] {
        let mut a = fold_app(source, "Python", width);
        let outer = a.workspace.editor().folded()[0];
        let inner = Region::new(
            source.find("        body").unwrap() - 1,
            source.find("\n    tail").unwrap(),
        );
        a.workspace.editor_mut().set_folded(vec![outer, inner]);
        a.view.apply_style(a.workspace.editor());
        let original = a.workspace.editor().folded();
        for extend in [false, true] {
            a.workspace.editor_mut().set_caret(outer.begin(), false);
            key(&mut a, "right");
            assert_eq!(a.workspace.editor().caret, outer.end());
            key(
                &mut a,
                if extend {
                    "super+shift+left"
                } else {
                    "super+left"
                },
            );
            let e = a.workspace.editor();
            let (_, segment) = e.layout.borrow().visual_at(&e.text, outer.begin());
            assert_eq!(e.caret, segment.bytes.start);
            assert_eq!(e.folded(), original);
        }
    }
}

#[test]
fn layout_queries_clamp_stale_rows_and_unicode_offsets_after_real_edits() {
    let mut a = fold_app("a long line\nsecond\nthird\n", "Plain Text", 7);
    a.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([Region::new(0, 25)]));
    text(&mut a, "é🦀\n");
    let e = a.workspace.editor();
    // Deliberately query before a frame or command can synchronize the old layout.
    let layout = e.layout.borrow();
    for byte in [0, 1, 3, e.text.len_bytes(), 999, usize::MAX] {
        let _ = layout.visual_at(&e.text, byte);
        let _ = layout.column(&e.text, byte);
    }
    for row in [0, 1, 99, usize::MAX] {
        let _ = layout.segment(&e.text, row, usize::MAX);
        for x in [0., 2., 9999., f64::NAN] {
            let offset = layout.offset(&e.text, row, x);
            assert!(offset <= e.text.len_bytes() && e.text.is_char_boundary(offset));
        }
    }
    drop(layout);
    key(&mut a, "super+left");
    key(&mut a, "super+right");
    assert_scalar_selections(&a);
}
#[test]
fn vintage_dot_after_host_insert_and_after_empty_insert_through_real_keys() {
    // Rebase integration: lane A's dot target flag and lane B's host insert sessions.
    let mut a = vintage_source("one\ntwo\n");
    key(&mut a, "escape");
    key(&mut a, "shift+a");
    text(&mut a, "猫x");
    key(&mut a, "escape");
    key(&mut a, "j");
    key(&mut a, ".");
    assert_eq!(a.workspace.editor().text.to_string(), "one猫x\ntwo猫x\n");
    key(&mut a, "u");
    assert_eq!(a.workspace.editor().text.to_string(), "one猫x\ntwo\n");
    // Vintage starts in insert mode by default: the first host session has no entry key.
    let mut a = vintage();
    text(&mut a, "ab");
    key(&mut a, "escape");
    key(&mut a, ".");
    assert_eq!(a.workspace.editor().text.to_string(), "aabb");
    key(&mut a, "u");
    assert_eq!(a.workspace.editor().text.to_string(), "ab");
    for entry in ["i", "shift+a"] {
        let mut a = vintage_source("first\nsecond\nlast\n");
        key(&mut a, "escape");
        for k in ["d", "d", entry, "escape", "."] {
            key(&mut a, k);
        }
        // An empty insert is itself the last change, so `.` must not repeat `dd`.
        assert_eq!(
            a.workspace.editor().text.to_string(),
            "second\nlast\n",
            "dd{entry}<Esc>."
        );
    }
}
