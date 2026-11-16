//! Native AppKit editor demo with automatic measurements; never launches in tests.
mod annotation_commands;
mod annotations;
#[cfg(target_os = "macos")]
mod app;
mod bench;
mod binary;
mod build;
mod cli;
mod completion;
mod context_menu;
mod editing;
mod editor;
mod feature_bench;
#[cfg(test)]
mod feature_tests;
mod git_ui;
mod html_ui;
mod instance;
mod intelligence;
mod intelligence_bench;
mod language;
mod large;
mod large_bench;
#[cfg(test)]
mod large_tests;
mod layout;
mod mouse_select;
mod open_requests;
mod panels;
mod parse_bench;
mod release;
mod report;
mod review_bench;
#[cfg(test)]
mod review_tests;
mod review_ui;
mod session;
mod stats;
mod theme_bench;
#[cfg(test)]
mod theme_tests;
mod theme_ui;
#[cfg(test)]
mod user_grammar_tests;
mod view;
mod vintage;
#[cfg(test)]
mod vintage_tests;
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
#[cfg(target_os = "macos")]
struct NativeForwarder {
    relay: open_requests::Forwarder,
    options: Options,
    reporter: Reporter,
}
#[cfg(target_os = "macos")]
impl vel_platform::AppHandler for NativeForwarder {
    fn on_start(&mut self, window: &vel_platform::WindowHandle) -> Result<(), String> {
        window.close();
        Ok(())
    }
    fn on_open_paths(
        &mut self,
        _window: &vel_platform::WindowHandle,
        paths: &[std::path::PathBuf],
    ) {
        self.relay.enqueue(paths.to_vec(), Instant::now());
    }
    fn on_reopen(&mut self, _window: &vel_platform::WindowHandle) {
        self.relay.enqueue(vec![], Instant::now());
    }
    fn on_idle(&mut self, window: &vel_platform::WindowHandle) {
        match self.relay.poll(Instant::now()) {
            open_requests::ForwardPoll::Waiting => {}
            open_requests::ForwardPoll::Quit => window.quit(),
            open_requests::ForwardPoll::Failed(error) => {
                eprintln!("velocity: native open forwarding failed: {error}");
                window.quit();
            }
            open_requests::ForwardPoll::Primary { instance, paths } => {
                let result = (|| {
                    let data = self
                        .options
                        .data_dir
                        .clone()
                        .or_else(workspace::Workspace::default_data_dir);
                    let workspace = workspace::Workspace::new(Text::new(), data, None, true)?;
                    let mut options = self.options.clone();
                    options.forward_native_opens = false;
                    let mut app = app::App::new(
                        options,
                        Clock::new(Instant::now(), SystemTime::now()),
                        self.reporter.clone(),
                        workspace,
                        Arc::new(AtomicBool::new(false)),
                        std::rc::Rc::new(std::cell::RefCell::new(session::Sessions::new(None))),
                    );
                    app.take_over_native_primary(instance)?;
                    if let Err(error) = app.open_received_paths(&paths) {
                        eprintln!("velocity: {error}");
                    }
                    if let Some(home) = std::env::var_os("HOME") {
                        match release::CrashStartup::install(Path::new(&home)) {
                            Ok(crash) => app.set_crash(Some(crash)),
                            Err(error) => {
                                eprintln!("velocity: crash reporting unavailable: {error}")
                            }
                        }
                    }
                    window.new_window(WindowOptions::default(), app)?;
                    Ok::<_, String>(())
                })();
                if let Err(error) = result {
                    eprintln!("velocity: native primary takeover failed: {error}");
                    window.quit();
                }
            }
        }
    }
    fn on_frame(&mut self, _context: &mut vel_platform::FrameContext<'_>) -> Result<(), String> {
        Ok(())
    }
}
fn run(entered: Instant, wall: SystemTime) -> Result<(), String> {
    let Some(mut options) = Options::parse(std::env::args().skip(1))? else {
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
    let launch_services = options.launched_by_launch_services(parent_pid());
    let instance = if options.mode == Mode::Interactive {
        let mut request = instance::Request::new(&options, stdin.clone())?;
        if launch_services {
            // This startup probe may focus an existing window, but cannot substitute
            // for the Apple event that LaunchServices has yet to deliver to us.
            request.native_open = true;
            request.add = true;
            request.wait = false;
        }
        match instance::Instance::start(data_dir.as_deref(), &request) {
            Ok(instance::Start::Forwarded) if launch_services => {
                // Retain the native event recipient until its actual paths are relayed.
                // This temporary receiver must never restore or overwrite the primary session.
                options.forward_native_opens = true;
                None
            }
            Ok(instance::Start::Forwarded) => return Ok(()),
            Ok(instance::Start::Primary(instance)) => {
                // Finder/Dock launches must retain the process receiving Apple events.
                // See Options::launched_by_launch_services for the path-free heuristics.
                if std::env::var_os("VELOCITY_GUI_PROCESS").is_none() && !launch_services {
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
                    if let Some(output) = &options.output {
                        command.arg("--output").arg(output);
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
    let crash = if options.mode == Mode::Interactive && !options.forward_native_opens {
        match std::env::var_os("HOME")
            .ok_or_else(|| "home directory unavailable".to_owned())
            .and_then(|home| release::CrashStartup::install(Path::new(&home)))
        {
            Ok(crash) => Some(crash),
            Err(error) => {
                eprintln!("velocity: local crash reporting unavailable: {error}");
                None
            }
        }
    } else {
        None
    };
    let mut clock = Clock::new(entered, wall);
    let reporter = Reporter::for_options(&options, entered)?;
    let _watchdog = options
        .mode
        .automatic()
        .then(|| Watchdog::start(options.timeout_seconds, reporter.clone()));
    clock.attach(reporter.clone())?;
    clock.mark("instrumentation_ready");
    let result = (|| {
        #[cfg(target_os = "macos")]
        if options.forward_native_opens {
            // This recipient owns no editor/session/renderer. Close its native host
            // before orderFront and let the closed-window timer pump the relay.
            return Platform::run(NativeForwarder {
                relay: open_requests::Forwarder::new(data_dir.clone(), Instant::now()),
                options: options.clone(),
                reporter: reporter.clone(),
            });
        }
        if options.mode == Mode::HeadlessLargeFiles {
            return large_bench::run(&reporter, &options);
        }
        if options.mode == Mode::HeadlessTheme {
            return theme_bench::run(&reporter, &options);
        }
        if options.mode == Mode::HeadlessReviewHistory {
            return review_bench::history(&reporter, &options);
        }
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
        } else if matches!(
            options.mode,
            Mode::Interactive | Mode::Open | Mode::LargeFiles
        ) {
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
            let mut window = WindowOptions {
                title: "Velocity".into(),
                size: Size {
                    width: options.width as f64,
                    height: options.height as f64,
                },
            };
            let sessions = std::rc::Rc::new(std::cell::RefCell::new(session::Sessions::new(
                if !options.forward_native_opens
                    && (options.mode == Mode::Interactive || options.restore_session)
                {
                    data_dir.as_deref()
                } else {
                    None
                },
            )));
            let mut restored_ui = None;
            let restored = sessions.borrow().restored.clone();
            let reserved = sessions.borrow_mut().reserve_recovered();
            let selected = open_requests::restored_window(&restored.windows, &options);
            let first = selected.map(|index| restored.windows[index].clone());
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
                Err(e) if project.is_some() || first.is_some() => {
                    let mut workspace = workspace::Workspace::new(
                        Text::new(),
                        data_dir.clone(),
                        None,
                        options.mode == Mode::Interactive,
                    )?;
                    workspace.messages.push(format!(
                        "{}: {e}",
                        project.as_deref().map_or_else(
                            || "Session project".into(),
                            |path| path.display().to_string()
                        )
                    ));
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
            if matches!(
                options.mode,
                Mode::Interactive | Mode::Open | Mode::LargeFiles
            ) {
                let mut panels = panels::Panels::new(&workspace, false);
                let errors = open_requests::open_locations(
                    &mut workspace,
                    &mut panels,
                    options
                        .files
                        .iter()
                        .filter(|location| location.path != Path::new("-"))
                        .cloned(),
                );
                workspace.messages.extend(errors);
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
            window.title = workspace.window_title();
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
            if let Some(index) = selected {
                app.restore_session_id(reserved[index]);
            }
            // The selected recovery keeps its reserved live ID. Restore every other window
            // exactly once, including when the explicit project matched a non-first window.
            let (windows, ids) = restored
                .windows
                .into_iter()
                .zip(reserved)
                .enumerate()
                .filter(|(index, _)| Some(*index) != selected)
                .map(|(_, pair)| pair)
                .unzip();
            app.restore_remaining_reserved(windows, ids);
            app.set_instance(instance);
            app.set_crash(crash);
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
fn parent_pid() -> u32 {
    #[cfg(unix)]
    {
        // SAFETY: getppid has no arguments, borrowed pointers, or other preconditions.
        unsafe { libc::getppid() as u32 }
    }
    #[cfg(not(unix))]
    {
        0
    }
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
        for surface in &mut layer.surfaces {
            let (rect, clip) = surface.bounds_mut();
            shift(rect);
            shift(clip);
        }
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
