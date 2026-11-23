//! Repeatable device-free workbench timing. No native handles, fonts or GPU initialization.
use crate::{
    cli::Options,
    report::{Reporter, ms},
    stats,
    view::View,
    workspace::Workspace,
};
use serde_json::json;
use std::time::{Duration, Instant};
use vel_buffer::{Buffer, ops};
use vel_platform::Size;
use vel_session::{Document, History, Session, Sheet, ViewState, Window};
use vel_text::Text;

pub fn run(reporter: &Reporter, options: &Options) -> Result<(), String> {
    let fixture = tempfile::Builder::new()
        .prefix("velocity-workbench-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    let root = fixture.path();
    let data = options
        .data_dir
        .clone()
        .unwrap_or_else(|| root.join("fresh-profile"));
    let fixtures = root.join("files");
    std::fs::create_dir_all(&fixtures).map_err(|e| e.to_string())?;
    let mut documents = Vec::new();
    let mut sheets = Vec::new();
    let mut workbench = vel_ui::workbench::Workbench::default();
    for i in 0..50 {
        let path = fixtures.join(format!("file-{i}.txt"));
        let content = if i >= 40 {
            "large line\n".repeat(1_000_000)
        } else {
            "small line\n".repeat(200)
        };
        std::fs::write(&path, &content).map_err(|e| e.to_string())?;
        let disk = vel_session::DiskStamp::read(&path);
        let history = if i < 5 {
            let mut buffer = Buffer::new(content.as_str());
            let id = buffer.new_view();
            ops::paste(&mut buffer, id, "unsaved 你好\n");
            Some(History::capture(&buffer))
        } else {
            None
        };
        let id = i as u64 + 1;
        documents.push(Document {
            id,
            path: Some(path),
            disk,
            disk_content: None,
            saved_in_effective_encoding: false,
            effective_encoding: None,
            encoding: "UTF-8".into(),
            encoding_chosen: false,
            preferred_encoding: None,
            encoding_display_name: None,
            bom: false,
            line_ending: "LF".into(),
            dirty: i < 5,
            history,
        });
        sheets.push(Sheet {
            settings: None,
            html: None,
            id,
            document: id,
            view: ViewState::default(),
            syntax: None,
            transient: false,
            name: None,
        });
        workbench.groups[0].add(id, false);
    }
    workbench.groups[0].click(1, false, false);
    let session = Session {
        documents,
        windows: vec![Window {
            sheets,
            workbench,
            sidebar_visible: Some(true),
            ..Window::default()
        }],
        ..Session::default()
    };
    let path = data.join("Sessions/Session.sublime-session");
    let writer = vel_session::Writer::new(path.clone(), Duration::from_millis(400))?;
    let mut saves = Vec::new();
    for _ in 0..10 {
        writer.changed(session.clone())?;
        saves.push(ms(writer.flush()?));
    }
    reporter.emit(json!({"kind":"session_save","files":50,"large_files":10,"large_file_bytes":11_000_000,"dirty_files":5,"samples_ms":saves,"save_worker_ms":stats::summarize(&saves),"budget_ms":20,"boundary":"worker encode + validate + atomic write + file/directory fsync; snapshot construction and fixture IO excluded","data_dir":data,"session_file":path,"gui_started":false,"gpu_started":false}))?;
    let at = Instant::now();
    let recovered = vel_session::load(&path);
    if !recovered.messages.is_empty() {
        return Err(recovered.messages.join("; "));
    }
    let mut workspace = Workspace::new(Text::new(), Some(data.clone()), None, false)?;
    workspace.restore_window(&recovered.session.windows[0], &recovered.session.documents)?;
    let loaded = workspace.tabs.iter().filter(|t| t.lazy.is_none()).count();
    let restored_ms = ms(at.elapsed());
    let mut first = View::new(
        Size {
            width: 1200.,
            height: 800.,
        },
        7.8,
        12.,
    );
    std::hint::black_box(first.scene(workspace.editor()));
    reporter.emit(json!({"kind":"session_restore","files":50,"loaded_files":loaded,"lazy_files":50-loaded,"first_window_cpu_ready_ms":restored_ms,"first_scene_cpu_ready_ms":ms(at.elapsed()),"budget_ms":300,"boundary":"session read/parse + resources/assets + selected document restore; scene separately reported; excludes AppKit/fonts/Metal/scanout","gui_started":false,"gpu_started":false}))?;
    workspace.tabs[workspace.active]
        .editor
        .run("undo", &json!({}))?;
    if workspace.editor().text.to_string().starts_with("unsaved") {
        return Err("restored undo failed".into());
    }
    scene_bench(reporter, options)?;
    if std::env::var_os("VELOCITY_KEEP_WORKBENCH_FIXTURES").is_some() {
        reporter.emit(json!({"kind":"complete","complete":true,"fixtures":root,"restore_gui_command":format!("./target/release/velocity --bench-startup --restore-session --data-dir '{}'",data.display())}))?;
        let _ = fixture.keep();
        Ok(())
    } else {
        reporter.emit(json!({"kind":"complete","complete":true,"fixtures_retained":false}))
    }
}
pub fn scene_bench(reporter: &Reporter, options: &Options) -> Result<(), String> {
    let text = Text::from("fn main() { let hello = 1; }\n".repeat(10_000));
    let mut workspace = Workspace::new(text, None, None, false)?;
    workspace.set_file_type("Rust")?;
    workspace.editor_mut().settings.set(
        vel_config::settings::Layer::View,
        "word_wrap",
        json!(false),
    );
    workspace.editor_mut().sync();
    let at = workspace.editor().text.line_to_byte(5000);
    workspace.editor_mut().set_caret(at, false);
    crate::workbench::configure_groups(&mut workspace, 4)?;
    let mut views = (0..4)
        .map(|_| {
            View::new(
                Size {
                    width: options.width as f64 / 2.,
                    height: options.height as f64 / 2.,
                },
                7.8,
                12.,
            )
        })
        .collect::<Vec<_>>();
    let original = workspace.active;
    for (index, view) in views.iter_mut().enumerate() {
        view.scroll_y = (5000 + index * 50) as f64 * view.line_height;
    }
    // Force all four view caches to exist before sampling; state changes use public buffer views.
    for (index, view) in views.iter_mut().enumerate() {
        workspace.prepare_sheet(index)?;
        std::hint::black_box(view.scene(&workspace.tabs[index].editor));
    }
    workspace.prepare_sheet(original)?;
    let mut typing = Vec::new();
    let mut scrolling = Vec::new();
    for i in 0..300 {
        wait_all(&mut workspace, &mut views, original)?;
        let at = Instant::now();
        crate::type_through_keymap(workspace.editor_mut(), "x")?;
        for (index, view) in views.iter_mut().enumerate() {
            workspace.prepare_sheet(index)?;
            std::hint::black_box(view.scene(&workspace.tabs[index].editor));
        }
        workspace.prepare_sheet(original)?;
        typing.push(ms(at.elapsed()));
        for (index, view) in views.iter_mut().enumerate() {
            workspace.prepare_sheet(index)?;
            view.scroll(
                &workspace.tabs[index].editor,
                0.,
                if i % 2 == 0 { 20. } else { -20. },
            );
        }
        wait_all(&mut workspace, &mut views, original)?;
        let at = Instant::now();
        for (index, view) in views.iter_mut().enumerate() {
            workspace.prepare_sheet(index)?;
            std::hint::black_box(view.scene(&workspace.tabs[index].editor));
        }
        workspace.prepare_sheet(original)?;
        scrolling.push(ms(at.elapsed()));
    }
    reporter.emit(json!({"kind":"workbench_four_groups","typing_samples_ms":typing,"scroll_samples_ms":scrolling,"typing_edit_and_scene_cpu_ms":stats::summarize(&typing),"scroll_scene_cpu_ms":stats::summarize(&scrolling),"samples":300,"groups":4,"document_lines":10000,"logical_size":[options.width,options.height],"typing_p95_budget_ms":4,"scroll_p95_budget_ms":2,"boundary":"command + actual shared Buffer ownership + four CPU scenes; highlighted 10k-line Rust; exact four viewports and backend readiness before key/scroll timers; post-key scene may fall back; excludes async delivery, fonts, AppKit, Metal and scanout","gui_started":false,"gpu_started":false}))
}

fn wait_all(workspace: &mut Workspace, views: &mut [View], active: usize) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let mut exact = true;
        for (index, view) in views.iter_mut().enumerate() {
            workspace.prepare_sheet(index)?;
            let editor = &mut workspace.tabs[index].editor;
            editor.poll_language();
            std::hint::black_box(view.scene(editor));
            exact &= view.exact(editor)
                && editor
                    .language
                    .borrow()
                    .as_ref()
                    .is_some_and(|l| l.ready() && l.highlight_complete());
        }
        workspace.prepare_sheet(active)?;
        if exact {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err("four-group highlighting timed out".into());
        }
        std::thread::park_timeout(Duration::from_micros(250));
    }
}
