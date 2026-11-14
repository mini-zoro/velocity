//! Native AppKit editor demo with automatic measurements; never launches in tests.
#[cfg(target_os = "macos")]
mod app;
mod bench;
mod cli;
mod editor;
mod panels;
mod report;
mod stats;
mod view;
mod workspace;
use cli::{Mode, Options};
use report::{Clock, Reporter, Watchdog, ms};
use serde_json::json;
use std::{
    io::{BufRead, BufReader},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Instant, SystemTime},
};
use vel_platform::{Platform, Size, WindowOptions};
use vel_text::Text;

fn main() {
    let entered = Instant::now();
    let wall = SystemTime::now();
    if let Err(e) = run(entered, wall) {
        eprintln!("velocity: {e}");
        std::process::exit(1);
    }
}
fn run(entered: Instant, wall: SystemTime) -> Result<(), String> {
    let Some(options) = Options::parse(std::env::args().skip(1))? else {
        print!("{}", cli::HELP);
        return Ok(());
    };
    let mut clock = Clock::new(entered, wall);
    let reporter = Reporter::new(options.mode, options.output.as_deref(), entered)?;
    let _watchdog = options
        .mode
        .automatic()
        .then(|| Watchdog::start(options.timeout_seconds, reporter.clone()));
    clock.attach(reporter.clone())?;
    clock.mark("instrumentation_ready");
    let result = (|| {
        if options.mode != Mode::Headless {
            vel_platform::display_status().check()?;
            clock.mark("display_preflight_passed");
        }
        let (text, source) = if matches!(options.mode, Mode::Interactive | Mode::Open) {
            if options.files.iter().any(|f| f.path == Path::new("-")) {
                let mut input = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
                    .map_err(|e| e.to_string())?;
                (Text::from(input.as_str()), "stdin".into())
            } else {
                (
                    Text::new(),
                    "empty document; files opened through vel-fileio".into(),
                )
            }
        } else {
            load(options.file.as_deref(), options.lines)?
        };
        clock.mark("text_loaded");
        reporter.emit(json!({"kind":"document","source":source,"bytes":text.len_bytes(),"lines":text.len_lines(),"line_limit":options.lines,"engine":"vel_text::Text","mode_automatic":options.mode.automatic()}))?;
        reporter.rss(
            "after_load",
            json!({"bytes":text.len_bytes(),"lines":text.len_lines()}),
        )?;
        if options.mode == Mode::Headless {
            return headless(&reporter, text, options.carets);
        }
        let done = Arc::new(AtomicBool::new(false));
        #[cfg(target_os = "macos")]
        {
            let window = WindowOptions {
                title: options.file.as_ref().map_or_else(
                    || "Velocity · native AppKit".into(),
                    |p| format!("{} — Velocity", p.display()),
                ),
                size: Size {
                    width: options.width as f64,
                    height: options.height as f64,
                },
            };
            let mut workspace = workspace::Workspace::new(
                text,
                options
                    .data_dir
                    .clone()
                    .or_else(workspace::Workspace::default_data_dir),
                options.project.clone(),
                options.mode == Mode::Interactive,
            )?;
            if matches!(options.mode, Mode::Interactive | Mode::Open) {
                for location in &options.files {
                    if location.path != Path::new("-") {
                        workspace.open(location.clone())?;
                    }
                }
            }
            let app = app::App::new(
                options.clone(),
                clock,
                reporter.clone(),
                workspace,
                Arc::clone(&done),
            );
            Platform::run_with_options(window, app)?;
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (&clock, &text);
            Platform::run_with_options(WindowOptions::default(), Unsupported)?;
        }
        if !done.load(Ordering::Acquire) {
            return Err("native loop returned before completion".into());
        }
        Ok(())
    })();
    if let Err(e) = &result {
        let _ = reporter.emit(json!({"kind":"error","stage":"run","message":e,"complete":false}));
    }
    result
}
fn load(path: Option<&Path>, limit: usize) -> Result<(Text, String), String> {
    if let Some(path) = path {
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let text = if limit == 0 {
            Text::from_reader(file).map_err(|e| e.to_string())?
        } else {
            let mut content = String::new();
            let mut reader = BufReader::new(file);
            for _ in 0..limit {
                if reader.read_line(&mut content).map_err(|e| e.to_string())? == 0 {
                    break;
                }
            }
            Text::from(content.as_str())
        };
        Ok((text, path.display().to_string()))
    } else {
        let count = if limit == 0 { 5000 } else { limit };
        let sample: Vec<_> = include_str!("../fixtures/sample.rs").lines().collect();
        let content = sample
            .iter()
            .cycle()
            .take(count)
            .copied()
            .collect::<Vec<_>>()
            .join("\n");
        Ok((
            Text::from(content.as_str()),
            "bundled sample (repeated to 5000 lines by default)".into(),
        ))
    }
}
fn headless(reporter: &Reporter, text: Text, carets: usize) -> Result<(), String> {
    let mut editor = editor::Editor::new(text);
    let mut view = view::View::new(
        Size {
            width: 1200.,
            height: 800.,
        },
        7.8,
        12.,
    );
    install_carets(&mut editor, carets);
    let mut samples = Vec::new();
    for i in 0..50 {
        let at = Instant::now();
        type_through_keymap(
            &mut editor,
            match i % 8 {
                0 => "你",
                1 => "🦀",
                2 => "é",
                3 => "\n",
                _ => "x",
            },
        )?;
        let scene = view.scene(&editor);
        std::hint::black_box(scene);
        samples.push(ms(at.elapsed()));
    }
    let before = editor.text.clone();
    editor.command(vel_platform::Command::Undo);
    editor.command(vel_platform::Command::Redo);
    if before != editor.text {
        return Err("headless undo/redo mismatch".into());
    }
    reporter.emit(json!({"kind":"headless_summary","edit_and_scene_cpu_ms":stats::summarize(&samples),"samples":samples.len(),"carets":carets,"path":"keymap -> command -> vel-buffer -> CPU scene","gui_started":false,"gpu_started":false,"text_bytes":editor.text.len_bytes(),"text_lines":editor.text.len_lines()}))?;
    reporter.rss("headless_after_edits", json!({}))?;
    reporter.emit(json!({"kind":"complete","complete":true}))
}
fn install_carets(editor: &mut editor::Editor, count: usize) {
    use vel_buffer::{Region, Selection};
    if count == 1000 && editor.text.len_lines() < count {
        editor.buffer = vel_buffer::Buffer::new("workload\n".repeat(count));
        editor.id = editor.buffer.new_view();
        editor.sync();
    }
    let selections =
        Selection::from_regions((0..count).map(|row| Region::caret(editor.text.line_to_byte(row))));
    editor.set_selection(selections);
}
fn type_through_keymap(editor: &mut editor::Editor, text: &str) -> Result<(), String> {
    for character in text.chars() {
        let name = match character {
            '\n' => "enter".into(),
            '\t' => "tab".into(),
            c => c.to_string(),
        };
        let key = vel_config::keymap::Key::parse(&name)?;
        if !editor.key(&key)? {
            editor.run("insert", &json!({"characters":character.to_string()}))?;
        }
    }
    Ok(())
}
#[cfg(not(target_os = "macos"))]
struct Unsupported;
#[cfg(not(target_os = "macos"))]
impl vel_platform::AppHandler for Unsupported {
    fn on_frame(&mut self, _: &mut vel_platform::FrameContext<'_>) -> Result<(), String> {
        unreachable!()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_sample_is_self_contained_and_exactly_five_thousand_lines() {
        let (text, _) = load(None, 0).unwrap();
        assert_eq!(text.len_lines(), 5000);
        assert!(text.to_string().contains("你好"));
        assert_eq!(load(None, 3).unwrap().0.len_lines(), 3);
    }
    #[test]
    fn explicit_missing_file_is_an_error() {
        assert!(load(Some(Path::new("/nonexistent/velocity-input.rs")), 0).is_err());
    }
}
