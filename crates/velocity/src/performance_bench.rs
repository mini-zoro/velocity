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

pub(crate) fn run(options: Options, mut clock: Clock, reporter: &Reporter) -> Result<(), String> {
    match options.mode {
        Mode::HeadlessPerfStartup => {
            let mut last = super::render_scenes::instructions();
            let mut phase = |clock: &mut Clock, name| -> Result<(), String> {
                let now = super::render_scenes::instructions();
                clock.mark(name);
                reporter.emit(json!({"kind":"perf_startup_phase","phase":name,
                    "instructions_since_last_phase":now.checked_sub(last),"process_instructions":now}))?;
                last = now;
                Ok(())
            };
            phase(&mut clock, "startup_probe_entered")?;
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
            phase(&mut clock, "text_loaded")?;
            let mut workspace = Workspace::new(text, options.data_dir.clone(), None, false)?;
            phase(&mut clock, "workspace_configured")?;
            if !workspace.editor().text.is_empty() {
                configure_syntax(
                    &mut workspace,
                    fixture_name.as_deref().unwrap_or("sample.rs"),
                )?;
            }
            phase(&mut clock, "startup_document_configured")?;
            let mut app = app_with_workspace(options, clock, reporter, workspace);
            phase(&mut app.clock, "app_initialized")?;
            let setup = Instant::now();
            black_box(app.scene());
            let ready = Instant::now();
            phase(&mut app.clock, "first_scene_ready")?;
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
        Mode::HeadlessPerfFile if options.memory_only => memory(options, clock, reporter)?,
        Mode::HeadlessPerfFile => file(options, clock, reporter)?,
        Mode::HeadlessPerfTyping | Mode::HeadlessPerfScroll | Mode::HeadlessPerfLarge => {
            replay_native(options, clock, reporter)?
        }
        Mode::HeadlessPerfHighlight => highlight(&options, reporter)?,
        Mode::HeadlessPerfFind => find(&options, reporter)?,
        Mode::HeadlessPerfGoto => goto(reporter)?,
        _ => unreachable!("not a comparison benchmark"),
    }
    reporter
        .emit(json!({"kind":"complete","complete":true,"gui_started":false,"gpu_started":false}))
}

fn thread_cpu_ms() -> f64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: This is a writable timespec; the clock measures the calling thread.
    let status = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) };
    assert_eq!(status, 0);
    time.tv_sec as f64 * 1000. + time.tv_nsec as f64 / 1_000_000.
}

