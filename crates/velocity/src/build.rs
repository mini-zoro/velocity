//! UI adapter for asynchronous builds and output-panel result navigation.
mod syntax_tests;
use crate::{
    panels::{Panel, Panels},
    ui_text::UiText,
    workspace::Workspace,
};
use vel_build::{BuildSystem, Diagnostic, Event, InputError, Stream, Task, Variables};
use vel_config::{Value, json};
#[cfg(test)]
thread_local! { pub(crate) static MATCHING_REQUESTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
type BuildChoice = (String, Option<String>);
type MatchingSet = Vec<(String, String)>;
fn apply_build_settings(build: &mut BuildSystem, settings: &vel_config::settings::Settings) {
    // Build systems override the view's fallback matchers and environment.
    // https://www.sublimetext.com/docs/build_systems.html#exec-target-options
    for (option, value) in [
        ("file_regex", settings.string("result_file_regex", "")),
        ("line_regex", settings.string("result_line_regex", "")),
        ("result_base_dir", settings.string("result_base_dir", "")),
    ] {
        if !value.is_empty() {
            build.options.entry(option).or_insert_with(|| json!(value));
        }
    }
    if let Some(defaults) = settings.get("build_env").and_then(Value::as_object)
        && !defaults.is_empty()
    {
        let env = build.options.entry("env").or_insert_with(|| json!({}));
        if let Some(env) = env.as_object_mut() {
            for (key, value) in defaults {
                if value.is_string() {
                    env.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
        }
    }
}

#[cfg(test)]
#[test]
fn build_settings_supply_result_base_and_merge_environment() {
    let dir = tempfile::tempdir().unwrap();
    let mut settings = vel_config::settings::Settings::default();
    let layer = vel_config::settings::Layer::User;
    for (key, value) in [
        ("result_file_regex", json!(r"^file (.+)$")),
        ("result_line_regex", json!(r"^line (\d+):(\d+) (.*)$")),
        ("result_base_dir", json!(dir.path())),
        (
            "build_env",
            json!({"LANE_DEFAULT":"yes","LANE_OVERRIDE":"default"}),
        ),
    ] {
        settings.set(layer, key, value);
    }
    let mut build = BuildSystem::from_value(
        "test",
        json!({
            "cmd":["true"], "env":{"LANE_OVERRIDE":"build"}
        }),
    )
    .unwrap();
    apply_build_settings(&mut build, &settings);
    let mut exec = build
        .resolve(&Variables::new(None, None, None, None))
        .unwrap();
    assert_eq!(exec.env["LANE_DEFAULT"], "yes");
    assert_eq!(exec.env["LANE_OVERRIDE"], "build");
    assert!(exec.parser.parse("file main.rs").is_none());
    let result = exec.parser.parse("line 4:2 message").unwrap();
    assert_eq!(
        result.path,
        dir.path().canonicalize().unwrap().join("main.rs")
    );
    assert_eq!((result.line, result.column), (4, 2));
}
#[derive(Debug, Default)]
pub struct Builds {
    pub task: Option<Task>,
    syntax_report: Option<std::sync::mpsc::Receiver<syntax_tests::Report>>,
    pub diagnostics: Vec<Diagnostic>,
    pub selected: Option<usize>,
    received_lines: usize,
    discarded_lines: usize,
    retention_notice: bool,
    discard_partial_line: bool,
    error_count: usize,
    pub syntax: Option<String>,
    pub word_wrap: bool,
    quiet: bool,
    environment_warning: Option<String>,
    pub choice: Option<BuildChoice>,
    choices: std::collections::VecDeque<(MatchingSet, BuildChoice)>,
}
impl Builds {
    pub fn start(
        &mut self,
        workspace: &Workspace,
        panels: &mut Panels,
        args: &Value,
    ) -> Result<(), String> {
        #[cfg(test)]
        crate::test_login::install();
        let direct = args.get("cmd").is_some() || args.get("shell_cmd").is_some();
        let mut build = if direct {
            BuildSystem::from_value("exec", args.clone())?
        } else {
            // The app resolves Automatic exactly once per request. Explicit Tools
            // choices intentionally apply across syntaxes (public build-system docs).
            let name = args["build_system"]
                .as_str()
                .or(workspace.build_system.as_deref());
            let build = workspace
                .build_systems
                .iter()
                .find(|b| Some(b.name.as_str()) == name)
                .ok_or("No matching build system; choose Tools > Build System")?;
            let variant = args["variant"].as_str().map(str::to_owned);
            if let Some(variant) = variant {
                build.variant(&variant)?
            } else {
                build.clone()
            }
        };
        let packages = workspace.data_path().map(|p| p.join("Packages"));
        let variables = Variables::new(
            workspace.tabs[workspace.active].path.as_deref(),
            workspace.folders.first().map(|f| f.path.as_path()),
            workspace.project_path(),
            packages.as_deref(),
        );
        apply_build_settings(&mut build, &workspace.editor().settings);
        let exec = match build.resolve(&variables) {
            Ok(exec) => exec,
            Err(error) => {
                panels.clear_output();
                panels.append_output_ui(UiText::tr_fmt(
                    "[Build error: {detail}]\n",
                    [("detail", error.clone())],
                ));
                panels.set_message(UiText::tr_fmt(
                    "Build failed: {detail}",
                    [("detail", error.clone())],
                ));
                return Err(error);
            }
        };
        let task = Task::spawn_after(&exec, self.task.take())?;
        self.syntax_report = None;
        panels.configure_output(workspace, exec.syntax.as_deref(), exec.word_wrap);
        self.diagnostics.clear();
        self.selected = None;
        self.received_lines = 0;
        self.discarded_lines = 0;
        self.retention_notice = false;
        self.discard_partial_line = false;
        self.error_count = 0;
        self.syntax = exec.syntax;
        self.word_wrap = exec.word_wrap;
        self.task = Some(task);
        self.quiet = exec.quiet;
        self.environment_warning = None;
        panels.clear_output();
        panels.build_output_offset = 0;
        if workspace
            .editor()
            .settings
            .bool("show_panel_on_build", true)
        {
            panels.panel = Some(Panel::Output);
        }
        if exec.interactive || panels.build_input.is_some() {
            panels.set_build_input(exec.interactive);
        }
        panels.result_scroll.offset = 0.;
        panels.build_results.clear();
        Ok(())
    }
    pub fn send_input(&self, panels: &mut Panels) {
        let Some((task, input)) = self.task.as_ref().zip(panels.build_input.as_ref()) else {
            return;
        };
        match task.send_line(&input.value()) {
            Ok(()) => {
                panels.set_build_input(true);
                panels.result_scroll.reveal(usize::MAX);
            }
            Err(InputError::Full) => {
                panels.set_message(UiText::tr("Build input is full"));
            }
            Err(InputError::Closed) => panels.set_build_input(false),
            Err(InputError::InvalidLine) => {
                panels.set_message(UiText::tr("Build input must be a single line"));
            }
        }
    }
    pub fn close_input(&mut self, panels: &mut Panels) {
        if let Some(task) = &mut self.task {
            task.close_input();
        }
        if panels.build_input.is_some() {
            panels.set_build_input(false);
        }
    }
    pub fn cancel(&mut self, panels: &mut Panels) {
        if let Some(task) = &self.task {
            task.cancel();
        }
        self.close_input(panels);
    }
    pub fn poll(&mut self, panels: &mut Panels) -> bool {
        if self.syntax_report.is_some() {
            return self.poll_syntax_tests(panels);
        }
        let mut changed = false;
        let mut finish = None;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(2);
        for _ in 0..128 {
            if changed && std::time::Instant::now() >= deadline {
                break;
            }
            let event = self.task.as_ref().and_then(|t| t.events.try_recv().ok());
            let Some(event) = event else {
                break;
            };
            changed = true;
            match event {
                Event::Output { stream, text } => self.append_output(panels, stream, &text),
                Event::OutputBatch(chunks) => {
                    for (stream, text) in chunks {
                        self.append_output(panels, stream, &text);
                    }
                }
                Event::Truncated {
                    bytes,
                    cancelling: true,
                    ..
                } => {
                    if !panels.output.is_empty() && !panels.output.ends_with('\n') {
                        panels.output.push('\n');
                    }
                    panels.append_output_ui(UiText::tr_fmt(
                        "[output truncated] ({bytes} bytes dropped)\n",
                        [("bytes", bytes.to_string())],
                    ));
                }
                Event::Truncated {
                    bytes,
                    first_line,
                    cancelling: false,
                } => {
                    panels.clear_output();
                    panels.append_output_ui(UiText::tr_fmt(
                        "[output truncated] ({bytes} bytes dropped)\n",
                        [("bytes", bytes.to_string())],
                    ));
                    panels.build_output_offset = 0;
                    self.discarded_lines = first_line;
                    self.received_lines = first_line;
                    self.retention_notice = true;
                    self.discard_partial_line = false;
                    self.diagnostics.clear();
                    panels.build_results.clear();
                }
                Event::Results(results) => {
                    self.accept_results(panels, results);
                }
                Event::Warning(message) => {
                    panels.message = message.clone().into();
                    self.environment_warning = Some(message);
                }
                Event::Error(e) => panels.output.push_str(&format!("\n[Build error: {e}]\n")),
                Event::InputClosed => self.close_input(panels),
                Event::Finished {
                    code,
                    signal,
                    cancelled,
                    elapsed,
                } => {
                    finish = Some((code, signal, cancelled, elapsed));
                    break;
                }
            }
        }
        self.retain_output(panels);
        // Parsing/counting happens before transport retention. Skipping 400 MB of
        // replay therefore preserves the total count and the retained result rows.
        if let Some(results) = self
            .task
            .as_ref()
            .and_then(|t| t.events.results(self.received_lines))
        {
            self.accept_results(panels, results);
            changed = true;
        }
        if let Some((code, signal, cancelled, elapsed)) = finish {
            self.close_input(panels);
            panels.append_output_ui(finish_line_with_signal(
                code, signal, cancelled, elapsed, self.quiet,
            ));
            let mut message = if self.error_count == 1 {
                UiText::tr("Build: 1 error")
            } else {
                UiText::tr_fmt(
                    "Build: {count} errors",
                    [("count", self.error_count.to_string())],
                )
            };
            if let Some(warning) = &self.environment_warning {
                message = message.with_raw_suffix(&format!(" · {warning}"));
            }
            panels.set_message(message);
            self.task = None;
        }
        changed
    }
    fn accept_results(&mut self, panels: &mut Panels, results: vel_build::Results) {
        self.error_count = results.errors;
        if results.reset {
            self.diagnostics.clear();
            panels.build_results.clear();
        }
        for mut d in results.diagnostics {
            if d.output_line < self.discarded_lines || d.output_line > self.received_lines {
                continue;
            }
            d.output_line =
                d.output_line - self.discarded_lines + usize::from(self.retention_notice);
            panels.build_results.push((
                d.output_line,
                crate::cli::FileLocation {
                    path: d.path.clone(),
                    line: d.line,
                    column: d.column,
                },
            ));
            self.diagnostics.push(d);
        }
        if self.diagnostics.len() > 32_768 {
            let n = self.diagnostics.len() - 32_768;
            self.diagnostics.drain(..n);
            panels.build_results.drain(..n);
        }
    }
    /// UI retention is bounded too when the consumer keeps up with the producer.
    fn retain_output(&mut self, panels: &mut Panels) {
        const LIMIT: usize = 8 * 1024 * 1024;
        const NOTICE: &str = "[output truncated]\n";
        if panels.output.len() > LIMIT {
            let cut = panels.output.len() - (6 * 1024 * 1024 - NOTICE.len());
            let at = panels.output.as_bytes()[cut..]
                .iter()
                .position(|b| *b == b'\n')
                .map_or(panels.output.len(), |n| cut + n + 1);
            self.discard_partial_line = at == panels.output.len() && !panels.output.ends_with('\n');
            let lines = panels.output[..at].bytes().filter(|b| *b == b'\n').count();
            panels.replace_output_prefix(at, UiText::tr("[output truncated]\n"));
            self.discarded_lines += lines.saturating_sub(usize::from(self.retention_notice));
            self.retention_notice = true;
            panels.build_output_offset = panels.build_output_offset.saturating_sub(lines);
            self.diagnostics.retain(|d| d.output_line >= lines);
            for d in &mut self.diagnostics {
                d.output_line = d.output_line - lines + 1;
            }
            panels.build_results.retain(|(row, _)| *row >= lines);
            for (row, _) in &mut panels.build_results {
                *row = *row - lines + 1;
            }
        }
    }
    fn append_output(&mut self, panels: &mut Panels, stream: Stream, text: &str) {
        self.received_lines += text.matches('\n').count();
        let displayed = if self.discard_partial_line {
            if let Some(end) = text.find('\n') {
                self.discard_partial_line = false;
                self.discarded_lines += 1;
                &text[end + 1..]
            } else {
                ""
            }
        } else {
            text
        };
        if stream == Stream::Input && !displayed.is_empty() {
            panels.echo_build_input(displayed);
        } else {
            panels.output.push_str(displayed);
        }
    }
    pub fn navigate(&mut self, workspace: &mut Workspace, forward: bool) -> Result<(), String> {
        if self.diagnostics.is_empty() {
            return Ok(());
        }
        let len = self.diagnostics.len();
        let index = self
            .selected
            .map_or(if forward { 0 } else { len - 1 }, |n| {
                (n as isize + if forward { 1 } else { -1 }).rem_euclid(len as isize) as usize
            });
        self.selected = Some(index);
        self.current_result(workspace)
    }
    pub fn current_result(&self, workspace: &mut Workspace) -> Result<(), String> {
        let Some(d) = self.selected.and_then(|index| self.diagnostics.get(index)) else {
            return Ok(());
        };
        workspace.open(crate::cli::FileLocation {
            path: d.path.clone(),
            line: d.line,
            column: d.column,
        })
    }
    /// Uses a bounded synchronous parse when a build needs unpublished embedded scopes.
    pub fn caret_scopes(workspace: &Workspace) -> Vec<vel_scope::Scope> {
        use vel_highlight::ScopeProvider;
        let editor = workspace.editor();
        let language = editor.language.borrow();
        let Some(language) = language.as_ref() else {
            return vec![];
        };
        if let Some(snapshot) = language.scope_snapshot_at(editor.caret) {
            return snapshot.scopes_at(editor.caret).unwrap_or(&[]).to_vec();
        }
        let Some(syntax) = language
            .assets
            .syntaxes
            .find_syntax_by_path(&language.syntax)
        else {
            return vec![language.root];
        };
        let mut engine = vel_highlight::DocumentHighlighter::new(
            editor.text.clone(),
            editor.buffer.change_id(),
            language.assets.syntaxes.clone(),
            syntax,
            language.scheme.clone(),
        );
        let row = editor.text.byte_to_line(editor.caret);
        let _ = engine.request_lines(
            editor.buffer.change_id(),
            row..row + 1,
            std::time::Duration::from_millis(5),
        );
        engine
            .scope_snapshot(row..row + 1)
            .scopes_at(editor.caret)
            .unwrap_or(&[language.root])
            .to_vec()
    }
    pub fn matching(workspace: &Workspace) -> Vec<&BuildSystem> {
        #[cfg(test)]
        MATCHING_REQUESTS.with(|n| n.set(n.get() + 1));
        let root = workspace
            .editor()
            .language
            .borrow()
            .as_ref()
            .map(|l| l.root)
            .unwrap_or_else(|| vel_scope::Scope::new("text.plain").unwrap());
        // Root-only selectors need no parsing. Other selectors can distinguish an
        // embedded scope or an exclusion; stale parsing gets a five-millisecond budget.
        let needs_caret = workspace
            .build_systems
            .iter()
            .filter_map(|b| b.options.get("selector").and_then(Value::as_str))
            .any(|selector| {
                vel_scope::Scope::new(selector).map_or(true, |s| !s.is_prefix_of(root))
            });
        let scopes = if needs_caret {
            Self::caret_scopes(workspace)
        } else {
            vec![root]
        };
        let roots = workspace
            .folders
            .iter()
            .map(|f| f.path.clone())
            .collect::<Vec<_>>();
        let mut builds = workspace
            .build_systems
            .iter()
            .filter_map(|b| {
                b.match_score(
                    &scopes,
                    workspace.tabs[workspace.active].path.as_deref(),
                    &roots,
                )
                .map(|score| (score, b))
            })
            .collect::<Vec<_>>();
        builds.sort_by_key(|a| std::cmp::Reverse(a.0));
        builds.into_iter().map(|(_, b)| b).collect()
    }
    pub fn choices(workspace: &Workspace, run: bool) -> Vec<vel_config::ui::PaletteItem> {
        if run {
            return Self::matching_choices(&Self::matching(workspace));
        }
        std::iter::once(vel_config::ui::PaletteItem {
            caption: "Build System: Automatic".into(),
            invocation: vel_config::keymap::Invocation {
                command: "set_build_system".into(),
                args: json!({"name":"Automatic"}),
            },
        })
        .chain(
            workspace
                .build_systems
                .iter()
                .map(|build| vel_config::ui::PaletteItem {
                    caption: build.name.clone(),
                    invocation: vel_config::keymap::Invocation {
                        command: "set_build_system".into(),
                        args: json!({"name":build.name}),
                    },
                }),
        )
        .collect()
    }
    fn matching_key(matching: &[&BuildSystem]) -> MatchingSet {
        let mut key = matching
            .iter()
            .map(|b| {
                (
                    b.name.clone(),
                    b.options
                        .get("selector")
                        .map_or(String::new(), Value::to_string),
                )
            })
            .collect::<Vec<_>>();
        key.sort();
        key
    }
    pub fn remembered(&self, matching: &[&BuildSystem]) -> Option<&BuildChoice> {
        let key = Self::matching_key(matching);
        self.choices
            .iter()
            .find(|(stored, (name, _))| *stored == key && matching.iter().any(|b| &b.name == name))
            .map(|(_, choice)| choice)
    }
    pub fn clear_choices(&mut self) {
        self.choice = None;
        self.choices.clear();
    }
    pub fn remember(&mut self, matching: &[&BuildSystem], name: &str, variant: Option<String>) {
        if matching.iter().any(|b| b.name == name) {
            let key = Self::matching_key(matching);
            self.choices.retain(|(stored, _)| *stored != key);
            let choice = (name.to_owned(), variant);
            self.choice = Some(choice.clone());
            self.choices.push_front((key, choice));
            self.choices.truncate(16);
        }
    }
    pub fn matching_choices(matching: &[&BuildSystem]) -> Vec<vel_config::ui::PaletteItem> {
        matching.iter().flat_map(|build| std::iter::once(None).chain(build.variants().into_iter().map(Some))
            .map(|variant| vel_config::ui::PaletteItem {
                caption: format!("{}{}", build.name, variant.as_ref().map_or(String::new(), |v| format!(" — {v}"))),
                invocation: vel_config::keymap::Invocation { command: "build".into(),
                    args: json!({"build_system":build.name,"variant":variant,"_build_picker":true}) }
            })).collect()
    }
}

/// Original formatting following the public build UI contract; quiet suppresses completion summaries.
// https://www.sublimetext.com/docs/build_systems.html#quiet
#[cfg(test)]
pub(crate) fn finish_line(
    code: Option<i32>,
    cancelled: bool,
    elapsed: std::time::Duration,
    quiet: bool,
) -> String {
    finish_line_with_signal(code, None, cancelled, elapsed, quiet).to_string()
}
pub(crate) fn finish_line_with_signal(
    code: Option<i32>,
    signal: Option<i32>,
    cancelled: bool,
    elapsed: std::time::Duration,
    quiet: bool,
) -> UiText {
    if cancelled {
        return UiText::tr("\n[Cancelled]\n");
    }
    if quiet {
        return UiText::default();
    }
    let duration = if elapsed.as_secs() == 0 {
        format!("{}ms", elapsed.as_millis())
    } else {
        format!("{:.1}s", elapsed.as_secs_f64())
    };
    match code {
        Some(0) => UiText::tr_fmt("\n[Finished in {duration}]\n", [("duration", duration)]),
        Some(code) => UiText::tr_fmt(
            "\n[Finished in {duration} with exit code {code}]\n",
            [("duration", duration), ("code", code.to_string())],
        ),
        None => signal.map_or_else(
            || {
                UiText::tr_fmt(
                    "\n[Finished in {duration} without an exit status]\n",
                    [("duration", duration.clone())],
                )
            },
            |signal| {
                #[cfg(unix)]
                let name = {
                    // SAFETY: strsignal accepts any signal number and returns a
                    // process-owned NUL-terminated string (or null); copy it now.
                    let ptr = unsafe { libc::strsignal(signal) };
                    if ptr.is_null() {
                        "Unknown signal".to_owned()
                    } else {
                        // SAFETY: nonnull strsignal result is a valid C string.
                        unsafe { std::ffi::CStr::from_ptr(ptr) }
                            .to_string_lossy()
                            .into_owned()
                    }
                };
                #[cfg(not(unix))]
                let name = "Terminated";
                let suffix = format!(": {signal}");
                let name = name.strip_suffix(&suffix).unwrap_or(&name);
                #[cfg(unix)]
                let name = if signal == libc::SIGUSR1 {
                    "SIGUSR1"
                } else if signal == libc::SIGUSR2 {
                    "SIGUSR2"
                } else {
                    name
                };
                UiText::tr_fmt(
                    "\n[Finished in {duration} with signal {signal} ({name})]\n",
                    [
                        ("duration", duration.clone()),
                        ("signal", signal.to_string()),
                        ("name", name.into()),
                    ],
                )
            },
        ),
    }
}

#[cfg(test)]
mod output_mirror_tests {
    use super::*;
    use crate::panels::Panel;
    use std::time::Duration;
    /// CPU time of the calling thread: frame cost without scheduler noise on a busy machine.
    fn thread_cpu() -> Duration {
        #[cfg(windows)]
        return vel_platform::metrics::thread_cpu().unwrap();
        #[cfg(unix)]
        {
            let mut t = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            // SAFETY: clock_gettime writes one timespec for a valid clock id.
            unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
            Duration::new(t.tv_sec as u64, t.tv_nsec as u32)
        }
    }
    /// Streams `total` bytes of build output through retention trimming and the output view,
    /// one producer chunk per frame, then drains; `restart` starts a new build half way.
    /// Returns per-frame main-thread CPU time (poll + layout + paint, as the native loop does).
    fn stream(total: usize, wrap: bool, restart: bool) -> Vec<Duration> {
        let mut workspace =
            Workspace::new(vel_text::Text::new(), None, None, false).expect("workspace");
        let mut panels = Panels::new(&workspace, false);
        panels.configure_output(&workspace, None, wrap);
        panels.panel = Some(Panel::Output);
        let size = vel_platform::Size {
            width: 1200.,
            height: 800.,
        };
        let mut builds = Builds::default();
        let mut produced = 0;
        let mut frames = Vec::new();
        let mut line = 0usize;
        let mut restarted = !restart;
        let mut next_long = 1 << 20;
        let mut long_lines = 0;
        for _ in 0..10_000 {
            if !restarted && produced >= total / 2 {
                // What `Builds::start` does for a new build (no process needed here).
                restarted = true;
                panels.configure_output(&workspace, None, wrap);
                panels.output.clear();
                panels.output_reset = true;
                builds = Builds::default();
            }
            if produced < total {
                let mut chunk = String::new();
                if produced >= next_long {
                    next_long += 3 << 20;
                    // An occasional 20 KB line (minified output, long paths) must not
                    // flip the view's large-document policy either.
                    chunk.push_str(&"long-line ".repeat(2_000));
                    chunk.push('\n');
                    long_lines += 1;
                }
                while chunk.len() < 96 * 1024 {
                    chunk.push_str(&format!(
                        "{line:>8} awk generated output line {}\n",
                        "x".repeat(line % 140)
                    ));
                    line += 1;
                }
                produced += chunk.len();
                builds.append_output(&mut panels, Stream::Stdout, &chunk);
                builds.retain_output(&mut panels);
            } else if !panels.busy() {
                break;
            }
            let at = thread_cpu();
            panels.poll(&mut workspace, 0..0);
            panels.layout(size);
            panels.paint(&workspace, &mut vel_render::scene::Scene::default());
            frames.push(thread_cpu() - at);
        }
        assert!(long_lines >= 2);
        let editor = panels.output_editor.as_ref().expect("output view");
        assert!(panels.output.len() <= 8 * 1024 * 1024);
        assert_eq!(editor.text.to_string(), panels.output, "mirror diverged");
        frames
    }
    #[test]
    fn trimmed_build_output_is_mirrored_exactly_and_incrementally() {
        for wrap in [false, true] {
            let frames = stream(12 * 1024 * 1024, wrap, false);
            assert!(frames.len() > 10);
        }
        stream(6 * 1024 * 1024, true, true);
    }
    /// `cargo test --release -p velocity output_mirror -- --ignored --nocapture`
    #[test]
    #[ignore = "timing; run on an idle machine"]
    fn large_build_output_frames_stay_short() {
        for (wrap, restart) in [(false, false), (true, false), (true, true)] {
            let frames = stream(20 * 1024 * 1024, wrap, restart);
            let slowest = frames.iter().max().copied().unwrap_or_default();
            let slow = frames
                .iter()
                .filter(|f| **f > Duration::from_millis(16))
                .count();
            eprintln!(
                "wrap={wrap} restart={restart}: {} frames, slowest {slowest:?}, {slow} over 16 ms",
                frames.len()
            );
            assert!(slowest < Duration::from_millis(16), "{slowest:?}");
        }
    }
}
