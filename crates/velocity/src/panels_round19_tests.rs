use super::*;
use std::time::Duration;

fn wait(w: &mut Workspace, p: &mut Panels, condition: impl Fn(&Workspace, &Panels) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        w.poll();
        p.poll(w, 0..100);
        if condition(w, p) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{} / {:?}",
            p.message,
            w.messages
        );
        std::thread::park_timeout(Duration::from_millis(1));
    }
}
fn setup() -> (tempfile::TempDir, tempfile::TempDir, Workspace, Panels) {
    let root = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    for (file, text) in [
        (
            "a.rs",
            "fn local() {}\nfn caller() { target(); local(); }\n",
        ),
        ("b.rs", "fn target() {}\nfn another() { local(); }\n"),
        ("c.rs", "fn third() {}\n"),
    ] {
        std::fs::write(root.path().join(file), text).unwrap();
    }
    let mut w =
        Workspace::new(vel_text::Text::new(), Some(data.path().into()), None, false).unwrap();
    let mut p = Panels::new(&w, false);
    p.add_folder(&mut w, root.path().into()).unwrap();
    for name in ["a.rs", "b.rs", "c.rs"] {
        w.open(FileLocation {
            path: root.path().join(name),
            line: 1,
            column: 1,
        })
        .unwrap();
    }
    wait(&mut w, &mut p, |w, p| {
        w.tabs.iter().all(|t| t.loading.is_none())
            && p.intel.projects.first().is_some_and(|project| {
                !project.index.status().active
                    && project.index.lookup_definition("target").len() == 1
                    && !project.index.lookup_references("local").is_empty()
            })
    });
    w.select(0);
    (root, data, w, p)
}
fn invoke(w: &mut Workspace, p: &mut Panels, command: &str, side: bool, clear: bool) {
    let at = w.editor().text.to_string().find("target();").unwrap();
    w.editor_mut().set_caret(at, false);
    p.action(
        w,
        command,
        &json!({"side_by_side":side,"clear_to_right":clear}),
        0..100,
    )
    .unwrap();
    wait(w, p, |_, p| !p.intel.pending);
}

#[test]
fn r19_goto_side_by_side_and_clear_to_right_sheet_outcomes() {
    for (side, clear, count) in [
        (false, false, 1),
        (false, true, 1),
        (true, false, 3),
        (true, true, 2),
    ] {
        let (_root, _data, mut w, mut p) = setup();
        let sheets: Vec<_> = w.tabs.iter().map(|t| t.sheet).collect();
        w.workbench.groups[0]
            .selected
            .extend([sheets[0], sheets[2]]);
        invoke(&mut w, &mut p, "goto_definition", side, clear);
        assert_eq!(w.workbench.groups.len(), 1);
        assert_eq!(w.workbench.groups[0].selected.len(), count);
        assert_eq!(w.tabs[w.active].sheet, sheets[1]);
        assert_eq!(w.workbench.groups[0].focused, Some(sheets[1]));
        if side {
            assert!(w.workbench.groups[0].selected.contains(&sheets[0]));
        }
        if side && clear {
            assert!(!w.workbench.groups[0].selected.contains(&sheets[2]));
        }
        assert!(
            w.workbench
                .validate(&w.tabs.iter().map(|t| t.sheet).collect())
                .is_ok()
        );
    }
}

#[test]
fn r19_side_by_side_existing_other_group_clones_and_retains_group() {
    let (_root, _data, mut w, mut p) = setup();
    let source = w.tabs[0].sheet;
    let target = w.tabs[1].sheet;
    w.workbench_action("set_layout", &json!({"preset":"columns2"}))
        .unwrap();
    w.select(1);
    w.workbench_action("move_to_group", &json!({"group":1}))
        .unwrap();
    w.select(w.sheet_index(source).unwrap());
    invoke(&mut w, &mut p, "goto_definition", true, true);
    let opened = w.tabs[w.active].sheet;
    assert_ne!(opened, target);
    assert_eq!(w.workbench.group_of(target), Some(1));
    assert_eq!(w.workbench.groups[1].focused, Some(target));
    assert_eq!(w.workbench.group_of(opened), Some(0));
    assert!(w.workbench.groups[0].selected.contains(&source));
    assert!(w.workbench.groups[0].selected.contains(&opened));
    assert!(
        w.workbench
            .validate(&w.tabs.iter().map(|t| t.sheet).collect())
            .is_ok()
    );
}

