//! Production editor-to-scene probes. Setup/index construction is excluded from edit frames.
use crate::{
    cli::{FileLocation, Options},
    editor::Editor,
    report::Reporter,
    view::View,
    workspace::Workspace,
};
use std::{
    fs::File,
    io::{Seek, SeekFrom, Write},
    sync::Arc,
    time::{Duration, Instant},
};
use vel_config::json;
use vel_platform::Size;
use vel_text::Text;

pub fn wait_index(editor: &Editor, view: &mut View, row: usize) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        view.scene(editor);
        if editor.layout.borrow().long.ready(row) {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err("long-line index timed out".into());
        }
        std::thread::park_timeout(Duration::from_millis(1));
    }
}
pub fn run(reporter: &Reporter, options: &Options) -> Result<(), String> {
    let mut workspace = Workspace::new(Text::new(), options.data_dir.clone(), None, false)?;
    let mut view = View::new(
        Size {
            width: 1200.,
            height: 800.,
        },
        8.,
        12.,
    );
    if let Some(path) = &options.file {
        let at = Instant::now();
        workspace.open(FileLocation {
            path: path.clone(),
            line: 1,
            column: 1,
        })?;
        loop {
            workspace.poll();
            if workspace.editor().text.len_bytes() > 0 {
                let scene = view.scene(workspace.editor());
                std::hint::black_box(scene);
                reporter.emit(json!({"kind":"large_file_prefix_scene","elapsed_ms":at.elapsed().as_secs_f64()*1000.,"budget_ms":100,"source_bytes":std::fs::metadata(path).map_err(|e|e.to_string())?.len(),"loaded_bytes":workspace.editor().text.len_bytes(),"loading":workspace.tabs[workspace.active].loading.is_some(),"read_only":workspace.editor().read_only,"boundary":"Workspace::open -> poll -> first nonempty CPU scene; no native pixels"}))?;
                workspace.close(workspace.active)?;
                break;
            }
            if at.elapsed() > Duration::from_secs(120) {
                return Err("prefix timed out".into());
            }
            if workspace.tabs[workspace.active].loading.is_none() {
                return Err(workspace.messages.join("; "));
            }
            std::thread::park_timeout(Duration::from_millis(1));
        }
    }
    let mut editor = Editor::new(Text::from("a".repeat(50_000_000).as_str()));
    editor.attach_language(
        workspace.assets.clone(),
        "JavaScript",
        Arc::new(vel_color_scheme::ColorScheme::velocity_dark()),
    )?;
    editor.set_caret(25_000_000, false);
    view.reveal(&editor);
    wait_index(&editor, &mut view, 0)?;
    view.reveal(&editor);
    // Warm both the scene and command routes before timing.
    for _ in 0..20 {
        editor.run("insert", &json!({"characters":"x"}))?;
        view.scene(&editor);
    }
    let mut typing = Vec::new();
    for n in 0..500 {
        let at = Instant::now();
        editor.text_input(vel_platform::TextInput::Insert {
            text: if n % 2 == 0 { "x" } else { "界" }.into(),
            replacement: None,
        });
        view.reveal(&editor);
        std::hint::black_box(view.scene(&editor));
        typing.push(at.elapsed().as_secs_f64() * 1000.);
    }
    reporter.emit(json!({"kind":"long_line_typing_scene","bytes":50_000_000,"samples":typing.len(),"stats_ms":crate::stats::summarize(&typing),"budget_p95_ms":4,"boundary":"TextInput -> registry/Buffer transaction -> update_after_edit -> reveal -> CPU Scene; setup excluded"}))?;
    let mut scroll = Vec::new();
    for n in 0..600 {
        let at = Instant::now();
        view.scroll_x = ((n * 83) % 49_999_000) as f64 * view.cell;
        view.scroll(&editor, 16., 0.);
        std::hint::black_box(view.scene(&editor));
        scroll.push(at.elapsed().as_secs_f64() * 1000.);
    }
    reporter.emit(json!({"kind":"long_line_horizontal_scene","samples":scroll.len(),"stats_ms":crate::stats::summarize(&scroll),"budget_p95_ms":2}))?;
    reporter.rss("large_line", json!({}))?;
    drop(editor);
    let directory = tempfile::tempdir().map_err(|e| e.to_string())?;
    let path = directory.path().join("four-gb.bin");
    let mut file = File::create(&path).map_err(|e| e.to_string())?;
    file.set_len(4_000_000_003).map_err(|e| e.to_string())?;
    file.seek(SeekFrom::End(-3)).map_err(|e| e.to_string())?;
    file.write_all(b"END").map_err(|e| e.to_string())?;
    drop(file);
    workspace.open(FileLocation {
        path,
        line: 1,
        column: 1,
    })?;
    let deadline = Instant::now() + Duration::from_secs(120);
    while workspace.editor().binary.is_none() {
        workspace.poll();
        if Instant::now() > deadline {
            return Err("binary snapshot timed out".into());
        }
        std::thread::park_timeout(Duration::from_millis(1));
    }
    let mut hex = Vec::new();
    for n in 0..600 {
        let at = Instant::now();
        let offset = (n as u64 * 6_666_656).min(3_999_999_984) as usize;
        workspace.editor_mut().set_caret(offset, false);
        view.reveal(workspace.editor());
        std::hint::black_box(view.scene(workspace.editor()));
        hex.push(at.elapsed().as_secs_f64() * 1000.);
    }
    workspace
        .editor_mut()
        .run("goto_hex_offset", &json!({"text":"4000000000"}))?;
    workspace.editor_mut().anchor = 4_000_000_003;
    workspace.editor_mut().run("copy_hex", &json!({}))?;
    if workspace.editor().clipboard != "45 4e 44" {
        return Err("hex tail copy mismatch".into());
    }
    reporter.emit(json!({"kind":"binary_4gb_scene","samples":hex.len(),"stats_ms":crate::stats::summarize(&hex),"tail_copy":workspace.editor().clipboard,"read_only":workspace.editor().read_only,"boundary":"offset navigation -> reveal -> 16-byte HexView rows -> CPU scene; private snapshot setup excluded"}))?;
    reporter.emit(json!({"kind":"large_files_complete","gui_started":false,"gpu_started":false,"complete":true}))
}

pub fn native_step(workspace: &mut Workspace, view: &mut View, step: usize) -> Result<(), String> {
    if let Some(binary) = &workspace.editor().binary {
        let offset = binary.view.len_bytes().saturating_sub(1) * step as u64 / 100;
        workspace
            .editor_mut()
            .run("goto_hex_offset", &json!({"text":offset.to_string()}))?;
        view.reveal(workspace.editor());
    } else if workspace.editor().document_metrics.longest_line_bytes
        >= crate::large::LONG_LINE as u64
    {
        let e = workspace.editor_mut();
        if step == 0 {
            e.set_caret(e.text.len_bytes() / 2, false);
        }
        if step.is_multiple_of(2) {
            e.run("insert", &json!({"characters":"x"}))?;
            view.reveal(e);
        } else {
            view.scroll(e, 256., 0.);
        }
    } else {
        view.scroll_y =
            (workspace.editor().text.len_lines() as f64 * view.line_height) * step as f64 / 100.;
        view.clamp_scroll(workspace.editor());
    }
    Ok(())
}
