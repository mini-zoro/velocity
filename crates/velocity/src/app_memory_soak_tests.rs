//! Memory soak: repeating real workloads must return the heap to its baseline.
//!
//! These count live heap bytes with the test allocator (`alloc_count`), so they run
//! alone and are ignored by the ordinary suite:
//! `cargo test --release -p velocity --bin velocity -- --ignored --test-threads=1 memory_soak`.
//! A workload runs once to warm bounded caches, then many times; the steady state may
//! not grow by more than a small slack, whatever the per-iteration peak was.
use super::*;
use crate::alloc_count::{allocation_calls, live_bytes};
use vel_text::Text;

const MIB: isize = 1024 * 1024;

fn app() -> App {
    app_with_text(Text::new())
}

fn app_with_text(text: Text) -> App {
    let options = Options::default();
    let mut workspace = Workspace::new(text, None, None, false).unwrap();
    workspace.wait_resources();
    let mut a = App::new(
        options.clone(),
        Clock::new(Instant::now(), std::time::SystemTime::now()),
        Reporter::for_options(&options, Instant::now()).unwrap(),
        workspace,
        Arc::new(AtomicBool::new(false)),
        std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(None))),
    );
    a.panels.side_visible = false;
    a.sync_active_view();
    a
}

#[test]
fn two_apps_keep_text_pixels_when_fonts_are_shared() {
    fn pixels(app: &mut App, fonts: Fonts) -> Vec<u8> {
        app.reload_font_views(fonts, 1.);
        let scene = app.scene();
        assert!(
            scene
                .layers
                .iter()
                .any(|layer| { layer.text.iter().any(|run| run.text.contains("let shared")) })
        );
        vel_render::snapshot::text_rgba(&scene, app.fonts.as_mut().unwrap(), 640, 360, 1.).unwrap()
    }
    let text = Text::from_str("let shared = 42;\n");
    let mut first = app_with_text(text.clone());
    let mut second = app_with_text(text);
    let before = pixels(&mut first, Fonts::load().unwrap());
    assert_eq!(pixels(&mut first, Fonts::load_shared().unwrap()), before);
    assert_eq!(pixels(&mut second, Fonts::load_shared().unwrap()), before);
}

fn named(a: &mut App, name: &str, args: Value) {
    a.run_named(None, name, &args);
}

/// Lets workers publish and retire what they hold, then reads the live heap.
fn settled(a: &mut App) -> isize {
    let mut last = live_bytes();
    for _ in 0..60 {
        a.poll_workspace_model();
        a.panels.poll(&mut a.workspace, 0..0);
        a.sync_active_view();
        let _ = a.scene();
        std::thread::sleep(Duration::from_millis(50));
        let now = live_bytes();
        if (now - last).abs() < 64 * 1024 {
            return now;
        }
        last = now;
    }
    last
}

/// Closes the active sheet without a save prompt.
fn discard(a: &mut App) {
    a.close_decisions
        .push_back(tab_adapter::CloseDecision::Discard);
    named(a, "close_file", json!({}));
    a.close_decisions.clear();
}

#[track_caller]
fn assert_flat(label: &str, base: isize, after: isize, slack: isize) {
    let grown = after - base;
    eprintln!(
        "{label}: baseline {:.1} MiB, after {:.1} MiB, grown {:+.2} MiB",
        base as f64 / MIB as f64,
        after as f64 / MIB as f64,
        grown as f64 / MIB as f64
    );
    assert!(
        grown <= slack,
        "{label}: the heap grew {:+.2} MiB over the soak (limit {:.2} MiB)",
        grown as f64 / MIB as f64,
        slack as f64 / MIB as f64
    );
}

/// A 1 MB document, many-caret edits and their undo, then the tab closes.
fn document_cycle(a: &mut App, text: &str) {
    named(a, "new_file", json!({}));
    named(a, "insert", json!({"characters": text}));
    named(a, "select_all", json!({}));
    named(a, "split_selection_into_lines", json!({}));
    named(a, "insert", json!({"characters": "x"}));
    named(a, "undo", json!({}));
    named(a, "redo", json!({}));
    named(a, "undo", json!({}));
    named(a, "single_selection", json!({}));
    let _ = a.scene();
    discard(a);
}

