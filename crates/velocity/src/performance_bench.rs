//! Clean-room comparison workloads; none creates a window, drawable, or renderer.
use super::{App, Clock, Mode, Options, Reporter, Workspace, ms, summarize};
use crate::{cli::FileLocation, session::Sessions};
use serde_json::json;
use std::{
    cell::RefCell,
    hint::black_box,
    rc::Rc,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};
use vel_platform::TextInput;
use vel_text::Text;

pub(crate) fn run(options: Options, clock: Clock, reporter: &Reporter) -> Result<(), String> {
    match options.mode {
        Mode::HeadlessPerfStartup => {
            let fixture_name = options
                .file
                .as_deref()
                .map(|p| p.to_string_lossy().into_owned());
            let text = if options.file.is_some() || options.lines != 0 {
                crate::load(options.file.as_deref(), options.lines)?.0
            } else {
                Text::new()
            };
            let entered = clock.entered;
            let mut app = app(options, clock, reporter, text)?;
            if !app.workspace.editor().text.is_empty() {
                let editor = app.workspace.editor();
                let syntax = vel_highlight::detect_syntax(
                    &app.workspace.assets.syntaxes,
                    fixture_name.as_deref().unwrap_or("sample.rs"),
                    &editor.text.line(0).to_string(),
                    None,
                )
                .unwrap_or_else(|| app.workspace.assets.syntaxes.find_syntax_plain_text())
                .name
                .clone();
                app.workspace.set_file_type(&syntax)?;
            }
            let setup = Instant::now();
            black_box(app.scene());
            let ready = Instant::now();
            let first_scene_ms = ms(ready - entered);
            let text = &app.workspace.editor().text;
            reporter.emit(json!({
                "kind": "perf_startup", "first_scene_ms": first_scene_ms,
                "setup_ms": ms(setup - entered), "scene_ms": ms(ready - setup),
                "syntax_assets_ms": app.workspace.assets.startup_ms,
                "bytes": text.len_bytes(), "lines": text.len_lines(),
                "boundary": "main -> first App::scene return; no native window, drawable or scanout"
            }))?;
            settle(&mut app);
            reporter.rss("perf_settled", json!({}))?;
        }
        Mode::HeadlessPerfFile => file(options, clock, reporter)?,
        Mode::HeadlessPerfHighlight => highlight(&options, reporter)?,
        Mode::HeadlessPerfFind => find(&options, reporter)?,
        Mode::HeadlessPerfGoto => goto(reporter)?,
        _ => unreachable!("not a comparison benchmark"),
    }
    reporter
        .emit(json!({"kind":"complete","complete":true,"gui_started":false,"gpu_started":false}))
}

fn app(options: Options, clock: Clock, reporter: &Reporter, text: Text) -> Result<App, String> {
    let workspace = Workspace::new(text, options.data_dir.clone(), None, false)?;
    Ok(App::new(
        options,
        clock,
        reporter.clone(),
        workspace,
        Arc::new(AtomicBool::new(false)),
        Rc::new(RefCell::new(Sessions::new(None))),
    ))
}

