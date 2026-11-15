//! Isolated disposable fixtures and full-history persistence measurements; no GUI initialization.
use crate::{
    cli::Options,
    report::{Reporter, ms},
    workspace::Workspace,
};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use vel_config::json;
use vel_session::{Document, History, Session, Sheet, ViewState, Window};
use vel_text::Text;

/// Creates commits only in the newly generated temporary benchmark directory.
pub fn repository(root: &Path) -> Result<(), String> {
    for args in [
        vec!["init", "-q"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=Velocity Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ],
    ] {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .map_err(|e| e.to_string())?;
        if !status.status.success() {
            return Err(String::from_utf8_lossy(&status.stderr).into_owned());
        }
    }
    std::fs::write(root.join("untracked.txt"), "untracked\n").map_err(|e| e.to_string())?;
    Ok(())
}
pub fn history(reporter: &Reporter, options: &Options) -> Result<(), String> {
    let data = options
        .data_dir
        .clone()
        .ok_or("--headless-review-history requires --data-dir")?;
    let root = data.join("fixtures");
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let mut documents = vec![];
    let mut sheets = vec![];
    let mut workbench = vel_ui::workbench::Workbench::default();
    let mut capture = vec![];
    for n in 1..=50 {
        let path = root.join(format!("history-{n}.txt"));
        let base = "saved line\n".repeat(100);
        std::fs::write(&path, &base).map_err(|e| e.to_string())?;
        let mut buffer = vel_buffer::Buffer::new(base.as_str());
        let views = [buffer.new_view(), buffer.new_view()];
        for step in 0..100 {
            vel_buffer::ops::paste(&mut buffer, views[step % 2], "edit 你好\n");
        }
        buffer.mark_saved();
        std::fs::write(&path, buffer.text().to_string()).map_err(|e| e.to_string())?;
        buffer.undo(views[0]);
        buffer.soft_undo(views[1]);
        let at = Instant::now();
        let history = History::capture(&buffer);
        capture.push(ms(at.elapsed()));
        documents.push(Document {
            id: n,
            path: Some(path.clone()),
            disk: vel_session::DiskStamp::read(&path),
            encoding: "UTF-8".into(),
            bom: false,
            line_ending: "LF".into(),
            dirty: true,
            history: Some(history),
        });
        for (i, view) in views.into_iter().enumerate() {
            let id = n * 2 + i as u64;
            sheets.push(Sheet {
                id,
                document: n,
                view: ViewState::capture(&buffer, view, (0., 0.)),
                syntax: None,
                transient: false,
            });
            workbench.groups[0].add(id, false);
        }
    }
    workbench.groups[0].click(2, false, false);
    let session = Session {
        documents,
        windows: vec![Window {
            workbench,
            sheets,
            ..Default::default()
        }],
        ..Default::default()
    };
    let path = data.join("Sessions/Session.sublime-session");
    let writer = vel_session::Writer::new(path.clone(), Duration::from_millis(400))?;
    let mut samples = vec![];
    for _ in 0..10 {
        writer.changed(session.clone())?;
        samples.push(ms(writer.flush()?));
    }
    reporter.emit(json!({"kind":"full_history_save","dirty_files":50,"views":100,"steps_per_file":100,"capture_samples_ms":capture,"save_samples_ms":samples,"worker_ms":crate::stats::summarize(&samples),"budget_ms":30,"serialized_bytes":std::fs::metadata(&path).map_err(|e|e.to_string())?.len(),"boundary":"worker validation + native history delta packing + JSON + atomic file/directory fsync; capture reported separately","gui_started":false,"gpu_started":false}))?;
    let at = Instant::now();
    let recovered = vel_session::load(&path);
    let read_ms = ms(at.elapsed());
    if !recovered.messages.is_empty() {
        return Err(recovered.messages.join("; "));
    }
    let phase = Instant::now();
    let mut workspace = Workspace::new(Text::new(), Some(data.clone()), None, false)?;
    let workspace_ms = ms(phase.elapsed());
    let phase = Instant::now();
    workspace.restore_window(&recovered.session.windows[0], &recovered.session.documents)?;
    reporter.emit(json!({"kind":"full_history_restore","asset_ms":workspace.assets.startup_ms,"read_ms":read_ms,"workspace_ms":workspace_ms,"window_ms":ms(phase.elapsed()),"first_window_cpu_ms":ms(at.elapsed()),"budget_ms":300,"loaded_sheets":workspace.tabs.iter().filter(|t|t.lazy.is_none()).count(),"lazy_sheets":workspace.tabs.iter().filter(|t|t.lazy.is_some()).count(),"boundary":"session load/parse + resources + selected native history import; excludes AppKit/fonts/Metal/scanout","gui_started":false,"gpu_started":false}))?;
    reporter.emit(json!({"kind":"complete","complete":true,"data_dir":data,"session_file":path,"gui_started":false,"gpu_started":false}))
}