#[test]
fn r19_hover_scope_table_and_stale_prefix_budget() {
    let mut w = Workspace::new(vel_text::Text::new(), None, None, false).unwrap();
    let mut p = Panels::new(&w, false);
    for (syntax, source, needle, allowed) in [
        ("Rust", "fn symbol() {}", "symbol", true),
        ("Rust", "// symbol", "symbol", false),
        ("Rust", "let s = \"symbol\";", "symbol", false),
        ("Plain text", "symbol", "symbol", false),
        ("HTML", "<div class=\"symbol\">", "symbol", true),
        ("HTML", "<div id=\"symbol\">", "symbol", true),
        ("HTML", "<div title=\"symbol\">", "symbol", false),
        ("HTML", "<div>symbol</div>", "symbol", false),
        (
            "HTML",
            "<script>function symbol() {}</script>",
            "symbol",
            true,
        ),
        (
            "HTML",
            "<style>.symbol { color: red; }</style>",
            "symbol",
            true,
        ),
        ("Markdown", "symbol", "symbol", false),
        (
            "Markdown",
            "```javascript\nfunction symbol() {}\n```",
            "symbol",
            true,
        ),
    ] {
        w.editor_mut().run("select_all", &json!({})).unwrap();
        w.editor_mut()
            .run("insert", &json!({"characters":source}))
            .unwrap();
        w.set_file_type(syntax).unwrap();
        let point = source.find(needle).unwrap() + 1;
        wait(&mut w, &mut p, |w, _| {
            w.editor()
                .language
                .borrow()
                .as_ref()
                .is_some_and(|l| l.scopes_ready_at(&w.editor().text, point))
        });
        p.hover_source = None;
        p.hover_requested = false;
        p.intel.cancel_query();
        p.hover_at(&w, Point { x: 20., y: 20. }, point);
        p.hover_since = Instant::now() - Duration::from_secs(1);
        p.poll(&mut w, 0..100);
        assert_eq!(p.hover_requested, allowed, "{syntax}: {source}");
        if !allowed {
            assert!(
                p.hover_term.is_empty(),
                "excluded scopes must not keep idle polling alive"
            );
        }
        assert_eq!(w.editor().exact_scope_parses.get(), 0);
    }
}

#[test]
fn r19_hover_stale_embedded_pointer_moves_never_parse() {
    for syntax in ["HTML", "Markdown"] {
        let mut w = Workspace::new(vel_text::Text::new(), None, None, false).unwrap();
        let mut p = Panels::new(&w, false);
        let line = format!("symbol {}\n", "x".repeat(951));
        let source = format!(
            "{}{}",
            if syntax == "HTML" {
                "<script>\n"
            } else {
                "```javascript\n"
            },
            line.repeat(255)
        );
        w.editor_mut()
            .run("insert", &json!({"characters":source}))
            .unwrap();
        w.set_file_type(syntax).unwrap();
        w.editor_mut()
            .run("insert", &json!({"characters":"x"}))
            .unwrap();
        let parses = w.editor().exact_scope_parses.get();
        let mut worst = Duration::ZERO;
        for row in 1..=51 {
            let point = w.editor().text.line_to_byte(row) + 2;
            let begin = Instant::now();
            p.hover_at(
                &w,
                Point {
                    x: 20.,
                    y: row as f64,
                },
                point,
            );
            worst = worst.max(begin.elapsed());
            p.hover_since = Instant::now() - Duration::from_secs(1);
            p.poll(&mut w, 0..100);
            assert!(!p.hover_requested, "unpublished scopes suppress the popup");
        }
        eprintln!(
            "{syntax} stale pointer worst: {:.3} ms, parses: {}",
            worst.as_secs_f64() * 1000.,
            w.editor().exact_scope_parses.get() - parses
        );
        assert!(worst < Duration::from_millis(1));
        assert_eq!(w.editor().exact_scope_parses.get(), parses);
    }
}

