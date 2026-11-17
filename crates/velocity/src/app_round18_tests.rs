//! Round 18 follows the shared native/headless command, input and resource paths.
use super::*;
use vel_buffer::{Region, Selection};
use vel_config::keymap::Key;
use vel_text::Text;

fn app(text: &str, syntax: &str, profile: Option<&Path>) -> App {
    let mut workspace = Workspace::new(
        Text::from(text),
        profile.map(Path::to_path_buf),
        None,
        false,
    )
    .unwrap();
    workspace.set_file_type(syntax).unwrap();
    let options = Options::default();
    let mut app = App::new(
        options.clone(),
        Clock::new(Instant::now(), std::time::SystemTime::now()),
        Reporter::for_options(&options, Instant::now()).unwrap(),
        workspace,
        Arc::new(AtomicBool::new(false)),
        std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(
            profile,
        ))),
    );
    app.panels.side_visible = false;
    app.sync_active_view();
    app.workspace.editor_mut().set_caret(text.len(), false);
    app
}
fn named(a: &mut App, name: &str, args: Value) {
    a.run_named(None, name, &args);
}
fn set(a: &mut App, name: &str, value: Value) {
    named(a, "set_setting", json!({"setting":name,"value":value}));
}
fn key(a: &mut App, name: &str) {
    let viewport = a.viewport();
    assert!(
        a.dispatch_key_binding(None, &Key::parse(name).unwrap(), Instant::now(), viewport),
        "{name}"
    );
}
fn input(a: &mut App, text: &str) {
    let event = TextInput::Insert {
        text: text.into(),
        replacement: None,
    };
    let viewport = a.viewport();
    if !a
        .panels
        .text_input(&mut a.workspace, event.clone(), viewport)
    {
        a.workspace.editor_mut().text_input(event);
        a.handle_named_actions(None).unwrap();
    }
}
fn until(a: &mut App, predicate: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !predicate(a) {
        a.workspace.editor_mut().poll_language();
        a.workspace.editor_mut().poll_completion();
        assert!(
            Instant::now() < deadline,
            "timeout: {:?}",
            a.workspace.editor().completion
        );
        std::thread::park_timeout(Duration::from_millis(1));
    }
}
fn engine(a: &mut App, words: &[&str]) {
    let mut engine = vel_complete::CompletionEngine::new();
    engine
        .add_completions(&json!({"scope":"source.rust","completions":words}).to_string())
        .unwrap();
    a.workspace
        .editor_mut()
        .completion
        .set_engine(Arc::new(engine));
}
fn snippets(a: &mut App, entries: &[(&str, &str, &str)]) {
    let snippets=entries.iter().enumerate().map(|(i,(trigger,content,scope))| {
        (format!("Packages/User/test{i}.sublime-snippet"),vel_snippet::SnippetFile::parse(&format!("<snippet><content><![CDATA[{content}]]></content><tabTrigger>{trigger}</tabTrigger><scope>{scope}</scope><description>Choice {i}</description></snippet>")).unwrap())
    }).collect();
    a.workspace.editor_mut().snippets = Arc::new(snippets);
}
#[test]
fn fresh_bundled_startup_has_no_modal_or_status_diagnostics() {
    let profile = tempfile::tempdir().unwrap();
    for _ in 0..2 {
        let a = app("", "Plain Text", Some(profile.path()));
        assert!(
            a.workspace.messages.is_empty(),
            "{:?}",
            a.workspace.messages
        );
        assert!(a.workspace.resource_reports.is_empty());
        assert!(a.panels.message.is_empty(), "{}", a.panels.message);
    }
}
#[test]
fn bracket_indent_both_values_and_live_syntax_override() {
    let profile = tempfile::tempdir().unwrap();
    let user = profile.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("Rust.sublime-settings"),
        r#"{"indent_to_bracket":true,"translate_tabs_to_spaces":true}"#,
    )
    .unwrap();
    let mut a = app("call(one,", "Rust", Some(profile.path()));
    key(&mut a, "enter");
    assert_eq!(a.workspace.editor().text.to_string(), "call(one,\n     ");
    named(&mut a, "undo", json!({}));
    set(&mut a, "indent_to_bracket", json!(false));
    key(&mut a, "enter");
    assert_eq!(a.workspace.editor().text.to_string(), "call(one,\n");
    // Inputs retain their own text/focus: changing the document setting cannot indent Find.
    named(
        &mut a,
        "show_panel",
        json!({"panel":"find","text":"needle"}),
    );
    set(&mut a, "indent_to_bracket", json!(true));
    let before = a.workspace.editor().text.to_string();
    input(&mut a, "query");
    assert_eq!(a.workspace.editor().text.to_string(), before);
    assert_eq!(a.panels.query.value(), "query");
}
#[test]
fn up_down_document_limit_table_including_extend_and_input_focus() {
    for enabled in [false, true] {
        for (key_name, point, expected) in [
            ("up", 2, if enabled { 0 } else { 2 }),
            ("down", 8, if enabled { 10 } else { 8 }),
        ] {
            let mut a = app("abcd\nefghi", "Plain Text", None);
            set(&mut a, "move_to_limit_on_up_down", json!(enabled));
            a.workspace.editor_mut().set_caret(point, false);
            key(&mut a, key_name);
            assert_eq!(a.workspace.editor().caret, expected, "{enabled}/{key_name}");
        }
    }
    let mut a = app("abcd", "Plain Text", None);
    set(&mut a, "move_to_limit_on_up_down", json!(true));
    a.workspace.editor_mut().set_caret(2, false);
    key(&mut a, "shift+up");
    assert_eq!(a.workspace.editor().selection(), 0..2);
    named(
        &mut a,
        "show_panel",
        json!({"panel":"find","text":"needle"}),
    );
    let before = a.workspace.editor().regions();
    key(&mut a, "up");
    assert_eq!(a.workspace.editor().regions(), before);
}
#[test]
fn close_tags_table_cold_stale_markup_void_quotes_comments_and_xml_case() {
    for enabled in [false, true] {
        for (syntax, source, expected) in [
            ("HTML", "<main><section><", "<main><section></section>"),
            ("HTML", "<main><img><", "<main><img></main>"),
            ("HTML", "<MAIN><Item></item><", "<MAIN><Item></item></MAIN>"),
            (
                "HTML",
                "<main><item x=\">\"></item><",
                "<main><item x=\">\"></item></main>",
            ),
            ("XML", "<Root><Child/><", "<Root><Child/></Root>"),
            ("HTML", "<main><!-- <fake><", "<main><!-- <fake></"),
            (
                "HTML",
                "<script>const s = '<';<",
                "<script>const s = '<';</",
            ),
            ("Plain Text", "<main><", "<main></"),
        ] {
            let mut a = app(source, syntax, None);
            set(&mut a, "auto_close_tags", json!(enabled));
            input(&mut a, "/");
            assert_eq!(
                a.workspace.editor().text.to_string(),
                if enabled {
                    expected.to_owned()
                } else {
                    format!("{source}/")
                },
                "{syntax}: {source}"
            );
        }
    }
    let mut a = app("<main><", "Plain Text", None);
    until(&mut a, |a| {
        a.workspace
            .editor()
            .language
            .borrow()
            .as_ref()
            .is_some_and(|l| l.ready())
    });
    a.workspace.set_file_type("HTML").unwrap(); // prior Plain Text publication must not be trusted
    input(&mut a, "/");
    assert_eq!(a.workspace.editor().text.to_string(), "<main></main>");
    named(&mut a, "undo", json!({}));
    assert_eq!(a.workspace.editor().text.to_string(), "<main><");
}
#[test]
fn tab_best_match_decision_table_and_snippet_field_selection_indent_exceptions() {
    for (words, popup, result) in [
        (vec!["alpha"], false, "alpha"),
        (vec!["alpha", "alpine"], false, "alpha"),
        (vec![], false, "al\t"),
    ] {
        let mut a = app("al", "Rust", None);
        engine(&mut a, &words);
        key(&mut a, "tab");
        until(&mut a, |a| !a.workspace.editor().completion.busy());
        assert_eq!(a.workspace.editor().completion.visible(), popup);
        assert_eq!(a.workspace.editor().text.to_string(), result);
        if popup {
            key(&mut a, "tab");
            assert_eq!(a.workspace.editor().text.to_string(), "alpha");
        }
    }
    for source in ["al", "    "] {
        let mut a = app(source, "Rust", None);
        engine(&mut a, &["alpha", "alpine"]);
        set(&mut a, "tab_completion", json!(false));
        key(&mut a, "tab");
        assert!(!a.workspace.editor().completion.busy());
        assert!(!a.workspace.editor().completion.visible());
    }
    let mut a = app("al", "Rust", None);
    engine(&mut a, &["alpha", "alpine"]);
    a.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([Region::new(0, 2)]));
    key(&mut a, "tab");
    assert!(!a.workspace.editor().completion.busy());
    named(
        &mut a,
        "insert_snippet",
        json!({"contents":"${1:al} ${2:second}$0"}),
    );
    key(&mut a, "tab");
    assert_eq!(
        a.workspace
            .editor()
            .text
            .slice(a.workspace.editor().selection())
            .to_string(),
        "second"
    );
}
#[test]
fn suffix_trigger_longest_and_duplicate_choice_through_key_dispatch() {
    for trigger in ["t.", "if?", "eac-", ":"] {
        let mut a = app(&format!("prefix {trigger}"), "Rust", None);
        snippets(
            &mut a,
            &[
                (trigger, "expanded", "source.rust"),
                (".", "short", "source.rust"),
            ],
        );
        key(&mut a, "tab");
        assert_eq!(
            a.workspace.editor().text.to_string(),
            "prefix expanded",
            "{trigger}"
        );
    }
    let mut a = app("t.", "Rust", None);
    snippets(
        &mut a,
        &[
            ("t.", "one", "source.rust"),
            ("t.", "two", "source.rust"),
            ("t.", "hidden", "text.html"),
        ],
    );
    key(&mut a, "tab");
    assert_eq!(a.panels.overlay, Some(crate::panels::Overlay::Choices));
    assert_eq!(a.workspace.editor().text.to_string(), "t.");
    let deadline = Instant::now() + Duration::from_secs(10);
    while a.panels.quick.items.len() != 2 {
        let viewport = a.viewport();
        a.panels.poll(&mut a.workspace, viewport);
        assert!(Instant::now() < deadline);
        std::thread::park_timeout(Duration::from_millis(1));
    }
    key(&mut a, "down");
    key(&mut a, "enter");
    assert_eq!(a.workspace.editor().text.to_string(), "two");
}
#[test]
fn snippet_environment_uses_system_name_and_document_base_scope() {
    let mut a = app("", "Rust", None);
    named(
        &mut a,
        "insert_snippet",
        json!({"contents":"${TM_FULLNAME:missing}|$TM_SCOPE"}),
    );
    let value = a.workspace.editor().text.to_string();
    assert!(value.ends_with("|source.rust"), "{value}");
    assert!(!value.starts_with("missing|"));
}
#[test]
fn triggers_run_on_text_input_and_panel_input_stays_isolated() {
    for rhs_empty in [false, true] {
        for rhs in ["", "tail", " tail", "\ttail"] {
            let mut a = app(&format!("object{rhs}"), "Rust", None);
            a.workspace.editor_mut().set_caret(6, false);
            engine(&mut a, &["member"]);
            set(&mut a, "auto_complete_selector", json!("text.never"));
            set(&mut a, "auto_complete_delay", json!(0));
            set(
                &mut a,
                "auto_complete_triggers",
                json!([{"selector":"punctuation.accessor","rhs_empty":rhs_empty}]),
            );
            input(&mut a, ".");
            until(&mut a, |a| !a.workspace.editor().completion.busy());
            assert_eq!(
                a.workspace.editor().completion.visible(),
                !rhs_empty || rhs.chars().next().is_none_or(char::is_whitespace),
                "{rhs_empty}/{rhs}"
            );
        }
    }
    let mut a = app("<", "HTML", None);
    named(&mut a, "show_panel", json!({"panel":"find","text":""}));
    input(&mut a, "<");
    assert!(!a.workspace.editor().completion.busy());
    assert_eq!(a.workspace.editor().text.to_string(), "<");
}
#[test]
fn focus_lost_saving_is_opt_in_and_skips_conflicts_readonly_and_untitled() {
    let dir = tempfile::tempdir().unwrap();
    for enabled in [false, true] {
        let file = dir.path().join(format!("save-{enabled}.txt"));
        std::fs::write(&file, "disk").unwrap();
        let mut a = app("", "Plain Text", None);
        a.workspace
            .open(crate::cli::FileLocation {
                path: file.clone(),
                line: 1,
                column: 1,
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while a.workspace.tabs[a.workspace.active].loading.is_some() {
            a.workspace.poll();
            assert!(Instant::now() < deadline);
            std::thread::park_timeout(Duration::from_millis(1));
        }
        set(&mut a, "save_on_focus_lost", json!(enabled));
        input(&mut a, "edit");
        let contents = a.workspace.editor().text.to_string();
        a.sync_active_view();
        a.save_on_focus_lost(None);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            if enabled { contents } else { "disk".into() }
        );
        if enabled {
            input(&mut a, "more");
            a.workspace.editor_mut().read_only = true;
            a.save_on_focus_lost(None);
            assert!(a.workspace.tabs[a.workspace.active].dirty());
            a.workspace.editor_mut().read_only = false;
            std::fs::write(&file, "external change before watcher").unwrap();
            a.save_on_focus_lost(None);
            assert_eq!(
                std::fs::read_to_string(&file).unwrap(),
                "external change before watcher"
            );
        }
        assert!(a.workspace.messages.is_empty());
    }
    let mut a = app("untitled", "Plain Text", None);
    set(&mut a, "save_on_focus_lost", json!(true));
    input(&mut a, "!");
    a.save_on_focus_lost(None);
    assert!(a.workspace.tabs[a.workspace.active].path.is_none());
}
#[test]
fn find_panel_seeding_table_and_close_after_all_values() {
    for panel in ["find", "replace", "incremental_find"] {
        for enabled in [false, true] {
            for (selection, expected) in [
                (0..3, if enabled { "one" } else { "last" }),
                (0..7, "last"),
                (1..1, "last"),
            ] {
                let mut a = app("one\ntwo", "Plain Text", None);
                a.panels.query.set("last");
                a.workspace
                    .editor_mut()
                    .set_selection(Selection::from_regions([Region::from(selection)]));
                set(&mut a, "find_selected_text", json!(enabled));
                named(&mut a, "show_panel", json!({"panel":panel}));
                assert_eq!(a.panels.query.value(), expected, "{panel}/{enabled}");
                assert_eq!(a.workspace.editor().text.to_string(), "one\ntwo");
            }
        }
    }
    for enabled in [false, true] {
        let mut a = app("one\ntwo", "Plain Text", None);
        a.workspace
            .editor_mut()
            .set_selection(Selection::from_regions([Region::new(0, 7)]));
        set(&mut a, "auto_find_in_selection", json!(enabled));
        named(&mut a, "show_panel", json!({"panel":"find","text":"one"}));
        assert_eq!(a.panels.in_selection, enabled);
    }
    for close in [false, true] {
        for command in ["find_all", "replace_all"] {
            let mut a = app("one one", "Plain Text", None);
            set(&mut a, &format!("close_find_after_{command}"), json!(close));
            named(
                &mut a,
                "show_panel",
                json!({"panel":if command=="find_all" {"find"} else {"replace"},"text":"one"}),
            );
            a.panels.replace.set("two");
            named(&mut a, command, json!({}));
            assert_eq!(a.panels.panel.is_none(), close, "{command}/{close}");
            if command == "replace_all" {
                assert_eq!(a.workspace.editor().text.to_string(), "two two");
            } else {
                assert_eq!(a.workspace.editor().regions().len(), 2);
            }
        }
    }
}
#[test]
fn scoped_snippets_are_filtered_in_the_real_palette() {
    let profile = tempfile::tempdir().unwrap();
    let user = profile.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    for (name, scope) in [("RustOnly", "source.rust"), ("HtmlOnly", "text.html")] {
        std::fs::write(user.join(format!("{name}.sublime-snippet")),format!("<snippet><content>{name}</content><scope>{scope}</scope><description>R18{name}</description></snippet>")).unwrap();
    }
    let mut a = app("", "Rust", Some(profile.path()));
    until(&mut a, |a| {
        a.workspace
            .editor()
            .language
            .borrow()
            .as_ref()
            .unwrap()
            .scopes_ready()
    });
    named(
        &mut a,
        "show_overlay",
        json!({"overlay":"command_palette","text":"R18"}),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while a.panels.quick.items.is_empty() {
        let viewport = a.viewport();
        a.panels.poll(&mut a.workspace, viewport);
        assert!(Instant::now() < deadline);
        std::thread::park_timeout(Duration::from_millis(1));
    }
    let captions: Vec<_> = a
        .panels
        .quick
        .items
        .iter()
        .map(|i| i.caption.as_str())
        .collect();
    assert!(
        captions.iter().any(|s| s.contains("RustOnly")),
        "{captions:?}"
    );
    assert!(
        !captions.iter().any(|s| s.contains("HtmlOnly")),
        "{captions:?}"
    );
}
#[test]
fn completion_suffix_settings_reach_the_commit_path() {
    for enabled in [false, true] {
        let mut a = app("alpha beta\nalpha beta\nalpha beta\nal", "Rust", None);
        a.workspace.editor_mut().set_caret(35, false);
        engine(&mut a, &[]);
        set(&mut a, "auto_complete_trailing_spaces", json!(enabled));
        named(&mut a, "auto_complete", json!({}));
        until(&mut a, |a| a.workspace.editor().completion.visible());
        key(&mut a, "tab");
        assert_eq!(
            a.workspace.editor().text.to_string(),
            if enabled {
                "alpha beta\nalpha beta\nalpha beta\nalpha "
            } else {
                "alpha beta\nalpha beta\nalpha beta\nalpha"
            }
        );
        let mut a = app("let value pri", "Rust", None);
        let mut index = vel_index::Index::new();
        index.replace_file(
            "fixture",
            vel_index::FileAnalysis {
                symbols: Vec::new(),
                words: vel_index::WordStatistics::from_text(
                    "let value println!(value)\nlet value println!(value)",
                ),
            },
        );
        let mut engine = vel_complete::CompletionEngine::new();
        engine.set_index(Arc::new(std::sync::RwLock::new(index)));
        a.workspace
            .editor_mut()
            .completion
            .set_engine(Arc::new(engine));
        set(&mut a, "auto_complete_trailing_symbols", json!(enabled));
        named(&mut a, "auto_complete", json!({}));
        until(&mut a, |a| a.workspace.editor().completion.visible());
        key(&mut a, "tab");
        assert_eq!(
            a.workspace.editor().text.to_string(),
            if enabled {
                "let value println!("
            } else {
                "let value println"
            }
        );
    }
}
#[test]
fn popup_scene_uses_kind_colors_match_color_and_kind_footer() {
    let mut a = app("al", "Rust", None);
    let mut engine = vel_complete::CompletionEngine::new();
    engine.add_completions(r#"{"scope":"source.rust","completions":[{"trigger":"alpha","contents":"alpha","kind":["function","f","Callable"],"details":"<b>detail</b>"},{"trigger":"alpine","contents":"alpine","kind":"namespace"}]}"#).unwrap();
    a.workspace
        .editor_mut()
        .completion
        .set_engine(Arc::new(engine));
    named(&mut a, "auto_complete", json!({}));
    until(&mut a, |a| a.workspace.editor().completion.visible());
    let scene = a.view.scene(a.workspace.editor());
    let runs: Vec<_> = scene.layers.iter().flat_map(|l| l.text.iter()).collect();
    let normal = runs.iter().find(|r| r.text == "alpha").unwrap().color;
    assert!(runs.iter().any(|r| r.text == "f" && r.color != normal));
    assert!(runs.iter().any(|r| r.text == "l" && r.color != normal));
    assert!(runs.iter().any(|r| r.text == "Callable"));
    assert!(runs.iter().any(|r| r.text == "detail"));
}
#[test]
fn focus_loss_tab_transition_saves_the_previous_document() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("switch.txt");
    std::fs::write(&file, "disk").unwrap();
    let mut a = app("", "Plain Text", None);
    a.workspace
        .open(crate::cli::FileLocation {
            path: file.clone(),
            line: 1,
            column: 1,
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while a.workspace.tabs[a.workspace.active].loading.is_some() {
        a.workspace.poll();
        assert!(Instant::now() < deadline);
        std::thread::park_timeout(Duration::from_millis(1));
    }
    a.sync_active_view();
    set(&mut a, "save_on_focus_lost", json!(true));
    input(&mut a, "edited");
    let expected = a.workspace.editor().text.to_string();
    named(&mut a, "new_file", json!({}));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), expected);
    assert!(a.workspace.tabs[a.workspace.active].path.is_none());
}
#[test]
#[ignore = "headless 10,000-caret timing; run explicitly with --ignored --nocapture"]
fn many_caret_input_hooks_timing() {
    let count = 10_000;
    for (syntax, line, character) in [
        ("Rust", "object\n", "x"),
        ("Rust", "object\n", "."),
        ("HTML", "<main><\n", "/"),
        ("Rust", "call(one,\n", "\n"),
        ("Rust", "alpha beta\nal\n", "p"),
    ] {
        for enabled in [false, true] {
            let source = line.repeat(count);
            let mut a = app(&source, syntax, None);
            set(&mut a, "auto_close_tags", json!(enabled));
            set(&mut a, "indent_to_bracket", json!(enabled));
            set(&mut a, "auto_complete_trailing_symbols", json!(enabled));
            set(&mut a, "auto_complete_trailing_spaces", json!(enabled));
            set(&mut a, "auto_complete_delay", json!(0));
            set(
                &mut a,
                "auto_complete_triggers",
                if enabled {
                    json!([{"selector":"punctuation.accessor","rhs_empty":true}])
                } else {
                    json!([])
                },
            );
            a.workspace
                .editor_mut()
                .set_selection(Selection::from_regions(
                    (0..count).map(|i| Region::caret(i * line.len() + line.len() - 1)),
                ));
            let begin = Instant::now();
            input(&mut a, character);
            let input_ms = begin.elapsed().as_secs_f64() * 1000.;
            assert_eq!(a.workspace.editor().regions().len(), count);
            until(&mut a, |a| {
                !a.workspace.editor().completion.busy()
                    && a.workspace.editor().pending_closing_tag.is_none()
            });
            let ready_ms = begin.elapsed().as_secs_f64() * 1000.;
            let mut commit_ms = None;
            if character == "p" {
                assert!(a.workspace.editor().completion.visible());
                let begin = Instant::now();
                named(&mut a, "commit_completion", json!({}));
                commit_ms = Some(begin.elapsed().as_secs_f64() * 1000.);
            }
            println!(
                "R18_TIMING {}",
                json!({"carets":count,"syntax":syntax,"character":character,"enabled":enabled,"input_ms":input_ms,"completion_ready_ms":ready_ms,"commit_ms":commit_ms})
            );
        }
    }
}

#[test]
fn close_tags_rejects_an_edit_stale_comment_and_handles_unicode() {
    let mut a = app("<main><", "HTML", None);
    until(&mut a, |a| {
        a.workspace
            .editor()
            .language
            .borrow()
            .as_ref()
            .is_some_and(|l| l.ready())
    });
    a.workspace.editor_mut().set_caret(0, false);
    input(&mut a, "<!--");
    let end = a.workspace.editor().text.len_bytes();
    a.workspace.editor_mut().set_caret(end, false);
    input(&mut a, "/");
    assert_eq!(a.workspace.editor().text.to_string(), "<!--<main></");
    let mut a = app("<main>é<", "HTML", None);
    input(&mut a, "/");
    assert_eq!(a.workspace.editor().text.to_string(), "<main>é</main>");
    let mut a = app("🦀", "HTML", None);
    input(&mut a, "/");
    assert_eq!(a.workspace.editor().text.to_string(), "🦀/");
}
#[test]
fn explicit_find_shortcuts_close_even_when_buttons_stay_open() {
    for (panel, setting, shortcut) in [
        ("find", "close_find_after_find_all", "alt+enter"),
        ("replace", "close_find_after_replace_all", "ctrl+alt+enter"),
    ] {
        let mut a = app("one one", "Plain Text", None);
        set(&mut a, setting, json!(false));
        named(&mut a, "show_panel", json!({"panel":panel,"text":"one"}));
        key(&mut a, shortcut);
        assert!(a.panels.panel.is_none());
    }
}
#[test]
fn selected_snippet_choice_is_rejected_after_a_document_change() {
    let mut a = app("t.", "Rust", None);
    snippets(
        &mut a,
        &[("t.", "one", "source.rust"), ("t.", "two", "source.rust")],
    );
    key(&mut a, "tab");
    assert_eq!(a.panels.overlay, Some(crate::panels::Overlay::Choices));
    // A programmatic edit can arrive while the panel owns keyboard focus.
    a.workspace
        .editor_mut()
        .run("insert", &json!({"characters":"new"}))
        .unwrap();
    let before = a.workspace.editor().text.to_string();
    key(&mut a, "enter");
    assert_eq!(a.workspace.editor().text.to_string(), before);
}

#[test]
fn focus_save_failure_is_status_only_and_preserves_disk_contents() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("error.txt");
    std::fs::write(&file, "disk").unwrap();
    let mut a = app("", "Plain Text", None);
    a.workspace
        .open(crate::cli::FileLocation {
            path: file.clone(),
            line: 1,
            column: 1,
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while a.workspace.tabs[a.workspace.active].loading.is_some() {
        a.workspace.poll();
        assert!(Instant::now() < deadline);
        std::thread::park_timeout(Duration::from_millis(1));
    }
    set(&mut a, "save_on_focus_lost", json!(true));
    input(&mut a, "edit");
    a.workspace.tabs[a.workspace.active].format.encoding = "invalid-encoding-fixture".into();
    a.save_on_focus_lost(None);
    assert!(a.panels.message.starts_with("Automatic save failed:"));
    assert!(a.workspace.messages.is_empty());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "disk");
}

#[test]
fn identifier_snippet_triggers_keep_the_existing_word_boundary() {
    let mut a = app("before", "Rust", None);
    set(&mut a, "tab_completion", json!(false));
    snippets(&mut a, &[("fore", "expanded", "source.rust")]);
    key(&mut a, "tab");
    assert_eq!(a.workspace.editor().text.to_string(), "before\t");
    let mut a = app("fore", "Rust", None);
    set(&mut a, "tab_completion", json!(false));
    snippets(&mut a, &[("fore", "expanded", "source.rust")]);
    key(&mut a, "tab");
    assert_eq!(a.workspace.editor().text.to_string(), "expanded");
}

#[test]
fn bracket_alignment_batches_carets_ignores_quoted_delimiters_and_undoes_once() {
    let original = "first(one,\nsecond([two,";
    let mut a = app(original, "Rust", None);
    set(&mut a, "indent_to_bracket", json!(true));
    set(&mut a, "translate_tabs_to_spaces", json!(true));
    a.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([
            Region::caret(10),
            Region::caret(original.len()),
        ]));
    key(&mut a, "enter");
    assert_eq!(
        a.workspace.editor().text.to_string(),
        "first(one,\n      \nsecond([two,\n        "
    );
    named(&mut a, "undo", json!({}));
    assert_eq!(a.workspace.editor().text.to_string(), original);
    let mut a = app("call(\"[\",", "Rust", None);
    set(&mut a, "indent_to_bracket", json!(true));
    set(&mut a, "translate_tabs_to_spaces", json!(true));
    key(&mut a, "enter");
    assert_eq!(a.workspace.editor().text.to_string(), "call(\"[\",\n     ");
}

#[test]
fn inferred_spaces_need_repeated_evidence_and_a_safe_right_side() {
    for enabled in [false, true] {
        for commit in ["tab", "enter"] {
            for (evidence, rhs, suffix) in [
                ("let count = 0;\n", ")", ""),
                ("count x\ncount y\ncount z\n", ")", ""),
                ("count x\ncount y\ncount z\n", "]", ""),
                ("count x\ncount y\ncount z\n", "}", ""),
                ("count x\ncount y\ncount z\n", ",", ""),
                ("count x\ncount y\ncount z\n", ";", ""),
                ("count x\ncount y\ncount z\n", "tail", ""),
                ("count x\ncount y\ncount z\n", "", " "),
                ("count x\ncount y\ncount z\n", "\n", " "),
                ("count x\ncount y\ncount z\n", " ", ""),
                ("count x\ncount y\ncount z\n", ".len()", ""),
                ("count x\ncount y\ncount z\n", "(", ""),
                ("count x\ncount y\ncount z\n", ":", ""),
                ("count x\ncount y\ncount z\n", "\"", ""),
                ("count x\ncount y\ncount z\n", ">", ""),
                ("count x\ncount y\ncount z\n", "\t", ""),
                ("count x\ncount y\ncount z\n", "\r\n", " "),
                ("let count = 0;\n", "+1", ""),
                ("count x\ncount y\ncount z\ncount)\ncount)\n", "+1", ""),
                ("count x\ncount y\ncount z\n", "+1", ""),
            ] {
                let prefix = format!("{evidence}foo(cou");
                let mut a = app(&format!("{prefix}{rhs}"), "Rust", None);
                engine(&mut a, &[]);
                set(&mut a, "auto_complete_trailing_spaces", json!(enabled));
                a.workspace.editor_mut().set_caret(prefix.len(), false);
                named(&mut a, "auto_complete", json!({}));
                until(&mut a, |a| a.workspace.editor().completion.visible());
                key(&mut a, commit);
                assert_eq!(
                    a.workspace.editor().text.to_string(),
                    format!(
                        "{evidence}foo(count{}{rhs}",
                        if enabled { suffix } else { "" }
                    )
                );
            }
        }
    }
    let mut a = app("count x\ncount y\ncount z\nfoo(cou+1", "Rust", None);
    engine(&mut a, &[]);
    a.workspace.editor_mut().set_caret(31, false);
    named(&mut a, "auto_complete", json!({}));
    until(&mut a, |a| a.workspace.editor().completion.visible());
    key(&mut a, "tab");
    assert_eq!(
        a.workspace.editor().text.to_string(),
        "count x\ncount y\ncount z\nfoo(count+1"
    );
}

#[test]
fn default_ranking_keeps_proximity_and_legacy_booleans() {
    let source = format!("alpha_aa\n{}alpha_zz\nalpha_", "unrelated\n".repeat(500));
    let mut a = app(&source, "Rust", None);
    engine(&mut a, &[]);
    named(&mut a, "auto_complete", json!({}));
    until(&mut a, |a| a.workspace.editor().completion.visible());
    assert_eq!(
        a.workspace.editor().completion.list.as_ref().unwrap().items[0].trigger,
        "alpha_zz"
    );
    for (policy, expected) in [
        (json!(true), "axbyc"),
        (json!(false), "abc"),
        (json!("strict"), "axbyc"),
        (json!("none"), "abc"),
        (json!("some"), "abc"),
    ] {
        let mut a = app("ab", "Rust", None);
        engine(&mut a, &["axbyc", "abc"]);
        set(&mut a, "auto_complete_preserve_order", policy);
        key(&mut a, "tab");
        until(&mut a, |a| !a.workspace.editor().completion.busy());
        assert_eq!(a.workspace.editor().text.to_string(), expected);
    }
}

#[test]
fn exact_snippet_beats_multiple_candidates_on_tab_with_and_without_popup() {
    for popup in [false, true] {
        let mut a = app("vec", "Rust", None);
        let mut completion = vel_complete::CompletionEngine::new();
        completion
            .add_completions(r#"{"scope":"source.rust","completions":["vector","vec_len"]}"#)
            .unwrap();
        let snippet = vel_snippet::SnippetFile::parse("<snippet><content>Vec&lt;$0&gt;</content><tabTrigger>vec</tabTrigger><scope>source.rust</scope></snippet>").unwrap();
        completion.add_snippet(snippet).unwrap();
        a.workspace
            .editor_mut()
            .completion
            .set_engine(Arc::new(completion));
        if popup {
            named(&mut a, "auto_complete", json!({}));
            until(&mut a, |a| a.workspace.editor().completion.visible());
        }
        key(&mut a, "tab");
        until(&mut a, |a| !a.workspace.editor().completion.busy());
        assert_eq!(a.workspace.editor().text.to_string(), "Vec<>");
        assert!(!a.workspace.editor().completion.visible());
    }
}

#[test]
fn stale_auto_pairs_and_base_scope_variables_never_parse() {
    for pair in ["(", "[", "{", "\"", "'"] {
        let mut a = app("", "Rust", None);
        until(&mut a, |a| {
            a.workspace
                .editor()
                .language
                .borrow()
                .as_ref()
                .unwrap()
                .scopes_ready()
        });
        let prefix = if pair == "\"" || pair == "'" {
            "o("
        } else {
            "o"
        };
        input(&mut a, prefix);
        assert!(
            !a.workspace
                .editor()
                .language
                .borrow()
                .as_ref()
                .unwrap()
                .scopes_ready()
        );
        key(&mut a, pair);
        assert_eq!(a.workspace.editor().exact_scope_parses.get(), 0, "{pair}");
        assert!(
            a.workspace
                .editor()
                .text
                .to_string()
                .starts_with(&format!("{prefix}{pair}"))
        );
    }
    let mut a = app("x", "Rust", None);
    named(&mut a, "insert_snippet", json!({"contents":"${TM_SCOPE}"}));
    assert_eq!(a.workspace.editor().exact_scope_parses.get(), 0);
    assert_eq!(a.workspace.editor().text.to_string(), "xsource.rust");
}

#[test]
fn scoped_trigger_and_comment_variables_share_one_prefix_parse() {
    let mut a = app(
        &format!("meta\n{}", "let unrelated = 1;\n".repeat(20_000)),
        "Rust",
        None,
    );
    a.workspace.editor_mut().set_caret(4, false);
    snippets(
        &mut a,
        &[("meta", "${TM_COMMENT_START}body", "source.rust - comment")],
    );
    key(&mut a, "tab");
    assert!(
        a.workspace
            .editor()
            .text
            .to_string()
            .starts_with("// body\n")
    );
    assert_eq!(a.workspace.editor().exact_scope_parses.get(), 1);
    assert_eq!(a.workspace.editor().exact_scope_cache.borrow().through, 5);
}

#[test]
fn punctuation_trigger_boundaries_and_trigger_variable_context() {
    for (source, trigger, expand) in [
        ("cat.", "t.", false),
        ("elif?", "if?", false),
        ("result.", "t.", false),
        (" t.", "t.", true),
        (" if?", "if?", true),
        ("word:", ":", true),
    ] {
        let mut a = app(source, "Rust", None);
        snippets(&mut a, &[(trigger, "EXP", "")]);
        set(&mut a, "tab_completion", json!(false));
        key(&mut a, "tab");
        if expand {
            assert!(a.workspace.editor().text.to_string().ends_with("EXP"));
        } else {
            assert!(a.workspace.editor().text.to_string().starts_with(source));
        }
    }
    for trigger in ["sel", "s."] {
        let mut a = app(trigger, "Rust", None);
        snippets(
            &mut a,
            &[(
                trigger,
                "[$SELECTION|$TM_SELECTED_TEXT|$TM_CURRENT_WORD|$TM_LINE_INDEX]",
                "source.rust",
            )],
        );
        key(&mut a, "tab");
        assert_eq!(a.workspace.editor().text.to_string(), "[|||0]");
    }
    let mut a = app("first\n  word", "Rust", None);
    a.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([Region::new(8, 12)]));
    named(
        &mut a,
        "insert_snippet",
        json!({"contents":"[$SELECTION|$TM_SELECTED_TEXT|$TM_CURRENT_WORD|$TM_LINE_INDEX]"}),
    );
    assert_eq!(
        a.workspace.editor().text.to_string(),
        "first\n  [word|word|word|1]"
    );
}

#[test]
fn hanging_indentation_and_bracket_pairs_keep_normal_enter_behavior() {
    for enabled in [false, true] {
        for (syntax, source, caret, expected) in [
            ("JSON", "[\n    1,", 8, "[\n    1,\n    "),
            ("Python", "result = compute(", 17, "result = compute(\n    "),
            ("Rust", "foo()", 4, "foo(\n    \n)"),
        ] {
            let mut a = app(source, syntax, None);
            set(&mut a, "translate_tabs_to_spaces", json!(true));
            set(&mut a, "indent_to_bracket", json!(enabled));
            a.workspace.editor_mut().set_caret(caret, false);
            key(&mut a, "enter");
            assert_eq!(
                a.workspace.editor().text.to_string(),
                expected,
                "{source} {enabled}"
            );
        }
    }
}

#[test]
fn close_tags_skip_raw_markup_nonmarkup_and_advance_past_existing_angle() {
    for source in ["ordinary", "🦀"] {
        let mut a = app(source, "HTML", None);
        input(&mut a, "/");
        assert_eq!(a.workspace.editor().exact_scope_parses.get(), 0);
        assert_eq!(a.workspace.editor().text.to_string(), format!("{source}/"));
    }
    let mut a = app("<div><>", "HTML", None);
    a.workspace.editor_mut().set_caret(6, false);
    input(&mut a, "/");
    assert_eq!(a.workspace.editor().text.to_string(), "<div></div>");
    assert_eq!(a.workspace.editor().caret, 11);
    for (syntax, source) in [
        ("Markdown", "`Vec<String>`<"),
        ("Markdown", "```html\n<div><"),
        ("JavaScript", "const value = a < b;\n<"),
    ] {
        let mut a = app(source, syntax, None);
        until(&mut a, |a| {
            a.workspace
                .editor()
                .language
                .borrow()
                .as_ref()
                .unwrap()
                .ready()
        });
        input(&mut a, "/");
        assert_eq!(a.workspace.editor().text.to_string(), format!("{source}/"));
        assert_eq!(a.workspace.editor().exact_scope_parses.get(), 0);
    }
}

#[test]
fn large_stale_tag_completion_defers_and_rejects_a_later_edit() {
    for action in ["complete", "edit", "move", "syntax"] {
        let source = format!("{}<div><", "<p>text</p>\n".repeat(300));
        let mut a = app(&source, "HTML", None);
        input(&mut a, "/");
        assert_eq!(a.workspace.editor().exact_scope_parses.get(), 0);
        assert!(a.workspace.editor().pending_closing_tag.is_some());
        match action {
            "edit" => input(&mut a, "x"),
            "move" => a.workspace.editor_mut().set_caret(0, false),
            "syntax" => a.workspace.set_file_type("XML").unwrap(),
            _ => (),
        }
        until(&mut a, |a| {
            a.workspace.editor().pending_closing_tag.is_none()
        });
        assert_eq!(
            a.workspace.editor().text.to_string(),
            format!(
                "{source}{}",
                match action {
                    "complete" => "/div>",
                    "edit" => "/x",
                    _ => "/",
                }
            )
        );
    }
}

#[test]
fn already_open_find_reseeds_using_the_same_selection_table() {
    for panel in ["find", "replace", "incremental_find"] {
        for enabled in [false, true] {
            for (region, expected) in [
                (Region::new(4, 7), "two"),
                (Region::caret(7), "old"),
                (Region::new(0, 8), "old"),
            ] {
                let mut a = app("one two\nthree", "Plain Text", None);
                set(&mut a, "find_selected_text", json!(enabled));
                named(&mut a, "show_panel", json!({"panel":panel,"text":"old"}));
                a.workspace
                    .editor_mut()
                    .set_selection(Selection::from_regions([region]));
                named(&mut a, "show_panel", json!({"panel":panel}));
                assert_eq!(
                    a.panels.query.value(),
                    if enabled { expected } else { "old" }
                );
            }
        }
    }
}

#[test]
fn palette_snippets_use_caret_after_comment_and_skip_unneeded_scope_work() {
    let profile = tempfile::tempdir().unwrap();
    let user = profile.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(user.join("after.sublime-snippet"), "<snippet><content>OK</content><scope>source.rust - comment</scope><description>AfterComment</description></snippet>").unwrap();
    let mut a = app("// comment\n", "Rust", Some(profile.path()));
    named(
        &mut a,
        "show_overlay",
        json!({"overlay":"command_palette","text":"AfterComment"}),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while a.panels.quick.items.is_empty() {
        let viewport = a.viewport();
        a.panels.poll(&mut a.workspace, viewport);
        assert!(Instant::now() < deadline);
        std::thread::park_timeout(Duration::from_millis(1));
    }
    assert!(
        a.panels
            .quick
            .items
            .iter()
            .any(|item| item.caption.contains("AfterComment"))
    );
    let mut a = app("// comment\n", "Rust", None);
    a.workspace
        .palette
        .retain(|item| item.invocation.command != "insert_snippet");
    named(&mut a, "show_overlay", json!({"overlay":"command_palette"}));
    assert_eq!(a.workspace.editor().exact_scope_parses.get(), 0);
}

fn open_ready(a: &mut App, path: &Path) {
    a.workspace
        .open(crate::cli::FileLocation {
            path: path.into(),
            line: 1,
            column: 1,
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while a.workspace.tabs[a.workspace.active].loading.is_some() {
        a.workspace.poll();
        assert!(Instant::now() < deadline);
        std::thread::park_timeout(Duration::from_millis(1));
    }
    a.sync_active_view();
}

#[test]
fn focus_save_only_leaves_the_active_file_and_never_materializes_lazy_drafts() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("inactive.txt");
    let active = dir.path().join("active.txt");
    for path in [&first, &active] {
        std::fs::write(path, "disk").unwrap();
    }
    let first = first.canonicalize().unwrap();
    let active = active.canonicalize().unwrap();
    let mut a = app("", "Plain Text", None);
    open_ready(&mut a, &first);
    input(&mut a, "draft ");
    open_ready(&mut a, &active);
    input(&mut a, "active ");
    let (window, documents) = a.workspace.snapshot().unwrap();
    let mut a = app("", "Plain Text", None);
    a.workspace.restore_window(&window, &documents).unwrap();
    a.sync_active_view();
    let hidden = a
        .workspace
        .tabs
        .iter()
        .position(|t| t.path.as_ref() == Some(&first))
        .unwrap();
    assert!(a.workspace.tabs[hidden].lazy.is_some());
    for tab in &mut a.workspace.tabs {
        tab.editor
            .settings
            .set(Layer::View, "save_on_focus_lost", json!(true));
    }
    a.save_on_focus_lost(None);
    assert_eq!(std::fs::read_to_string(&active).unwrap(), "active disk");
    assert_eq!(std::fs::read_to_string(&first).unwrap(), "disk");
    assert!(a.workspace.tabs[hidden].lazy.is_some());
    assert!(a.workspace.tabs[hidden].dirty());
    // A direct leave request also must not load a lazy sheet.
    let sheet = a.workspace.tabs[hidden].sheet;
    a.save_on_focus_lost(Some(sheet));
    assert!(a.workspace.tabs[hidden].lazy.is_some());
    assert_eq!(std::fs::read_to_string(&first).unwrap(), "disk");
}

#[test]
fn focus_save_writes_a_shared_buffer_once_and_preserves_safety_skips() {
    let dir = tempfile::tempdir().unwrap();
    for skip in ["clone", "deleted", "ime", "readonly", "external"] {
        let path = dir.path().join(format!("{skip}.txt"));
        std::fs::write(&path, "disk").unwrap();
        let mut a = app("", "Plain Text", None);
        open_ready(&mut a, &path);
        input(&mut a, "edit ");
        set(&mut a, "save_on_focus_lost", json!(true));
        if skip == "clone" {
            let original = a.workspace.active;
            a.workspace.clone_file().unwrap();
            let sibling = a.workspace.active;
            a.workspace.tabs[sibling].format.encoding = "invalid-encoding-fixture".into();
            a.workspace.active = original;
        } else if skip == "deleted" {
            a.workspace.tabs[a.workspace.active].deleted = true;
        } else if skip == "ime" {
            a.workspace.editor_mut().text_input(TextInput::Marked {
                text: "preedit".into(),
                selected: Utf16Range {
                    location: 0,
                    length: 0,
                },
                replacement: None,
            });
        } else if skip == "readonly" {
            a.workspace.editor_mut().read_only = true;
        } else {
            std::fs::write(&path, "external").unwrap();
        }
        a.save_on_focus_lost(None);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            match skip {
                "clone" => "edit disk",
                "external" => "external",
                _ => "disk",
            }
        );
        assert!(a.panels.message.is_empty());
        assert!(a.workspace.messages.is_empty());
        if skip == "clone" {
            let saved = vel_session::DiskStamp::read(&path).unwrap();
            a.save_on_focus_lost(None);
            assert_eq!(vel_session::DiskStamp::read(&path).unwrap(), saved);
        }
    }
    let path = dir.path().join("loading.txt");
    std::fs::write(&path, "content\n".repeat(100_000)).unwrap();
    let mut a = app("", "Plain Text", None);
    a.workspace
        .settings
        .set(Layer::User, "large_file.load_bytes", json!(1));
    a.workspace
        .open(crate::cli::FileLocation {
            path: path.clone(),
            line: 1,
            column: 1,
        })
        .unwrap();
    let tab = &mut a.workspace.tabs[a.workspace.active];
    assert!(tab.loading.is_some());
    tab.shared_dirty = true;
    tab.editor
        .settings
        .set(Layer::View, "save_on_focus_lost", json!(true));
    a.save_on_focus_lost(None);
    assert_eq!(std::fs::metadata(path).unwrap().len(), 800_000);
    assert!(a.panels.message.is_empty());
}

#[test]
#[ignore = "large-document input timings; run explicitly with --ignored --nocapture"]
fn large_document_input_hooks_timing() {
    for (label, syntax, source) in [
        (
            "js20k",
            "JavaScript",
            format!("{}<", "let x = left < right;\n".repeat(20_000)),
        ),
        (
            "html1.4mb",
            "HTML",
            format!(
                "{}<main><",
                "<p class=\"example\">A bounded scan need not inspect these tags.</p>\n"
                    .repeat(22_000)
            ),
        ),
        (
            "indent20k",
            "Rust",
            format!("{}call(one,", "let x = 1;\n".repeat(20_000)),
        ),
    ] {
        for stale in [false, true] {
            let mut a = app(&source, syntax, None);
            set(&mut a, "auto_complete", json!(false));
            set(&mut a, "indent_to_bracket", json!(true));
            until(&mut a, |a| {
                a.workspace
                    .editor()
                    .language
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .scopes_ready()
            });
            if stale {
                a.workspace.editor_mut().set_caret(0, false);
                input(&mut a, " ");
                let end = a.workspace.editor().text.len_bytes();
                a.workspace.editor_mut().set_caret(end, false);
            }
            let begin = Instant::now();
            input(&mut a, if label == "indent20k" { "\n" } else { "/" });
            let ms = begin.elapsed().as_secs_f64() * 1000.;
            assert_eq!(a.workspace.editor().exact_scope_parses.get(), 0);
            println!(
                "R18_LARGE_TIMING {}",
                json!({"case":label,"bytes":source.len(),"stale":stale,"input_ms":ms})
            );
            if label == "html1.4mb" {
                until(&mut a, |a| {
                    a.workspace.editor().pending_closing_tag.is_none()
                });
                assert!(
                    a.workspace
                        .editor()
                        .text
                        .to_string()
                        .ends_with("<main></main>")
                );
            }
        }
    }
    let source = format!("{}<main><", "<p>minified</p>".repeat(115_000));
    let mut a = app(&source, "HTML", None);
    set(&mut a, "large_file.viewport_bytes", json!(1));
    set(&mut a, "auto_complete", json!(false));
    let begin = Instant::now();
    input(&mut a, "/");
    println!(
        "R18_LARGE_TIMING {}",
        json!({"case":"minified_viewport","bytes":source.len(),"input_ms":begin.elapsed().as_secs_f64()*1000.})
    );
    assert_eq!(a.workspace.editor().exact_scope_parses.get(), 0);
}

fn scopes_ready(a: &App) -> bool {
    a.workspace
        .editor()
        .language
        .borrow()
        .as_ref()
        .unwrap()
        .scopes_ready()
}

fn palette_ready(a: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let viewport = a.viewport();
        a.panels.poll(&mut a.workspace, viewport);
        if a.panels.overlay.is_none() || !a.panels.quick_pending {
            break;
        }
        assert!(Instant::now() < deadline, "palette worker timeout");
        std::thread::park_timeout(Duration::from_millis(1));
    }
}

fn has_palette_item(a: &App, text: &str) -> bool {
    a.panels
        .quick
        .items
        .iter()
        .any(|item| item.caption.contains(text))
}

#[test]
fn palette_maps_scopes_and_refilters_only_the_unchanged_open_query() {
    let profile = tempfile::tempdir().unwrap();
    let user = profile.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    for (name, scope) in [("Rust", "source.rust - comment"), ("Html", "text.html")] {
        std::fs::write(user.join(format!("{name}.sublime-snippet")), format!(
            "<snippet><content>OK</content><scope>{scope}</scope><description>ScopeProbe {name}</description></snippet>"
        )).unwrap();
    }
    for change_query in [false, true] {
        let mut a = app("let value;", "Rust", Some(profile.path()));
        named(
            &mut a,
            "show_overlay",
            json!({"overlay":"command_palette","text":"ScopeProbe"}),
        );
        palette_ready(&mut a);
        // Cold display shows snippets pending a publication, without authorizing an edit.
        assert!(has_palette_item(&a, "ScopeProbe Rust"));
        assert!(has_palette_item(&a, "ScopeProbe Html"));
        assert_eq!(a.workspace.editor().exact_scope_parses.get(), 0);
        if change_query {
            key(&mut a, "end");
            input(&mut a, " ");
            palette_ready(&mut a);
        }
        until(&mut a, scopes_ready);
        palette_ready(&mut a);
        assert!(has_palette_item(&a, "ScopeProbe Rust"));
        assert_eq!(has_palette_item(&a, "ScopeProbe Html"), change_query);
        key(&mut a, "escape");
        palette_ready(&mut a);
        assert!(a.panels.overlay.is_none());
    }
    let mut a = app("let value;", "Rust", Some(profile.path()));
    until(&mut a, scopes_ready);
    input(&mut a, " // comment");
    key(&mut a, "left");
    named(
        &mut a,
        "show_overlay",
        json!({"overlay":"command_palette","text":"ScopeProbe"}),
    );
    palette_ready(&mut a);
    // Map the old publication for display until the newly typed comment is published.
    assert!(has_palette_item(&a, "ScopeProbe Rust"));
    assert!(!has_palette_item(&a, "ScopeProbe Html"));
    until(&mut a, scopes_ready);
    palette_ready(&mut a);
    assert!(!has_palette_item(&a, "ScopeProbe Rust"));
    assert_eq!(a.workspace.editor().exact_scope_parses.get(), 0);
    let mut a = app("let value;", "Rust", Some(profile.path()));
    named(
        &mut a,
        "show_overlay",
        json!({"overlay":"command_palette","text":"ScopeProbe"}),
    );
    key(&mut a, "escape");
    until(&mut a, scopes_ready);
    palette_ready(&mut a);
    assert!(a.panels.overlay.is_none());
}

#[test]
fn snippet_metadata_includes_go_rails_and_user_shell_variables() {
    for (syntax, contents, expected) in [
        (
            "HTML (Go)",
            "[${TM_TEMPLATE_START}|${TM_RAILS_TEMPLATE_START_RUBY_EXPR}]",
            "[{{ |]",
        ),
        (
            "HTML (Rails)",
            "[${TM_RAILS_TEMPLATE_START_RUBY_EXPR}]",
            "[<%= ]",
        ),
    ] {
        let mut a = app("<p>", syntax, None);
        named(&mut a, "insert_snippet", json!({"contents":contents}));
        assert_eq!(
            a.workspace.editor().text.to_string(),
            format!("<p>{expected}"),
            "{syntax}"
        );
        assert_eq!(a.workspace.editor().exact_scope_parses.get(), 1);
    }
    let profile = tempfile::tempdir().unwrap();
    let user = profile.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(user.join("custom.tmPreferences"), r#"<plist version="1.0"><dict>
        <key>scope</key><string>source.rust - comment</string><key>settings</key><dict>
        <key>shellVariables</key><array>
        <dict><key>name</key><string>USER_TAG</string><key>value</key><string>custom</string></dict>
        <dict><key>name</key><string>TM_LINE_TERMINATOR</string><key>value</key><string>terminator</string></dict>
        </array></dict></dict></plist>"#).unwrap();
    let mut a = app("", "Rust", Some(profile.path()));
    named(
        &mut a,
        "insert_snippet",
        json!({"contents":"[$USER_TAG|$TM_LINE_TERMINATOR]"}),
    );
    assert_eq!(a.workspace.editor().text.to_string(), "[custom|terminator]");
    // This also exercises the budgeted metadata fallback, rather than emitting empty variables.
    let source = format!("{}end", "let x = 1;\n".repeat(20_000));
    for touch_caret_line in [false, true] {
        let mut a = app(&source, "Rust", Some(profile.path()));
        until(&mut a, scopes_ready);
        a.workspace
            .editor_mut()
            .set_caret(if touch_caret_line { source.len() } else { 0 }, false);
        input(&mut a, " ");
        let end = a.workspace.editor().text.len_bytes();
        a.workspace.editor_mut().set_caret(end, false);
        let before = a.workspace.editor().text.to_string();
        named(&mut a, "insert_snippet", json!({"contents":"[$USER_TAG]"}));
        assert_eq!(
            a.workspace.editor().text.to_string(),
            if touch_caret_line {
                before
            } else {
                format!("{before}[custom]")
            }
        );
        assert_eq!(a.workspace.editor().exact_scope_parses.get(), 0);
    }
}

#[test]
fn exact_trigger_wins_with_typing_popup_and_disabled_tab_completion() {
    for popup in [false, true] {
        for tab_completion in [false, true] {
            for commit in ["tab", "enter"] {
                if commit == "enter" && !popup {
                    continue;
                }
                let source = "fn_alpha fnord fn_beta\n";
                let mut a = app(source, "Rust", None);
                snippets(&mut a, &[("fn", "EXACT", "source.rust")]);
                set(&mut a, "tab_completion", json!(tab_completion));
                set(&mut a, "auto_complete", json!(popup));
                set(&mut a, "auto_complete_delay", json!(0));
                for ch in ["f", "n"] {
                    input(&mut a, ch);
                    if popup {
                        until(&mut a, |a| !a.workspace.editor().completion.busy());
                        assert!(a.workspace.editor().completion.visible());
                    }
                }
                let selected =
                    popup.then(|| {
                        let completion = &a.workspace.editor().completion;
                        let items = &completion.list.as_ref().unwrap().items;
                        assert!(items.iter().all(|item| item.completion_format
                            != vel_complete::CompletionFormat::Snippet));
                        items[completion.selected].completion.clone()
                    });
                key(&mut a, commit);
                assert_eq!(
                    a.workspace.editor().text.to_string(),
                    format!(
                        "{source}{}",
                        if commit == "tab" {
                            "EXACT"
                        } else {
                            selected.as_deref().unwrap()
                        }
                    )
                );
                assert!(!a.workspace.editor().completion.visible());
            }
        }
    }
}

#[test]
fn scoped_tab_budget_requires_an_untouched_published_caret_line() {
    for state in ["cold", "untouched", "touched", "viewport"] {
        let source = format!("{}f", "let x = 1;\n".repeat(20_000));
        let mut a = app(&source, "Rust", None);
        snippets(&mut a, &[("fn", "EXACT", "source.rust")]);
        set(&mut a, "auto_complete", json!(false));
        set(&mut a, "translate_tabs_to_spaces", json!(false));
        if matches!(state, "untouched" | "touched") {
            if state == "untouched" {
                input(&mut a, "n");
            }
            until(&mut a, scopes_ready);
            if state == "untouched" {
                a.workspace.editor_mut().set_caret(0, false);
                input(&mut a, " ");
                let end = a.workspace.editor().text.len_bytes();
                a.workspace.editor_mut().set_caret(end, false);
            } else {
                input(&mut a, "n");
            }
        } else {
            input(&mut a, "n");
            if state == "viewport" {
                set(&mut a, "large_file.viewport_bytes", json!(1));
            }
        }
        key(&mut a, "tab");
        assert!(
            a.workspace
                .editor()
                .text
                .to_string()
                .ends_with(if state == "untouched" {
                    "EXACT"
                } else {
                    "fn\t"
                }),
            "{state}"
        );
        assert_eq!(a.workspace.editor().exact_scope_parses.get(), 0, "{state}");
    }
}

#[test]
fn leaving_view_finishes_or_cancels_tags_before_focus_save() {
    let dir = tempfile::tempdir().unwrap();
    for save in [false, true] {
        for publish in [false, true] {
            let path = dir.path().join(format!("pending-{save}-{publish}.html"));
            let source = format!("{}<div><", "<p>x</p>\n".repeat(20_000));
            std::fs::write(&path, &source).unwrap();
            let mut a = app("", "Plain Text", None);
            open_ready(&mut a, &path);
            let old = a.workspace.active;
            until(&mut a, scopes_ready);
            set(&mut a, "save_on_focus_lost", json!(save));
            a.workspace.editor_mut().set_caret(0, false);
            input(&mut a, " ");
            let end = a.workspace.editor().text.len_bytes();
            a.workspace.editor_mut().set_caret(end, false);
            input(&mut a, "/");
            assert!(a.workspace.editor().pending_closing_tag.is_some());
            if publish {
                let deadline = Instant::now() + Duration::from_secs(20);
                while !scopes_ready(&a) {
                    a.workspace
                        .editor()
                        .language
                        .borrow_mut()
                        .as_mut()
                        .unwrap()
                        .poll();
                    assert!(Instant::now() < deadline);
                    std::thread::park_timeout(Duration::from_millis(1));
                }
                assert!(a.workspace.editor().pending_closing_tag.is_some());
            }
            named(&mut a, "new_file", json!({}));
            let before = a.workspace.tabs[old].editor.text.to_string();
            assert!(before.ends_with(if publish { "<div></div>" } else { "<div></" }));
            assert!(a.workspace.tabs[old].editor.pending_closing_tag.is_none());
            assert_eq!(a.workspace.tabs[old].dirty(), !save);
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                if save { before.clone() } else { source }
            );
            // A later publication/poll may not edit the background document.
            for _ in 0..3 {
                a.workspace.poll();
                a.workspace.tabs[old].editor.poll_language();
            }
            assert_eq!(a.workspace.tabs[old].editor.text.to_string(), before);
            assert_eq!(a.workspace.tabs[old].dirty(), !save);
            assert_eq!(a.workspace.tabs[old].editor.exact_scope_parses.get(), 0);
        }
    }
}

#[test]
fn bracket_alignment_checks_long_line_limits_before_parsing() {
    let source = format!("const result = {}compute(value,", "left + ".repeat(9_000));
    let mut a = app(&source, "JavaScript", None);
    set(&mut a, "indent_to_bracket", json!(true));
    input(&mut a, "x");
    key(&mut a, "enter");
    assert_eq!(
        a.workspace.editor().text.to_string(),
        format!("{source}x\n")
    );
    assert_eq!(a.workspace.editor().exact_scope_parses.get(), 0);
}

#[test]
fn html_template_roots_close_tags_with_cold_and_stale_scopes() {
    let profile = tempfile::tempdir().unwrap();
    let user = profile.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    for (name, root, pattern, embed, escape) in [
        (
            "Outer Template",
            "source.outer",
            "\\{",
            "embedding.bridge",
            "\\}",
        ),
        (
            "Bridge Template",
            "embedding.bridge",
            "\\[",
            "text.html.basic",
            "\\]",
        ),
    ] {
        std::fs::write(user.join(format!("{name}.sublime-syntax")), format!(
            "%YAML 1.2\n---\nname: {name}\nscope: {root}\ncontexts:\n  main:\n    - match: '{pattern}'\n      embed: scope:{embed}\n      embed_scope: {embed}\n      escape: '{escape}'\n"
        )).unwrap();
    }
    // Repeat to include the cached syntax-set loader, which retains the derived gate.
    for _ in 0..2 {
        for stale in [false, true] {
            for (syntax, source) in [("PHP", "<div><"), ("Outer Template", "{[<div><")] {
                let mut a = app(source, syntax, Some(profile.path()));
                if stale {
                    until(&mut a, scopes_ready);
                    a.workspace.editor_mut().set_caret(0, false);
                    input(&mut a, " ");
                    let end = a.workspace.editor().text.len_bytes();
                    a.workspace.editor_mut().set_caret(end, false);
                }
                input(&mut a, "/");
                assert_eq!(
                    a.workspace.editor().text.to_string(),
                    format!("{}{source}/div>", if stale { " " } else { "" }),
                    "{syntax}/{stale}"
                );
            }
        }
    }
}

#[test]
#[ignore = "palette and long-line input timings; run explicitly with --ignored --nocapture"]
fn palette_and_long_line_scope_budget_timing() {
    let source = "var value = call(12345);".repeat(68_000);
    let mut a = app(&source, "JavaScript", None);
    input(&mut a, "x");
    let mut times = Vec::new();
    for _ in 0..7 {
        let begin = Instant::now();
        named(&mut a, "show_overlay", json!({"overlay":"command_palette"}));
        times.push(begin.elapsed().as_secs_f64() * 1000.);
        key(&mut a, "escape");
    }
    times.sort_by(f64::total_cmp);
    println!(
        "R18_SCOPE_TIMING {}",
        json!({"case":"palette_minified_stale","bytes":source.len(),"median_ms":times[3],"samples_ms":times})
    );
    assert!(times[3] < 10., "palette median {} ms", times[3]);
    assert_eq!(a.workspace.editor().exact_scope_parses.get(), 0);
    let source = format!("const result = {}compute(value,", "left + ".repeat(9_000));
    let mut a = app(&source, "JavaScript", None);
    set(&mut a, "indent_to_bracket", json!(true));
    input(&mut a, "x");
    let begin = Instant::now();
    key(&mut a, "enter");
    println!(
        "R18_SCOPE_TIMING {}",
        json!({"case":"indent_long_line_stale","bytes":source.len(),"input_ms":begin.elapsed().as_secs_f64()*1000.})
    );
    assert_eq!(a.workspace.editor().exact_scope_parses.get(), 0);
}