#[test]
#[ignore = "memory soak: run alone with --ignored --test-threads=1"]
fn memory_soak_documents_and_many_caret_edits_return_to_baseline() {
    let text = "alpha bravo charlie delta echo\n".repeat(33_000);
    let mut a = app();
    document_cycle(&mut a, &text);
    let base = settled(&mut a);
    for _ in 0..20 {
        document_cycle(&mut a, &text);
    }
    let after = settled(&mut a);
    assert_flat("documents", base, after, 4 * MIB);
}

#[test]
#[ignore = "memory soak: run alone with --ignored --test-threads=1"]
fn memory_soak_huge_single_line_paste_returns_to_baseline() {
    let line = "网关".repeat(350_000);
    let mut a = app();
    let cycle = |a: &mut App| {
        named(a, "new_file", json!({}));
        a.handle_text_input(
            None,
            TextInput::Insert {
                text: line.clone(),
                replacement: None,
            },
        );
        a.workspace.editor_mut().sync();
        let _ = a.scene();
        named(a, "move_to", json!({"to": "eof"}));
        let _ = a.scene();
        discard(a);
    };
    cycle(&mut a);
    let base = settled(&mut a);
    for _ in 0..10 {
        cycle(&mut a);
    }
    let after = settled(&mut a);
    assert_flat("huge line", base, after, 4 * MIB);
}

#[test]
#[ignore = "memory soak: run alone with --ignored --test-threads=1"]
fn memory_soak_many_open_tabs_closed_together_return_to_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..200)
        .map(|i| {
            let path = dir.path().join(format!("file{i}.rs"));
            std::fs::write(&path, format!("fn item_{i}() {{ let value_{i} = {i}; }}\n")).unwrap();
            path
        })
        .collect();
    let mut a = app();
    let cycle = |a: &mut App| {
        for path in &paths {
            named(a, "open_file", json!({"file": path}));
        }
        let _ = settled(a);
        named(a, "close_all", json!({}));
    };
    cycle(&mut a);
    let base = settled(&mut a);
    for _ in 0..5 {
        cycle(&mut a);
    }
    let after = settled(&mut a);
    // Before the fixes each cycle kept ~0.8 MiB: parked views of bulk-closed sheets,
    // per-widget theme state of closed tabs and a telemetry record per open.
    assert_flat("200 files", base, after, MIB);
}