#[test]
fn r19_hover_pointer_excludes_self_lists_references_and_discards_stale() {
    let (_root, _data, mut w, mut p) = setup();
    let end = w.editor().text.len_bytes();
    w.editor_mut().set_caret(end, false);
    let point = w.editor().text.to_string().find("local").unwrap() + 1;
    p.hover_at(&w, Point { x: 20., y: 20. }, point);
    wait(&mut w, &mut p, |_, p| !p.hover.is_empty());
    assert_eq!(
        p.hover_definitions, 0,
        "definition beneath pointer excluded"
    );
    assert!(
        p.hover
            .iter()
            .all(|l| !l.range.contains(&point) || Some(&l.file) != w.tabs[w.active].path.as_ref())
    );
    p.size = Size {
        width: 800.,
        height: 600.,
    };
    p.paint_hover(&w, &mut Scene::default());
    assert!(
        p.hover_html
            .as_ref()
            .unwrap()
            .html
            .source
            .contains("References")
    );
    w.editor_mut()
        .run("insert", &json!({"characters":"x"}))
        .unwrap();
    p.poll(&mut w, 0..100);
    assert!(p.hover.is_empty());
    assert!(p.hover_html.is_none());
}

#[test]
fn r19_hover_unpublished_scopes_use_slow_bounded_wakes() {
    let mut w = Workspace::new(
        vel_text::Text::from("<script>function symbol() {}</script>"),
        None,
        None,
        false,
    )
    .unwrap();
    w.set_file_type("HTML").unwrap();
    let mut p = Panels::new(&w, false);
    // Do not poll the workspace: the worker may finish, but no scopes have been published.
    let point = 19;
    p.hover_at(&w, Point { x: 20., y: 20. }, point);
    p.hover_since = Instant::now() - Duration::from_millis(300);
    p.poll(&mut w, 0..100);
    assert!(!p.hover_requested);
    assert!(!p.busy(), "unpublished scopes must not request 8 ms frames");
    let deadline = p.message_deadline().expect("slow scope retry");
    assert!(deadline.saturating_duration_since(Instant::now()) >= Duration::from_millis(90));
    for _ in 0..20 {
        p.poll(&mut w, 0..100);
        assert_eq!(p.message_deadline(), Some(deadline));
        assert!(!p.busy());
    }
    // Publication can reveal the hover without another pointer move.
    wait(&mut w, &mut p, |_, p| p.hover_requested);
    assert!(p.hover_retry.is_none());

    w.editor_mut()
        .run("insert", &json!({"characters":"x"}))
        .unwrap();
    // The prior published hover may still have an index query in flight. Give the
    // independent unpublished timeout its own panel so busy() measures scope wakes.
    let mut p = Panels::new(&w, false);
    p.hover_at(&w, Point { x: 20., y: 20. }, point);
    p.hover_since = Instant::now() - Duration::from_millis(2001);
    p.poll(&mut w, 0..100);
    assert!(p.hover_term.is_empty());
    assert!(!p.busy());
    assert!(p.message_deadline().is_none());
    assert_eq!(w.editor().exact_scope_parses.get(), 0);
}