fn replay_native(mut options: Options, clock: Clock, reporter: &Reporter) -> Result<(), String> {
    use crate::bench::{PANEL_ACTIONS, SCROLL_FRAMES, TYPING_INSERTIONS, WARMUP_FRAMES};
    let path = options.file.clone().ok_or("missing input file")?;
    options.mode = match options.mode {
        Mode::HeadlessPerfTyping => Mode::Typing,
        Mode::HeadlessPerfScroll => Mode::Scroll,
        Mode::HeadlessPerfLarge => Mode::LargeFiles,
        _ => unreachable!(),
    };
    let mode = options.mode;
    let text = if mode == Mode::LargeFiles {
        Text::new()
    } else {
        crate::load(Some(&path), options.lines)?.0
    };
    let mut workspace = Workspace::new(text, options.data_dir.clone(), None, false)?;
    if mode != Mode::LargeFiles {
        configure_syntax(&mut workspace, &path.to_string_lossy())?;
    }
    let mut app = app_with_workspace(options, clock, reporter, workspace);
    initialize_fonts(&mut app)?;
    black_box(app.scene());
    if mode == Mode::LargeFiles {
        reporter.rss("replay_empty", json!({}))?;
        app.workspace.open(FileLocation {
            path,
            line: 1,
            column: 1,
        })?;
        while app.workspace.tabs[app.workspace.active].loading.is_some()
            || app.workspace.tabs[app.workspace.active]
                .binary_loading
                .is_some()
        {
            app.workspace.poll();
            black_box(app.scene());
            std::thread::sleep(Duration::from_millis(1));
        }
        if app.workspace.editor().text.is_empty() && app.workspace.editor().binary.is_none() {
            return Err(format!(
                "fixture did not open: {:?}",
                app.workspace.messages
            ));
        }
        reporter.rss(
            "replay_loaded",
            json!({"bytes":app.workspace.editor().text.len_bytes()}),
        )?;
        for step in 0..PANEL_ACTIONS {
            app.workspace.poll();
            crate::large_bench::native_step(&mut app.workspace, &mut app.view, step)?;
            black_box(app.scene());
            // Native presents naturally give workers time between actions.
            std::thread::sleep(Duration::from_millis(16));
            if step % 10 == 0 {
                reporter.rss("replay_step", json!({"step":step}))?;
            }
        }
        settle(&mut app);
        return reporter.rss("replay_after_actions", json!({}));
    }
    wait_for_language(&mut app)?;
    black_box(app.scene());
    let editor = app.workspace.editor();
    reporter.emit(json!({"kind":"replay_ready","carets":editor.regions().len(),
        "lines":editor.text.len_lines(),"bytes":editor.text.len_bytes(),"logical_size":[app.view.size.width,app.view.size.height],
        "scale":2,"syntax":editor.language.borrow().as_ref().map(|l| l.name.clone()),"wrap":crate::view::motion_layout_options(editor).width != 0}))?;
    let mut totals = Vec::new();
    let mut edits = Vec::new();
    let mut scenes = Vec::new();
    if mode == Mode::Typing {
        for step in 0..TYPING_INSERTIONS {
            let wall = Instant::now();
            let start = thread_cpu_ms();
            let input = crate::bench::typing_text(step);
            crate::type_through_keymap(app.workspace.editor_mut(), input)?;
            let edited = thread_cpu_ms();
            app.changed_state(wall, true);
            let synced = thread_cpu_ms();
            black_box(app.scene());
            let finished = thread_cpu_ms();
            edits.push(edited - start);
            scenes.push(finished - synced);
            totals.push(finished - start);
            reporter.emit(json!({"kind":"replay_typing_sample","index":step,"newline":input=="\n",
                "thread_cpu_ms":finished-start,"command_cpu_ms":edited-start,
                "sync_cpu_ms":synced-edited,"scene_cpu_ms":finished-synced,"wall_ms":ms(wall.elapsed())}))?;
        }
    } else {
        prime_scroll_styles(&mut app, WARMUP_FRAMES + SCROLL_FRAMES)?;
        for step in 0..(WARMUP_FRAMES + SCROLL_FRAMES) {
            let start = thread_cpu_ms();
            app.benchmark_scroll_step();
            black_box(app.scene());
            let finished = thread_cpu_ms();
            if !app.exact_scene {
                return Err("scroll replay lost its exact highlighted viewport".into());
            }
            if step >= WARMUP_FRAMES {
                totals.push(finished - start);
            }
        }
    }
    reporter.emit(json!({"kind":"replay_summary","thread_cpu_ms":summarize(&totals),
        "command_cpu_ms":summarize(&edits),"scene_cpu_ms":summarize(&scenes),
        "view_profile":app.view.profile,
        "boundary":"native actions through CPU App::scene; excludes display, fonts rasterization, encoding and submission"}))
}

fn configure_syntax(workspace: &mut Workspace, name: &str) -> Result<(), String> {
    let first_line = workspace.editor().text.line(0).to_string();
    let set = &workspace.assets.syntaxes;
    let syntax = vel_highlight::detect_syntax(set, name, &first_line, None)
        .unwrap_or_else(|| set.find_syntax_plain_text())
        .name
        .clone();
    workspace.set_file_type(&syntax)
}

fn initialize_fonts(app: &mut App) -> Result<(), String> {
    app.fonts = Some(super::Fonts::load_named(
        &app.workspace.editor().preferences.font_face,
    )?);
    app.workspace.themes.ui.borrow_mut().density = 2.;
    app.initialize_view_metrics();
    Ok(())
}