#[test]
#[ignore = "memory soak: run alone with --ignored --test-threads=1"]
fn memory_soak_painted_tabs_closed_one_by_one_and_together_return_to_baseline() {
    let mut a = app();
    let cycle = |a: &mut App| {
        for _ in 0..200 {
            named(a, "new_file", json!({}));
            named(
                a,
                "insert",
                json!({"characters": "fn item() { let value = 1; }\n"}),
            );
            let _ = a.scene();
            discard(a);
        }
        for _ in 0..200 {
            named(a, "new_file", json!({}));
            named(
                a,
                "insert",
                json!({"characters": "fn item() { let value = 1; }\n"}),
            );
        }
        let _ = a.scene();
        for _ in 0..400 {
            a.close_decisions
                .push_back(tab_adapter::CloseDecision::Discard);
        }
        named(a, "close_all", json!({}));
        a.close_decisions.clear();
    };
    cycle(&mut a);
    let base = settled(&mut a);
    for _ in 0..5 {
        cycle(&mut a);
    }
    let after = settled(&mut a);
    // Before the fixes: +3.5 MiB (theme state of painted, closed tabs) plus +3 MiB
    // (views of tabs closed by close_all) over these five cycles.
    assert!(
        a.sheet_views.len() <= a.workspace.tabs.len(),
        "{} parked views for {} tabs",
        a.sheet_views.len(),
        a.workspace.tabs.len()
    );
    assert_flat("painted tabs", base, after, MIB);
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "memory soak: run alone with --ignored --test-threads=1"]
fn memory_soak_pressure_releases_cjk_and_image_caches_without_changing_output() {
    use vel_render::scene::{Image, ImageQuad, Scene, Surface};

    let mut cjk = String::new();
    for i in 0..4096 {
        cjk.push(char::from_u32(0x4e00 + i).unwrap());
        if i % 64 == 63 {
            cjk.push('\n');
        }
    }
    let mut a = app_with_text(Text::from(cjk.as_str()));
    // A blinking caret changes its alpha between the two compared frames whenever the
    // trim takes longer than a blink phase on a slow CI runner.
    a.workspace
        .editor_mut()
        .settings
        .set(Layer::View, "caret_style", json!("solid"));
    a.sync_active_view();
    let original = a.workspace.tabs[a.workspace.active].sheet;
    let size = Size {
        width: 1400.,
        height: 1200.,
    };
    a.resize_views(size, 1.);
    let image = |id, side| {
        Arc::new(Image {
            id,
            width: side,
            height: side,
            logical_size: [side as f32; 2],
            rgba: vec![180; side as usize * side as usize * 4].into(),
        })
    };

    // Simulate completed decodes in three real parked tabs. Their view state
    // must survive pressure even though their pixels can be released.
    for id in 1..=3 {
        a.workspace.new_file().unwrap();
        a.sync_active_view();
        a.view.image_preview.set_test_image(image(id, 512));
        a.view.scroll_x = id as f64;
    }
    a.workspace
        .select(a.workspace.sheet_index(original).unwrap());
    a.sync_active_view();
    let mut fonts = Fonts::load_named("PingFang SC")
        .or_else(|_| Fonts::load())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    // Complete any asynchronous cascade lookup before comparing frames.
    loop {
        for line in cjk.lines() {
            fonts.shape(line, 13.);
        }
        if !fonts.fallback_pending() {
            break;
        }
        assert!(Instant::now() < deadline, "font cascade did not settle");
        std::thread::sleep(Duration::from_millis(1));
        fonts.poll_fallbacks();
    }
    assert!(fonts.document_cache_bytes().0 > 0);
    a.fonts = Some(fonts);
    a.sync_active_view();
    a.renderer = Renderer::offscreen(size.width as u32, size.height as u32)
        .inspect_err(|err| eprintln!("skipped pixel readback: no Metal adapter ({err})"))
        .ok();

    let pictures: Vec<_> = (4..=6).map(|id| image(id, 32)).collect();
    let scene = |app: &mut App| {
        let mut scene = app.scene();
        scene.layers.push(SceneLayer {
            surfaces: pictures
                .iter()
                .enumerate()
                .map(|(i, image)| {
                    let rect = Rect {
                        x: 4. + i as f64 * 34.,
                        y: 32.,
                        width: 32.,
                        height: 32.,
                    };
                    Surface::Image(ImageQuad {
                        image: image.clone(),
                        rect,
                        clip: rect,
                        source: [0., 0., 32., 32.],
                        color: [1.; 4],
                    })
                })
                .collect(),
            ..Default::default()
        });
        scene
    };
    let render = |a: &mut App, scene: &Scene| {
        a.renderer.as_mut().map(|renderer| {
            renderer
                .render_rgba(scene, a.fonts.as_mut().unwrap(), 1.)
                .unwrap()
        })
    };
    // Prime window geometry and theme, and let time-driven state (the tab strip's slide
    // after the selection change) finish, so only the trim can change the next frame.
    // A slow CI runner otherwise captured the "before" scene mid-animation.
    let before_scene = {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut last = format!("{:?}", scene(&mut a));
        loop {
            std::thread::sleep(Duration::from_millis(40));
            let next = scene(&mut a);
            let text = format!("{next:?}");
            if text == last {
                break next;
            }
            assert!(Instant::now() < deadline, "the scene never settled");
            last = text;
        }
    };
    let before_pixels = render(&mut a, &before_scene);
    let parked: Vec<_> = a
        .sheet_views
        .iter()
        .map(|(&sheet, view)| (sheet, view.scroll_x))
        .collect();
    let before = live_bytes();
    a.trim_memory(memory::Trim::Pressure);
    let after = live_bytes();
    eprintln!(
        "pressure trim: live heap {before} -> {after} bytes (released {})",
        before - after
    );
    assert!(before - after >= 2 * MIB);
    for (sheet, scroll_x) in parked {
        let view = &a.sheet_views[&sheet];
        assert_eq!(view.scroll_x, scroll_x);
        assert!(view.image_preview.image().is_none());
    }
    assert_eq!(a.workspace.editor().buffer.text().len_bytes(), cjk.len());
    let after_scene = scene(&mut a);
    let (before_text, after_text) = (format!("{before_scene:?}"), format!("{after_scene:?}"));
    if before_text != after_text {
        // Name the first difference: CI runners differ from dev Macs in installed fonts.
        let at = before_text
            .bytes()
            .zip(after_text.bytes())
            .position(|(b, a)| b != a)
            .unwrap_or(before_text.len().min(after_text.len()));
        let window = |text: &str| {
            let start = text.floor_char_boundary(at.saturating_sub(300));
            let end = text.ceil_char_boundary((at + 300).min(text.len()));
            text[start..end].to_owned()
        };
        panic!(
            "trim changed the scene at byte {at} of {}/{}:\nbefore: {}\nafter:  {}",
            before_text.len(),
            after_text.len(),
            window(&before_text),
            window(&after_text)
        );
    }
    assert!(
        render(&mut a, &after_scene) == before_pixels,
        "trim changed rendered pixels"
    );
}

