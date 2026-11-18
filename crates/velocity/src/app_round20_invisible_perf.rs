use super::*;
use std::{cell::RefCell, rc::Rc};

#[test]
#[ignore = "paired 10k/200k line headless typing and paint timing"]
fn round20_invisible_key_timing() {
    let backspace = vel_config::keymap::Key::parse("backspace").unwrap();
    for lines in [10_000, 200_000] {
        let source = "alpha\u{200b} visible characters \u{202e} end of the line\n".repeat(lines);
        let mut samples = [Vec::new(), Vec::new()];
        for enabled in [false, true, true, false, false, true] {
            let now = Instant::now();
            let options = Options::default();
            let reporter = Reporter::for_options(&options, now).unwrap();
            let mut workspace =
                Workspace::new(vel_text::Text::from(source.as_str()), None, None, false).unwrap();
            workspace
                .settings
                .set(Layer::User, "index_files", json!(false));
            for (key, value) in [
                ("word_wrap", json!(true)),
                ("wrap_width", json!(36)),
                (
                    "draw_unicode_white_space",
                    json!(if enabled { "all" } else { "none" }),
                ),
                ("draw_unicode_bidi", json!(enabled)),
            ] {
                workspace.editor_mut().settings.set(Layer::View, key, value);
            }
            let mut a = App::new(
                options,
                Clock::new(now, std::time::SystemTime::now()),
                reporter,
                workspace,
                Arc::new(AtomicBool::new(false)),
                Rc::new(RefCell::new(crate::session::Sessions::new(None))),
            );
            let at = a.workspace.editor().text.line_to_byte(10) + 3;
            a.workspace.editor_mut().set_caret(at, false);
            let _ = a.view.scene(a.workspace.editor());
            let start = Instant::now();
            for _ in 0..80 {
                a.run_named(None, "insert", &json!({"characters":"x"}));
                std::hint::black_box(a.view.scene(a.workspace.editor()));
                assert!(a.dispatch_key_binding(None, &backspace, Instant::now(), 0..0));
                std::hint::black_box(a.view.scene(a.workspace.editor()));
            }
            samples[usize::from(enabled)].push(start.elapsed().as_secs_f64() * 1000. / 160.);
            assert_eq!(a.workspace.editor().caret, at);
        }
        samples
            .iter_mut()
            .for_each(|values| values.sort_by(f64::total_cmp));
        eprintln!(
            "{lines} lines: {:.3} ms/key off, {:.3} ms/key on ({:.3}x)",
            samples[0][1],
            samples[1][1],
            samples[1][1] / samples[0][1]
        );
    }
}

#[test]
#[ignore = "10,000 carets typing 400 keys on growing wrapped lines"]
fn round20_invisible_multicaret_timing() {
    fn median(keys: &[f64]) -> f64 {
        let mut sorted = keys.to_vec();
        sorted.sort_by(f64::total_cmp);
        (sorted[49] + sorted[50]) / 2.
    }
    let source = "one\u{200b} two three four five six seven eight nine ten\n".repeat(10_000);
    for enabled in [false, true, true, false] {
        let now = Instant::now();
        let options = Options::default();
        let reporter = Reporter::for_options(&options, now).unwrap();
        let mut workspace =
            Workspace::new(vel_text::Text::from(source.as_str()), None, None, false).unwrap();
        workspace
            .settings
            .set(Layer::User, "index_files", json!(false));
        let editor = workspace.editor_mut();
        editor.settings.set(Layer::View, "word_wrap", json!(true));
        editor.settings.set(Layer::View, "wrap_width", json!(36));
        editor.settings.set(
            Layer::View,
            "draw_unicode_white_space",
            json!(if enabled { "all" } else { "none" }),
        );
        let selection = vel_buffer::Selection::from_regions(
            (1..=10_000).map(|row| vel_buffer::Region::caret(editor.text.line_to_byte(row) - 1)),
        );
        editor.set_selection(selection);
        let mut a = App::new(
            options,
            Clock::new(now, std::time::SystemTime::now()),
            reporter,
            workspace,
            Arc::new(AtomicBool::new(false)),
            Rc::new(RefCell::new(crate::session::Sessions::new(None))),
        );
        a.view.apply_style(a.workspace.editor());
        let invocation = json!({"characters":"x"});
        let mut samples = Vec::new();
        for _ in 0..400 {
            let start = Instant::now();
            a.run_named(None, "insert", &invocation);
            a.view.apply_style(a.workspace.editor());
            samples.push(start.elapsed().as_secs_f64() * 1000.);
        }
        let first = median(&samples[..100]);
        let last = median(&samples[300..]);
        eprintln!(
            "10,000 carets, invisibles={enabled}: {first:.3} -> {last:.3} ms/key ({:.3}x)",
            last / first
        );
        assert_eq!(
            a.workspace.editor().text.len_bytes(),
            source.len() + 4_000_000
        );
    }
}