fn settle(app: &mut App) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(150) {
        app.workspace.poll();
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn file(options: Options, clock: Clock, reporter: &Reporter) -> Result<(), String> {
    let path = options.file.clone().ok_or("missing input file")?;
    let source_bytes = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
    if source_bytes == 0 {
        return Err("file workload requires a nonempty text file".into());
    }
    let carets = options.carets;
    let mut app = app(options, clock, reporter, Text::new())?;
    black_box(app.scene()); // Excludes blank workspace and widget initialization from file open.
    let start = Instant::now();
    app.workspace.open(FileLocation {
        path,
        line: 1,
        column: 1,
    })?;
    let mut first = None;
    let full_load_ms;
    loop {
        app.workspace.poll();
        let sheet = &app.workspace.tabs[app.workspace.active];
        let loaded = sheet.editor.text.len_bytes();
        let complete = sheet.loading.is_none();
        let elapsed = ms(start.elapsed());
        if first.is_none() && loaded != 0 {
            black_box(app.scene());
            first = Some((ms(start.elapsed()), loaded));
        }
        if complete {
            full_load_ms = elapsed;
            break;
        }
        std::thread::sleep(Duration::from_micros(500));
    }
    let (first_scene_ms, first_scene_bytes) = first.ok_or("file did not load a text viewport")?;
    let editor = app.workspace.editor();
    let bytes = editor.text.len_bytes();
    let lines = editor.text.len_lines();
    let longest_line_bytes = editor.document_metrics.longest_line_bytes;
    let read_only = editor.read_only;
    if bytes as u64 != source_bytes {
        return Err(format!(
            "expected {source_bytes} UTF-8 bytes, loaded {bytes}"
        ));
    }
    reporter.emit(json!({
        "kind": "perf_file", "first_scene_ms": first_scene_ms, "first_scene_bytes": first_scene_bytes,
        "full_load_ms": full_load_ms, "bytes": bytes, "lines": lines,
        "longest_line_bytes": longest_line_bytes, "read_only": read_only,
        "boundary": "warm workspace -> async open -> nonempty App::scene and complete text installation"
    }))?;
    reporter.rss("perf_after_load", json!({}))?;

    let single_line = lines == 1 && longest_line_bytes >= 10_000;
    if single_line {
        app.workspace.editor_mut().settings.set(
            vel_config::settings::Layer::View,
            "word_wrap",
            json!(false),
        );
    }
    // Exercise end, start and middle; do not claim that every line has been highlighted.
    for row in [lines - 1, 0, lines / 2] {
        let editor = app.workspace.editor_mut();
        editor.set_caret(editor.text.line_to_byte(row), false);
        app.view.reveal(editor);
        black_box(app.scene());
    }
    if single_line {
        let editor = app.workspace.editor_mut();
        editor.set_caret(bytes / 2, false); // ASCII comparison fixtures.
        app.view.reveal(editor);
        if longest_line_bytes >= crate::large::LONG_LINE as u64 {
            crate::large_bench::wait_index(editor, &mut app.view, 0)?;
            app.view.reveal(editor);
        }
    }
    if carets > 1 {
        crate::install_carets(app.workspace.editor_mut(), carets);
    }
    settle(&mut app);
    let mut typing = Vec::new();
    let mut scroll = Vec::new();
    let before = app.workspace.editor().buffer.change_id();
    if !read_only {
        for n in 0..220 {
            let at = Instant::now();
            app.workspace.editor_mut().text_input(TextInput::Insert {
                text: "x".into(),
                replacement: None,
            });
            app.changed_state(at, true);
            black_box(app.scene());
            if n >= 20 {
                typing.push(ms(at.elapsed()));
            }
        }
        if app.workspace.editor().buffer.change_id() == before {
            return Err("typing fixture did not edit the buffer".into());
        }
    }
    for n in 0..220 {
        let at = Instant::now();
        let editor = app.workspace.editor();
        if single_line {
            app.view.scroll_x = (bytes - 500) as f64 * (n as f64 / 219.) * app.view.cell;
            app.view.scroll(editor, 16., 0.);
        } else {
            app.view.scroll(editor, 0., app.view.line_height * 3.);
        }
        black_box(app.scene());
        if n >= 20 {
            scroll.push(ms(at.elapsed()));
        }
    }
    reporter.emit(json!({
        "kind": "perf_input", "typing_ms": summarize(&typing), "scroll_ms": summarize(&scroll),
        "typing_samples_ms": typing, "scroll_samples_ms": scroll,
        "carets": carets, "axis": if single_line {"horizontal"} else {"vertical"},
        "boundary": "synthetic TextInput or scroll to App::scene; excludes native dispatch/submit/present/display",
    }))?;
    settle(&mut app);
    reporter.rss("perf_settled", json!({}))
}

fn highlight(options: &Options, reporter: &Reporter) -> Result<(), String> {
    let path = options.file.as_deref().ok_or("missing input file")?;
    let text = crate::load(Some(path), 0)?.0;
    let assets = crate::language::Assets::bundled()?;
    let syntax = assets
        .syntaxes
        .find_syntax_for_file(path)
        .map_err(|e| e.to_string())?
        .ok_or("workload has no bundled syntax")?;
    let scheme = Arc::new(vel_color_scheme::ColorScheme::velocity_dark());
    for pass in ["first", "warm"] {
        #[cfg(feature = "highlight-profile")]
        vel_syntax::matcher_profile::start();
        let at = Instant::now();
        let mut engine = vel_highlight::DocumentHighlighter::new(
            text.clone(),
            vel_buffer::ChangeId(0),
            assets.syntaxes.clone(),
            syntax,
            scheme.clone(),
        );
        engine.finish().map_err(|e| e.to_string())?;
        let elapsed = at.elapsed();
        #[cfg(feature = "highlight-profile")]
        {
            let matches = vel_syntax::matcher_profile::finish();
            let phases = engine.profile();
            reporter.emit(json!({
                "kind": "perf_highlight_profile", "pass": pass, "matcher": matches,
                "phase_ms": {"input": ms(phases.input), "parse": ms(phases.parse),
                    "render": ms(phases.render), "checkpoint": ms(phases.checkpoint)},
                "render_hits": phases.render_hits, "render_queries": phases.render_queries,
                "render_entries": phases.render_entries, "built_spans": phases.built_spans,
                "interned_stacks": phases.interned_stacks,
                "checkpoint_scope_copies": phases.checkpoint_scope_copies
            }))?;
        }
        let lines = engine.parsed_through();
        if lines != text.len_lines() {
            return Err("highlight did not finish at EOF".into());
        }
        reporter.emit(json!({
            "kind": "perf_highlight", "pass": pass, "ms": ms(elapsed),
            "bytes": text.len_bytes(), "lines": lines,
            "lines_per_second": lines as f64 / elapsed.as_secs_f64(),
            "grammar": syntax.name, "rewinds": engine.stats().rewinds,
            "boundary": "new DocumentHighlighter + full parse, scopes, theme spans, checkpoints; excludes file/grammar loading"
        }))?;
        black_box(&engine);
    }
    Ok(())
}

fn find(options: &Options, reporter: &Reporter) -> Result<(), String> {
    use vel_search::{files, project::Folder, text};
    let folders = options
        .files
        .iter()
        .map(|root| {
            let path = root.path.canonicalize().map_err(|e| e.to_string())?;
            if !path.is_dir() {
                return Err(format!("not a directory: {}", path.display()));
            }
            Ok(Folder {
                name: path.display().to_string(),
                path,
                rules: Default::default(),
                follow_links: false,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let request = files::Request {
        search: text::Search::new(
            "pub fn",
            text::Options {
                case_sensitive: true,
                ..Default::default()
            },
        )?,
        scope: files::Where::parse("", &folders[0].path)?,
        folders,
        open: Vec::new(),
        context: 0,
    };
    let mut results = files::Results::ordered(&request.folders);
    let start = Instant::now();
    let task = files::Task::start(request)?;
    let mut failures = 0;
    let mut hit_lines = 0;
    let mut first_failure = None;
    loop {
        match task.events.recv().map_err(|e| e.to_string())? {
            files::Event::Line(line) => {
                hit_lines += 1;
                black_box(results.push(line));
            }
            files::Event::Error(e) => {
                failures += 1;
                first_failure.get_or_insert(e);
            }
            files::Event::Done { matches, files } => {
                reporter.emit(json!({
                    "kind": "perf_find", "ms": ms(start.elapsed()), "searched_files": files,
                    "matches": matches, "matched_lines": hit_lines, "matched_files": results.matched_files(),
                    "errors": failures, "first_error": first_failure, "query": "pub fn",
                    "boundary": "worker start -> traversal/decoding/search and ordered accumulation -> Done; no result UI"
                }))?;
                break;
            }
        }
    }
    Ok(())
}

fn goto(reporter: &Reporter) -> Result<(), String> {
    use vel_search::fuzzy::{Candidate, Matcher};
    let names = [
        "main", "lib", "runtime", "stream", "reader", "parser", "index", "worker",
    ];
    let at = Instant::now();
    let paths: Vec<_> = (0..100_000)
        .map(|n| {
            Candidate::new(format!(
                "src/{}/crate_{:04}/{}_{}.rs",
                if n % 3 == 0 { "tokio" } else { "vendor" },
                n / 64,
                names[n % names.len()],
                n
            ))
        })
        .collect();
    let prepare_ms = ms(at.elapsed());
    for query in ["srcmainrs", "tokioruntime"] {
        let at = Instant::now();
        let hits = black_box(Matcher::default().rank(&paths, query, 50));
        let elapsed = ms(at.elapsed());
        if hits.len() != 50 {
            return Err("not enough hits in Goto Anything fixture".into());
        }
        reporter.emit(json!({
            "kind": "perf_goto", "ms": elapsed, "prepare_ms": prepare_ms,
            "query": query, "paths": paths.len(), "hits": hits.len(),
            "boundary": "fresh production matcher, prepared synthetic paths -> ordered top 50; no directory walk or UI"
        }))?;
    }
    Ok(())
}
