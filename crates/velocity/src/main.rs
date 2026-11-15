//! Native AppKit editor demo with automatic measurements; never launches in tests.
#[cfg(target_os = "macos")]
mod app;
mod bench;
mod build;
mod cli;
mod completion;
mod editing;
mod editor;
mod feature_bench;
#[cfg(test)]
mod feature_tests;
mod instance;
mod intelligence;
mod intelligence_bench;
mod language;
mod layout;
mod panels;
mod parse_bench;
mod report;
mod session;
mod stats;
mod view;
mod workbench;
mod workbench_bench;
#[cfg(test)]
mod workbench_tests;
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
    let data_dir = options
        .data_dir
        .clone()
        .or_else(workspace::Workspace::default_data_dir);
    let mut stdin = None;
    if options.mode == Mode::Interactive && options.files.iter().any(|f| f.path == Path::new("-")) {
        let mut input = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
            .map_err(|e| e.to_string())?;
        stdin = Some(input);
    }
    let instance = if options.mode == Mode::Interactive {
        let request = instance::Request::new(&options, stdin.clone())?;
        match instance::Instance::start(data_dir.as_deref(), &request) {
            Ok(instance::Start::Forwarded) => return Ok(()),
            Ok(instance::Start::Primary(instance)) => {
                if std::env::var_os("VELOCITY_GUI_PROCESS").is_none() {
                    drop(instance);
                    let mut command = std::process::Command::new(
                        std::env::current_exe().map_err(|e| e.to_string())?,
                    );
                    command
                        .env("VELOCITY_GUI_PROCESS", "1")
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null());
                    if let Some(data) = &data_dir {
                        command.arg("--data-dir").arg(data);
                    }
                    command.args([
                        "--width",
                        &options.width.to_string(),
                        "--height",
                        &options.height.to_string(),
                    ]);
                    if options.log_input {
                        command.arg("--log-input");
                    }
                    let mut child = command.spawn().map_err(|e| e.to_string())?;
                    let at = Instant::now();
                    loop {
                        if instance::Instance::forward_existing(data_dir.as_deref(), &request)? {
                            return Ok(());
                        }
                        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                            return Err(format!(
                                "native application exited before accepting files: {status}"
                            ));
                        }
                        if at.elapsed() > std::time::Duration::from_secs(30) {
                            return Err("native application did not open its local socket".into());
                        }
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                }
                Some(instance)
            }
            Err(e) => {
                eprintln!("velocity: single-instance forwarding unavailable: {e}");
                None
            }
        }
    } else {
        None
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
        if options.mode == Mode::HeadlessIntelligence {
            return intelligence_bench::run(&reporter, &options);
        }
        #[cfg(target_os = "macos")]
        if options.mode == Mode::Intelligence {
            vel_platform::display_status().check()?;
            let done = Arc::new(AtomicBool::new(false));
            let native = intelligence_bench::Native::new(&options, reporter.clone(), done.clone())?;
            Platform::run_with_options(
                WindowOptions {
                    title: "Velocity Intelligence Measurement".into(),
                    size: Size {
                        width: options.width as f64,
                        height: options.height as f64,
                    },
                },
                native,
            )?;
            if !done.load(Ordering::Acquire) {
                return Err("intelligence benchmark ended early".into());
            }
            return Ok(());
        }
        if options.mode == Mode::HeadlessParse {
            return parse_bench::run(&reporter, &options);
        }
        if options.mode == Mode::HeadlessFeatures {
            return feature_bench::run(&reporter, &options);
        }
        if options.mode == Mode::HeadlessWorkbench {
            return workbench_bench::run(&reporter, &options);
        }
        if options.mode != Mode::Headless {
            vel_platform::display_status().check()?;
            clock.mark("display_preflight_passed");
        }
        let (text, source) = if options.restore_session {
            (Text::new(), "restored session".into())
        } else if matches!(options.mode, Mode::Interactive | Mode::Open) {
            if let Some(input) = &stdin {
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
            let sessions = std::rc::Rc::new(std::cell::RefCell::new(session::Sessions::new(
                if options.mode == Mode::Interactive || options.restore_session {
                    data_dir.as_deref()
                } else {
                    None
                },
            )));
            let mut restored_ui = None;
            let restored = sessions.borrow().restored.clone();
            let first = if (options.mode == Mode::Interactive || options.restore_session)
                && options.project.is_none()
                && options.files.is_empty()
            {
                restored.windows.first().cloned()
            } else {
                None
            };
            let project = options.project.clone().or_else(|| {
                first
                    .as_ref()
                    .and_then(|w| w.project.as_ref())
                    .filter(|p| p.is_file())
                    .cloned()
            });
            let initial_workspace = workspace::Workspace::new(
                if stdin.is_some() { Text::new() } else { text },
                data_dir.clone(),
                project.clone(),
                options.mode == Mode::Interactive,
            );
            let mut workspace = match initial_workspace {
                Ok(workspace) => workspace,
                Err(e) if options.project.is_none() && first.is_some() => {
                    let mut workspace = workspace::Workspace::new(
                        Text::new(),
                        data_dir.clone(),
                        None,
                        options.mode == Mode::Interactive,
                    )?;
                    workspace.messages.push(format!("Restored project: {e}"));
                    workspace
                }
                Err(e) => return Err(e),
            };
            workspace.recent_files = restored.recent_files.clone();
            workspace.recent_folders = restored.recent_folders.clone();
            workspace.recent_projects = restored.recent_projects.clone();
            if let Some(path) = workspace.project_path().map(Path::to_owned) {
                workbench::remember(&mut workspace.recent_projects, path);
            }
            workspace
                .messages
                .append(&mut sessions.borrow_mut().messages);
            if let Some(window) = &first {
                if let Err(e) = workspace.restore_window(window, &restored.documents) {
                    workspace.messages.push(format!("Session restore: {e}"));
                } else {
                    restored_ui = Some(window.clone());
                }
            } else if options.mode == Mode::Interactive
                && let Some(project) = &options.project
            {
                let recovery = vel_session::load(&vel_session::project::workspace_path(project));
                workspace.messages.extend(recovery.messages);
                if let Some(window) = recovery.session.windows.first() {
                    if let Err(e) = workspace.restore_window(window, &recovery.session.documents) {
                        workspace.messages.push(e);
                    } else {
                        restored_ui = Some(window.clone());
                    }
                }
            }
            if let Some(input) = &stdin {
                workspace
                    .editor_mut()
                    .run("insert", &json!({"characters":input}))?;
            }
            if matches!(options.mode, Mode::Interactive | Mode::Open) {
                for location in &options.files {
                    if location.path != Path::new("-") {
                        if location.path.is_dir() {
                            let mut panels = panels::Panels::new(&workspace, false);
                            panels.add_folder(&mut workspace, location.path.clone())?;
                        } else {
                            workspace.open(location.clone())?;
                        }
                    }
                }
            } else if !options.restore_session {
                let path = options
                    .file
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "sample.rs".into());
                let syntax = vel_highlight::detect_syntax(
                    &workspace.assets.syntaxes,
                    &path,
                    &workspace.editor().text.line(0).to_string(),
                    None,
                )
                .unwrap_or_else(|| workspace.assets.syntaxes.find_syntax_plain_text())
                .name
                .clone();
                workspace.set_file_type(&syntax)?;
            }
            if options.groups > 1 {
                workbench::configure_groups(&mut workspace, options.groups)?;
            }
            let mut app = app::App::new(
                options.clone(),
                clock,
                reporter.clone(),
                workspace,
                Arc::clone(&done),
                sessions,
            );
            if let Some(window) = &restored_ui {
                app.restore_ui(window);
            }
            if first.is_some() {
                app.restore_remaining(restored.windows.iter().skip(1).cloned().collect());
            }
            app.set_instance(instance);
            Platform::run_with_options(window, app)?;
            if options.mode == Mode::Interactive {
                done.store(true, Ordering::Release);
            }
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
fn app_scene_translate(scene: &mut vel_render::scene::Scene, x: f64, y: f64) {
    let shift = |r: &mut vel_platform::Rect| {
        r.x += x;
        r.y += y;
    };
    for layer in &mut scene.layers {
        for rect in &mut layer.rects {
            shift(&mut rect.rect);
            shift(&mut rect.clip);
        }
        for shadow in &mut layer.shadows {
            shift(&mut shadow.rect);
            shift(&mut shadow.clip);
        }
        for underline in &mut layer.underlines {
            shift(&mut underline.rect);
            shift(&mut underline.clip);
        }
        for text in &mut layer.text {
            text.x += x as f32;
            text.baseline += y as f32;
            shift(&mut text.clip);
        }
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