fn wait_for_language(app: &mut App) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(120);
    while !app
        .workspace
        .editor()
        .language
        .borrow()
        .as_ref()
        .is_some_and(|l| l.ready() && l.highlight_complete())
    {
        if Instant::now() >= deadline {
            return Err("timed out waiting for native replay syntax and analysis".into());
        }
        app.workspace.poll();
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

fn prime_scroll_styles(app: &mut App, frames: usize) -> Result<(), String> {
    // Prime all rows the benchmark will visit, not only the first viewport.
    // Syntax delivery is asynchronous and isn't timed as main-thread scene CPU.
    let editor = app.workspace.editor();
    let end =
        (((frames as f64 * 20. + app.view.size.height) / app.view.line_height).ceil() as usize + 4)
            .min(editor.text.len_lines());
    let mut cell = editor.language.borrow_mut();
    let language = cell.as_mut().expect("scroll replay language");
    language.request_visible(std::iter::once(0..end).collect());
    let deadline = Instant::now() + Duration::from_secs(120);
    while !(0..end).all(|row| language.styled.get(&row).is_some_and(|l| l.exact)) {
        if Instant::now() >= deadline {
            return Err("timed out priming exact scroll styles".into());
        }
        language.poll();
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

fn app(options: Options, clock: Clock, reporter: &Reporter, text: Text) -> Result<App, String> {
    let workspace = Workspace::new(text, options.data_dir.clone(), None, false)?;
    Ok(app_with_workspace(options, clock, reporter, workspace))
}

fn app_with_workspace(
    options: Options,
    clock: Clock,
    reporter: &Reporter,
    workspace: Workspace,
) -> App {
    App::new(
        options,
        clock,
        reporter.clone(),
        workspace,
        Arc::new(AtomicBool::new(false)),
        Rc::new(RefCell::new(Sessions::new(None))),
    )
}

fn settle(app: &mut App) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(150) {
        app.workspace.poll();
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[derive(Default)]
#[repr(C)]
struct MallocStatistics {
    blocks_in_use: u32,
    size_in_use: usize,
    max_size_in_use: usize,
    size_allocated: usize,
}

fn malloc_statistics() -> serde_json::Value {
    unsafe extern "C" {
        fn malloc_zone_statistics(zone: *mut libc::c_void, stats: *mut MallocStatistics);
    }
    let mut stats = MallocStatistics::default();
    // SAFETY: NULL asks for all zones; stats has the layout of malloc_statistics_t.
    unsafe { malloc_zone_statistics(std::ptr::null_mut(), &mut stats) };
    json!({
        "malloc_live_bytes": stats.size_in_use,
        "malloc_reserved_bytes": stats.size_allocated,
        "malloc_reserved_minus_live_bytes": stats.size_allocated.saturating_sub(stats.size_in_use),
        "malloc_live_blocks": stats.blocks_in_use,
    })
}

fn memory(options: Options, clock: Clock, reporter: &Reporter) -> Result<(), String> {
    let path = options.file.clone().ok_or("missing input file")?;
    let deadline = Instant::now() + Duration::from_secs(options.timeout_seconds);
    let mut app = app(options, clock, reporter, Text::new())?;
    black_box(app.scene());
    reporter.rss("memory_empty", malloc_statistics())?;
    initialize_fonts(&mut app)?;
    black_box(app.scene());
    reporter.rss("memory_empty_fonts", malloc_statistics())?;
    app.workspace.open(FileLocation {
        path,
        line: 1,
        column: 1,
    })?;
    loop {
        app.workspace.poll();
        let tab = &app.workspace.tabs[app.workspace.active];
        if tab.load_failed || tab.editor.binary.is_some() {
            return Err("memory probe requires a readable text file".into());
        }
        if tab.loading.is_none() {
            let cell = tab.editor.language.borrow();
            let language = cell.as_ref().ok_or("no document language")?;
            if language.policy.highlighting != vel_largefile::HighlightMode::Full {
                return Err("memory probe requires full-file highlighting".into());
            }
            if !language.busy() && tab.editor.annotations.borrow().pending == 0 {
                break;
            }
        }
        if Instant::now() >= deadline {
            return Err("timed out waiting for full highlighting and analysis".into());
        }
        black_box(app.scene());
        std::thread::sleep(Duration::from_millis(10));
    }
    black_box(app.scene());
    // Publish the open-buffer index and completion data as the native loop
    // does. This isolated workspace has no project folders to crawl.
    app.panels.intel.poll(&mut app.workspace);
    app.panels.intel.flush_publications()?;
    settle(&mut app);
    let editor = app.workspace.editor();
    let mut details = malloc_statistics();
    details["text_bytes"] = json!(editor.text.len_bytes());
    details["lines"] = json!(editor.text.len_lines());
    reporter.rss("memory_highlighted", details)?;
    let cell = editor.language.borrow();
    let language = cell.as_ref().ok_or("missing highlight worker")?;
    let highlight = loop {
        if let Some(bytes) =
            language.measure_highlighter(deadline.saturating_duration_since(Instant::now()))
        {
            break bytes;
        }
        if Instant::now() >= deadline {
            return Err("timed out counting highlight allocations".into());
        }
    };
    let [symbols, analysis, minimap, words] = language
        .analysis
        .as_ref()
        .ok_or("missing analysis")?
        .memory_usage();
    let (layout_index, layout) = editor.layout.borrow().geometry_bytes();
    let rope_bytes = editor.text.estimated_storage_bytes();
    let line_index = editor.text.line_index_bytes();
    let (font_heap, font_mappings) = app
        .fonts
        .as_ref()
        .ok_or("missing fonts")?
        .file_storage_bytes();
    let git = &app.workspace.git.snapshots;
    let diff = editor.annotations.borrow();
    let mut details = json!({
        "text_rope": rope_bytes - line_index,
        "rope_line_index": line_index,
        "layout_line_index": layout_index,
        "layout_wrap": layout,
        "view_row_cache": app.view.row_cache_bytes(),
        "symbols": symbols,
        "analysis_geometry": analysis,
        "word_index": words,
        "minimap": minimap,
        "undo": editor.buffer.history_retained_bytes(),
        "git_repositories": git.len(),
        "git_indexed_paths": git.values().map(|repo| repo.files.len()).sum::<usize>(),
        "project_indexes": app.panels.intel.symbol_source().len(),
        "diff_publication": diff.current.as_ref().map_or(0, |d| {
            d.reference.estimated_retained_bytes(&editor.text)
                + size_of_val(d.hunks.as_ref())
                + d.inline_spans.values().map(|spans| size_of_val(spans.as_ref())).sum::<usize>()
        }),
        "font_face_heap": font_heap,
        "font_face_file_mappings": font_mappings,
        "glyph_atlas": 0,
        "renderer_buffers": 0,
    });
    details["highlight"] = json!({
        "line_records": highlight.records,
        "parser_states": highlight.parser_states,
        "parser_line_memo": highlight.parser_memo,
        "scope_ops": highlight.operations,
        "interned_scopes": highlight.interned_scopes,
        "scope_runs": highlight.scope_runs,
        "published_scope_line_table": language.scopes.as_ref().map(|s| s.line_table_bytes()),
        "style_spans": highlight.style_spans,
        "render_cache": highlight.render_cache,
    });
    reporter.emit(json!({
        "kind": "memory_breakdown",
        "method": "reachable allocation payload estimates; font mappings are not resident bytes",
        "details": details,
    }))?;
    Ok(())
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
                "visual_shapes": phases.visual_shapes, "visual_hits": phases.visual_hits,
                "interned_stacks": phases.interned_stacks,
                "checkpoint_scope_copies": phases.checkpoint_scope_copies,
                "retained_scope_snapshots": phases.retained_scope_snapshots
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::typing_text;

    fn fixture(mode: Mode, lines: usize, carets: usize, source: &str) -> App {
        let options = Options {
            mode,
            width: 1920,
            height: 1000,
            carets,
            ..Options::default()
        };
        let now = Instant::now();
        let clock = Clock::new(now, std::time::SystemTime::now());
        let mut workspace =
            Workspace::new(Text::from(source.repeat(lines).as_str()), None, None, false).unwrap();
        configure_syntax(&mut workspace, "fixture.c").unwrap();
        let reporter = Reporter::for_options(&Options::default(), now).unwrap();
        let mut app = app_with_workspace(options, clock, &reporter, workspace);
        initialize_fonts(&mut app).unwrap();
        black_box(app.scene());
        wait_for_language(&mut app).unwrap();
        app
    }

    #[test]
    fn stale_scope_enter_at_1000_carets_has_bounded_cpu() {
        let mut app = fixture(Mode::Typing, 151_639, 1000, "/* source */ int n;\n");
        let editor = app.workspace.editor_mut();
        let snapshot = editor
            .language
            .borrow()
            .as_ref()
            .unwrap()
            .scopes
            .as_ref()
            .unwrap()
            .clone();
        let mut samples = Vec::new();
        for step in 0..264 {
            let input = typing_text(step);
            let start = thread_cpu_ms();
            crate::type_through_keymap(editor, input).unwrap();
            if input == "\n" && step > 224 {
                samples.push(thread_cpu_ms() - start);
            }
        }
        assert!(
            editor
                .language
                .borrow()
                .as_ref()
                .unwrap()
                .scopes
                .as_ref()
                .is_none_or(|scopes| Arc::ptr_eq(scopes, &snapshot))
        );
        assert!(snapshot.version < editor.buffer.change_id());
        assert!(editor.published_scopes(editor.caret, false).is_some());
        assert_eq!(editor.regions().len(), 1000);
        assert_eq!(samples.len(), 5);
        // Main-thread CPU, not a scheduler deadline. The old search took
        // hundreds of ms here even with a warm grammar and no scene work.
        assert!(summarize(&samples).unwrap().p50 < 40., "{samples:?}");
    }

    #[test]
    fn highlighted_4k_scroll_scene_has_bounded_cpu() {
        let mut app = fixture(
            Mode::Scroll,
            3000,
            1,
            "static int f(int a, int b) { int v = (a * 25 + b) % 11; if (v < 0) return 0; return v + 1; } /* alpha beta gamma */\n",
        );
        prime_scroll_styles(&mut app, 120).unwrap();
        let mut samples = Vec::new();
        for step in 0..120 {
            let start = thread_cpu_ms();
            app.benchmark_scroll_step();
            let scene = black_box(app.scene());
            let spent = thread_cpu_ms() - start;
            assert!(app.exact_scene);
            assert!(
                scene
                    .layers
                    .iter()
                    .map(|layer| layer.text.len())
                    .sum::<usize>()
                    > 500
            );
            if step >= 20 {
                samples.push(spent);
            }
        }
        assert!(summarize(&samples).unwrap().p95 < 8., "{samples:?}");
    }
}
