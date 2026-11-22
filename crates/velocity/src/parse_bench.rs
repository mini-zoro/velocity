//! Shared round-7/8 parser-removal probe. This file also builds unchanged in the baseline tree.
use crate::{
    cli::{FileLocation, Options},
    language::Assets,
    report::{Reporter, ms},
    workspace::Workspace,
};
use std::time::{Duration, Instant};
use vel_text::Text;
pub type Fixture = tempfile::TempDir;
pub fn prepare(options: &Options) -> Result<(Fixture, Workspace), String> {
    Assets::bundled()?;
    let fixture = tempfile::Builder::new()
        .prefix("velocity-intelligence-fixture-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    let root = fixture.path();
    for file in 0..10 {
        let source = (0..1000)
            .map(|row| {
                format!(
                    "pub fn render_{file}_{row}() {{ let value = {row}; }} // 你好{}\n",
                    if options.review_ui.is_some() {
                        " mispelled comment"
                    } else {
                        ""
                    }
                )
            })
            .collect::<String>();
        std::fs::write(root.join(format!("file{file}.rs")), source).map_err(|e| e.to_string())?;
    }
    if options.review_ui.is_some() {
        crate::review_bench::repository(root)?;
    }
    let mut workspace = Workspace::new(Text::new(), options.data_dir.clone(), None, false)?;
    for file in 0..10 {
        workspace.open(FileLocation {
            path: root.join(format!("file{file}.rs")),
            line: 1,
            column: 1,
        })?;
    }
    Ok((fixture, workspace))
}
pub fn wait(workspace: &mut Workspace) -> Result<(), String> {
    let until = Instant::now() + Duration::from_secs(120);
    loop {
        workspace.poll();
        if workspace.tabs.iter().filter(|t| t.path.is_some()).all(|t| {
            t.editor
                .language
                .borrow()
                .as_ref()
                .is_some_and(|l| l.ready() && l.highlight_complete())
        }) {
            return Ok(());
        }
        if Instant::now() > until {
            return Err(format!("parser probe timed out: {:?}", workspace.messages));
        }
        std::thread::park_timeout(Duration::from_millis(1));
    }
}
pub fn cpu_ms() -> Result<f64, String> {
    #[cfg(windows)]
    return vel_platform::metrics::process_cpu_ms();
    #[cfg(unix)]
    {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        // SAFETY: getrusage writes one rusage into the valid out pointer on success.
        let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
        if result != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        // SAFETY: the successful getrusage initialized all fields.
        let usage = unsafe { usage.assume_init() };
        Ok(
            (usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) as f64 * 1000.
                + (usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) as f64 / 1000.,
        )
    }
}
pub fn run(reporter: &Reporter, options: &Options) -> Result<(), String> {
    Assets::bundled()?;
    let cpu = cpu_ms()?;
    let at = Instant::now();
    let (_fixture, mut workspace) = prepare(options)?;
    wait(&mut workspace)?;
    let elapsed = ms(at.elapsed());
    std::thread::park_timeout(Duration::from_millis(100));
    reporter.emit(serde_json::json!({"kind":"ten_file_parser_memory_cpu","files":10,"lines_per_file":1000,"bytes":workspace.tabs.iter().map(|t|t.editor.text.len_bytes()).sum::<usize>(),"cpu_ms":cpu_ms()?-cpu,"wall_ms":elapsed,"phys_footprint_bytes":vel_platform::metrics::phys_footprint_bytes().ok(),"rss_bytes":vel_platform::metrics::rss_bytes().ok(),"index_enabled":false,"gui_started":false,"gpu_started":false,"boundary":"assets warmed; ten real files opened, complete highlighter and editor analysis; CPU includes all process threads; no index or completion query"}))?;
    reporter.emit(serde_json::json!({"kind":"complete","complete":true,"gui_started":false,"gpu_started":false}))
}