fn pointer(a: &mut App, kind: MouseKind, position: Point) {
    a.handle_mouse_event(
        None,
        MouseEvent {
            button: 0,
            kind,
            position,
            click_count: 1,
            modifiers: Modifiers::default(),
            timestamp: 0.,
        },
    );
}

/// One floating drag of the middle tab: a diagonal sweep out of the strip and
/// back, released on the strip, then the landing runs to its end. Returns the
/// allocation calls made by the pointer moves alone.
fn tab_drag_cycle(a: &mut App, sheet: u64) -> usize {
    let _ = a.scene();
    let (_, rect) = *a.tab_hits.iter().find(|(id, _)| *id == sheet).unwrap();
    let start = Point {
        x: rect.x + rect.width / 2.,
        y: rect.y + rect.height / 2.,
    };
    pointer(a, MouseKind::Down, start);
    let _ = a.scene();
    pointer(
        a,
        MouseKind::Drag,
        Point {
            x: start.x + 6.,
            y: start.y + 3.,
        },
    );
    assert!(a.tab_press.as_ref().unwrap().drag.unwrap().floating);
    let before = allocation_calls();
    for step in 0..240 {
        let t = f64::from(step) / 239.;
        let (dx, dy) = if t < 0.5 {
            (300. * t * 2., 150. * t * 2.)
        } else {
            (300. - 420. * (t - 0.5) * 2., 150. * (1. - (t - 0.5) * 2.))
        };
        pointer(
            a,
            MouseKind::Drag,
            Point {
                x: start.x + dx,
                y: start.y + dy,
            },
        );
    }
    let moves = allocation_calls() - before;
    let end = Point {
        x: start.x - 120.,
        y: start.y,
    };
    pointer(a, MouseKind::Up, end);
    let far = Instant::now() + Duration::from_secs(1);
    a.advance_tab_landing(None, far);
    a.release_landed_tab_image(None, far + Duration::from_secs(1), true);
    assert!(a.tab_landing.is_none() && a.sessions.borrow().tab_drag.is_none());
    moves
}

#[test]
#[ignore = "memory soak: run alone with --ignored --test-threads=1"]
fn memory_soak_floating_tab_moves_allocate_nothing_and_drags_return_to_baseline() {
    let mut a = app();
    named(&mut a, "new_file", json!({}));
    named(&mut a, "new_file", json!({}));
    a.window_frame = Some([600., 300., a.window_size.width, a.window_size.height]);
    let sheet = a.workspace.workbench.groups[0].sheets[1];
    tab_drag_cycle(&mut a, sheet);
    let base = settled(&mut a);
    for _ in 0..100 {
        assert_eq!(
            tab_drag_cycle(&mut a, sheet),
            0,
            "pointer moves of a floating tab must not allocate"
        );
    }
    let after = settled(&mut a);
    assert_flat("floating tab drags", base, after, MIB / 4);
}
