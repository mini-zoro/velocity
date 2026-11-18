//! Paired App typing and keymap startup probes, also runnable against the main worktree.
use super::*;
use vel_buffer::{Region, Selection};
use vel_config::{keymap::Key, settings::Layer};
use vel_text::Text;

#[repr(C)]
#[derive(Default)]
struct ThreadCounts {
    instructions: u64,
    cycles: u64,
}
// THSC_CPI, from Apple's XNU bsd/sys/resource_private.h.
unsafe extern "C" {
    fn thread_selfcounts(kind: u32, counts: *mut ThreadCounts, size: usize) -> libc::c_int;
}
fn instructions() -> u64 {
    let mut counts = ThreadCounts::default();
    // SAFETY: kind 1 writes the two counters to this live repr(C) allocation.
    let status = unsafe { thread_selfcounts(1, &mut counts, std::mem::size_of::<ThreadCounts>()) };
    assert_eq!(status, 0);
    assert!(counts.instructions > 0);
    counts.instructions
}

fn measure(f: impl FnOnce()) -> (u64, u128) {
    let start = Instant::now();
    let before = instructions();
    f();
    (instructions() - before, start.elapsed().as_nanos())
}

fn input(app: &mut App, key: &Key, text: &str) {
    let time = Instant::now();
    let viewport = app.viewport();
    if !app.dispatch_key_binding(None, key, time, viewport) {
        let input = TextInput::Insert {
            text: text.into(),
            replacement: None,
        };
        let viewport = app.viewport();
        if !app
            .panels
            .text_input(&mut app.workspace, input.clone(), viewport)
        {
            app.workspace.editor_mut().text_input(input);
            app.changed_state(time, true);
        }
    }
}

#[test]
#[ignore = "paired syntax-aware App-path benchmark; no GUI"]
fn round21_typing_after_enter() {
    crate::test_login::install();
    let enter = Key::parse("enter").unwrap();
    let word = "endpoint";
    let keys: Vec<_> = word
        .as_bytes()
        .iter()
        .enumerate()
        .map(|(i, _)| Key::parse(&word[i..i + 1]).unwrap())
        .collect();
    for (syntax, seed, column) in [
        ("Rust", "if condition {\n}\n", 14),
        ("Python", "if condition:\n    pass\n", 13),
    ] {
        for count in [1, 1_000] {
            let text = seed.repeat(1_000);
            let carets =
                Selection::from_regions((0..count).map(|i| Region::caret(seed.len() * i + column)));
            let mut w = Workspace::new(Text::from(text.as_str()), None, None, false).unwrap();
            w.set_file_type(syntax).unwrap();
            w.wait_resources();
            for (setting, value) in [
                ("word_wrap", json!(false)),
                ("auto_match_enabled", json!(false)),
                ("translate_tabs_to_spaces", json!(true)),
            ] {
                w.editor_mut().settings.set(Layer::View, setting, value);
            }
            w.editor_mut().sync();
            let options = Options::default();
            let mut app = App::new(
                options.clone(),
                Clock::new(Instant::now(), std::time::SystemTime::now()),
                Reporter::for_options(&options, Instant::now()).unwrap(),
                w,
                Arc::new(AtomicBool::new(false)),
                std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(None))),
            );
            app.panels.side_visible = false;
            app.sync_active_view();
            let mut samples = Vec::new();
            for sample in 0..40 {
                let editor = app.workspace.editor_mut();
                editor.set_selection(carets.clone());
                let deadline = Instant::now() + Duration::from_secs(15);
                loop {
                    editor.poll_language();
                    if editor
                        .language
                        .borrow()
                        .as_ref()
                        .unwrap()
                        .scopes_ready_at(&editor.text, editor.caret)
                    {
                        break;
                    }
                    assert!(Instant::now() < deadline, "{syntax} scope publication");
                    std::thread::park_timeout(Duration::from_millis(1));
                }
                input(&mut app, &enter, "\n");
                // Do not poll scopes between these keys: this is the stale-scope path.
                let costs: Vec<_> = keys
                    .iter()
                    .enumerate()
                    .map(|(i, key)| measure(|| input(&mut app, key, &word[i..i + 1])))
                    .collect();
                let actual = app.workspace.editor().text.to_string();
                assert_eq!(actual.matches(word).count(), count);
                assert_eq!(
                    actual.matches("    endpoint").count(),
                    count,
                    "{syntax} did not auto-indent"
                );
                if sample >= 5 {
                    samples.push(costs);
                }
                while app.workspace.editor().text.len_bytes() > text.len() {
                    app.run_named(None, "undo", &json!({}));
                }
                assert_eq!(app.workspace.editor().text.to_string(), text);
            }
            let emit = |name: &str, range: std::ops::Range<usize>| {
                let at = |f: fn(&(u64, u128)) -> u128| {
                    let mut totals: Vec<_> = samples
                        .iter()
                        .map(|row| row[range.clone()].iter().map(f).sum::<u128>())
                        .collect();
                    totals.sort_unstable();
                    totals[totals.len() / 2] as f64 / range.len() as f64
                };
                println!(
                    "R21_REVIEW {}",
                    json!({
                        "syntax":syntax,"carets":count,"case":name,
                        "instructions":at(|v| v.0 as u128), "wall_ns":at(|v| v.1),
                    })
                );
            };
            emit("first3", 0..3);
            emit("word", 0..keys.len());
        }
    }
}

#[test]
#[ignore = "paired startup retired instructions; no GUI"]
fn round21_keymap_startup_cost() {
    let source = include_str!("../resources/Packages/Default/Velocity.sublime-keymap");
    // Warm the measurement call, not regex or keymap loading.
    let _ = instructions();
    let (count, _) = measure(|| {
        let mut map = vel_config::keymap::Keymap::default();
        map.append(source).unwrap();
        std::hint::black_box(map);
    });
    println!("R21_STARTUP {count}");
}
