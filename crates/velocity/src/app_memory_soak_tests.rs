//! Memory soak: repeating real workloads must return the heap to its baseline.
//!
//! These count live heap bytes with the test allocator (`alloc_count`), so they run
//! alone and are ignored by the ordinary suite:
//! `cargo test --release -p velocity --bin velocity -- --ignored --test-threads=1 memory_soak`.
//! A workload runs once to warm bounded caches, then many times; the steady state may
//! not grow by more than a small slack, whatever the per-iteration peak was.
use super::*;
use crate::alloc_count::live_bytes;
use vel_text::Text;

const MIB: isize = 1024 * 1024;

fn app() -> App {
    let options = Options::default();
    let mut workspace = Workspace::new(Text::new(), None, None, false).unwrap();
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