#[test]
fn r19_find_result_and_closed_view_history() {
    let (root, _data, mut w, mut p) = setup();
    w.editor_mut().set_caret(4, false);
    *w.jump_history.borrow_mut() = Default::default();
    w.observe_navigation();
    let original = w.tabs[w.active].sheet;
    p.result_view.push(files::Line {
        path: root.path().canonicalize().unwrap().join("b.rs").into(),
        number: 2,
        text: "fn another() { local(); }".into(),
        highlights: std::iter::once(15..20).collect(),
        column_offset: 0,
        match_count: 1,
    });
    p.result_selected = p
        .result_view
        .rows
        .iter()
        .position(|row| matches!(row, files::Row::Line(_)))
        .unwrap();
    p.action(
        &mut w,
        "show_panel",
        &json!({"panel":"find_results"}),
        0..100,
    )
    .unwrap();
    p.key(&mut w, &Key::parse("enter").unwrap(), 0..100)
        .unwrap();
    assert_ne!(w.tabs[w.active].sheet, original);
    p.action(&mut w, "jump_back", &json!({}), 0..100).unwrap();
    assert_eq!(w.tabs[w.active].sheet, original);
    assert_eq!(w.editor().caret, 4);
    p.action(&mut w, "jump_forward", &json!({}), 0..100)
        .unwrap();
    let index = w.sheet_index(original).unwrap();
    w.close(index).unwrap();
    p.action(&mut w, "jump_back", &json!({}), 0..100).unwrap();
    wait(&mut w, &mut p, |w, _| w.tabs[w.active].loading.is_none());
    assert_eq!(
        w.tabs[w.active].path.as_ref().unwrap().file_name().unwrap(),
        "a.rs"
    );
    assert_eq!(w.editor().caret, 4);
}

#[test]
fn r19_reference_preview_arguments_cancel_and_commit() {
    for accept in [false, true] {
        let (_root, _data, mut w, mut p) = setup();
        let source = w.tabs[0].sheet;
        w.editor_mut().set_caret(4, false);
        let original = w.editor().buffer.view(w.editor().id).sel().clone();
        p.action(
            &mut w,
            "goto_reference",
            &json!({"side_by_side":true,"clear_to_right":true}),
            0..100,
        )
        .unwrap();
        wait(&mut w, &mut p, |_, p| {
            p.overlay == Some(Overlay::Locations) && !p.quick_pending
        });
        assert!(w.workbench.groups[0].selected.contains(&source));
        assert!(w.workbench.groups[0].selected.len() >= 2);
        for _ in 0..3 {
            p.key(&mut w, &Key::parse("down").unwrap(), 0..100).unwrap();
            assert!(w.tabs.iter().filter(|tab| tab.transient).count() <= 1);
        }
        p.key(
            &mut w,
            &Key::parse(if accept { "enter" } else { "escape" }).unwrap(),
            0..100,
        )
        .unwrap();
        assert!(p.overlay.is_none());
        assert!(
            w.workbench
                .validate(&w.tabs.iter().map(|t| t.sheet).collect())
                .is_ok()
        );
        if !accept {
            assert_eq!(w.tabs[w.active].sheet, source);
            assert_eq!(w.editor().buffer.view(w.editor().id).sel(), &original);
            assert_eq!(w.workbench.groups[0].selected, BTreeSet::from([source]));
        } else {
            p.action(&mut w, "jump_back", &json!({}), 0..100).unwrap();
            assert_eq!(w.tabs[w.active].sheet, source);
            assert_eq!(w.editor().buffer.view(w.editor().id).sel(), &original);
        }
    }
}

#[test]
fn r19_cancel_pending_definition_cannot_jump_later() {
    let (_root, _data, mut w, mut p) = setup();
    let source = w.tabs[w.active].sheet;
    let point = w.editor().text.to_string().find("target();").unwrap();
    w.editor_mut().set_caret(point, false);
    p.action(&mut w, "goto_definition", &json!({}), 0..100)
        .unwrap();
    p.action(&mut w, "show_panel", &json!({"panel":"find"}), 0..100)
        .unwrap();
    // A serial worker barrier ensures any already enqueued query has had time to run.
    p.intel
        .query(crate::intelligence::Query::Symbols("target".into()), &w);
    wait(&mut w, &mut p, |_, p| !p.intel.pending);
    assert_eq!(w.tabs[w.active].sheet, source);
    assert_eq!(w.editor().caret, point);
}
