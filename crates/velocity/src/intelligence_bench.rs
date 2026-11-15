//! Repeatable intelligence workloads. Headless CPU evidence is distinct from native presentation.
use crate::{
    cli::Options,
    panels::Panels,
    parse_bench::{self, Fixture},
    report::{Reporter, ms},
    stats::summarize,
    view::View,
    workspace::Workspace,
};
use std::{
    hint::black_box,
    time::{Duration, Instant},
};
use vel_config::json;
use vel_platform::Size;
#[derive(Debug)]
pub struct Prepared {
    pub full_size: Size,
    pub review: Option<bool>,
    pub fixture: Fixture,
    pub workspace: Workspace,
    pub panels: Panels,
    pub view: View,
}
impl Prepared {
    pub fn new(options: &Options) -> Result<Self, String> {
        let (fixture, mut workspace) = parse_bench::prepare(options)?;
        if let Some(enabled) = options.review_ui {
            workspace.settings.set(
                vel_config::settings::Layer::View,
                "show_git_status",
                json!(enabled),
            );
            for tab in &mut workspace.tabs {
                for (key, value) in [
                    ("mini_diff", json!(enabled)),
                    ("spell_check", json!(enabled)),
                    ("show_git_status", json!(enabled)),
                ] {
                    tab.editor
                        .settings
                        .set(vel_config::settings::Layer::View, key, value);
                }
                tab.editor.sync();
            }
        }
        parse_bench::wait(&mut workspace)?;
        let mut panels = Panels::new(&workspace, false);
        panels.add_folder(&mut workspace, fixture.0.clone())?;
        let until = Instant::now() + Duration::from_secs(120);
        loop {
            workspace.poll();
            panels.poll(&mut workspace, 0..1000);
            if panels
                .intel
                .projects
                .first()
                .is_some_and(|p| !p.index.status().active)
            {
                break;
            }
            if Instant::now() > until {
                return Err("project benchmark setup timeout".into());
            }
            std::thread::park_timeout(Duration::from_millis(1));
        }
        let vocabulary = (0..options.intel_words)
            .map(|n| format!("candidate_{n:06} "))
            .collect::<String>();
        let analysis = vel_index::FileAnalysis {
            words: vel_index::WordStatistics::from_text(&vocabulary),
            ..Default::default()
        };
        drop(vocabulary);
        panels.intel.projects[0]
            .index
            .shared_index()
            .write()
            .unwrap()
            .replace_file(fixture.0.join("vocabulary.txt"), analysis);
        workspace.select(
            workspace
                .tabs
                .iter()
                .position(|t| t.path.as_ref().is_some_and(|p| p.ends_with("file9.rs")))
                .ok_or("missing typing file")?,
        );
        let at = workspace.editor().text.line_to_byte(500) + 7;
        workspace
            .editor_mut()
            .set_selection(vel_buffer::Selection::from_regions([
                vel_buffer::Region::from(at..at + "render_9_500".len()),
            ]));
        workspace.editor_mut().run(
            "insert",
            &json!({"characters":if options.intel_words==0{"render"}else{"candidate"}}),
        )?;
        workspace.editor_mut().settings.set(
            vel_config::settings::Layer::View,
            "auto_complete_delay",
            json!(0),
        );
        workspace.editor_mut().sync();
        let mut view = View::new(
            Size {
                width: options.width as f64,
                height: options.height as f64,
            },
            7.8,
            12.,
        );
        view.reveal(workspace.editor());
        // The fixture prefix is preparation; measured keys must undo independently.
        workspace.editor_mut().buffer.break_undo_group();
        let mut result = Self {
            full_size: Size {
                width: options.width as f64,
                height: options.height as f64,
            },
            review: options.review_ui,
            fixture,
            workspace,
            panels,
            view,
        };
        result.request();
        result.wait_popup()?;
        parse_bench::wait(&mut result.workspace)?;
        result.panels.poll(&mut result.workspace, 0..1000);
        result.panels.intel.flush_publications()?;
        Ok(result)
    }
    pub fn request(&mut self) {
        let _ = self.workspace.editor_mut().run("auto_complete", &json!({}));
    }
    pub fn scene(&mut self) -> vel_render::scene::Scene {
        if self.review.is_some() {
            let content = self.panels.layout(self.full_size);
            self.view.size = Size {
                width: content.width,
                height: content.height,
            };
            let mut scene = self.view.scene(self.workspace.editor());
            crate::app_scene_translate(&mut scene, content.x, content.y);
            self.panels.paint(&self.workspace, &mut scene);
            scene
        } else {
            self.view.scene(self.workspace.editor())
        }
    }
    pub fn poll(&mut self) -> bool {
        if self.review.is_some() {
            black_box(self.scene());
        }
        self.workspace.poll();
        self.panels.poll(&mut self.workspace, 0..1000);
        let ready = self.review != Some(true) || {
            let editor = self.workspace.editor();
            let annotations = editor.annotations.borrow();
            let git_ready = self.workspace.tabs[self.workspace.active]
                .path
                .as_deref()
                .and_then(|p| self.workspace.git.snapshot(p))
                .is_some();
            annotations.pending == 0
                && annotations.current.as_ref().is_some_and(|p| {
                    p.identity.1 == editor.buffer.change_id() && !p.spelling.is_empty()
                })
                && git_ready
        };
        ready
            && self.workspace.editor().completion.visible()
            && !self.workspace.editor().completion.busy()
            && self
                .workspace
                .editor()
                .completion
                .list
                .as_ref()
                .is_some_and(|l| l.version == self.workspace.editor().buffer.change_id())
    }
    pub fn wait_popup(&mut self) -> Result<(), String> {
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            if self.poll() {
                black_box(self.scene());
                return Ok(());
            }
            if Instant::now() > until {
                return Err(format!(
                    "completion benchmark timed out: {:?}; annotations {:?}",
                    self.workspace.messages,
                    self.workspace
                        .editor()
                        .annotations
                        .borrow()
                        .current
                        .as_ref()
                        .map(|p| (p.identity, p.spelling.len()))
                ));
            }
            std::thread::park_timeout(Duration::from_micros(100));
        }
    }
    pub fn reset(&mut self) -> Result<(), String> {
        self.workspace.editor_mut().run("undo", &json!({}))?;
        self.request();
        Ok(())
    }
    pub fn definitions(&mut self) -> Result<Vec<f64>, String> {
        let root = self.fixture.0.canonicalize().map_err(|e| e.to_string())?;
        let file = root.join("file9.rs");
        let location = root.join("file0.rs");
        // A real reference in the source buffer resolves to file0's indexed definition.
        let end = self.workspace.editor().text.len_bytes();
        self.workspace.editor_mut().set_caret(end, false);
        self.workspace
            .editor_mut()
            .run("insert", &json!({"characters":"\n// render_0_500\n"}))?;
        let reference = self
            .workspace
            .editor()
            .text
            .to_string()
            .rfind("render_0_500")
            .ok_or("missing reference")?;
        parse_bench::wait(&mut self.workspace)?;
        self.panels.poll(&mut self.workspace, 0..1000);
        self.panels.intel.flush_publications()?;
        let mut samples = Vec::new();
        for _ in 0..100 {
            let tab = self
                .workspace
                .tabs
                .iter()
                .position(|t| t.path.as_ref() == Some(&file))
                .ok_or("typing tab closed")?;
            self.workspace.select(tab);
            self.workspace.editor_mut().set_caret(reference, false);
            let start = Instant::now();
            self.panels
                .action(&mut self.workspace, "goto_definition", &json!({}), 0..1000)?;
            let until = Instant::now() + Duration::from_secs(5);
            loop {
                self.panels.poll(&mut self.workspace, 0..1000);
                if !self.panels.intel.pending {
                    break;
                }
                if Instant::now() > until {
                    return Err("definition benchmark timed out".into());
                }
                std::thread::park_timeout(Duration::from_micros(50));
            }
            self.view.reveal(self.workspace.editor());
            black_box(self.view.scene(self.workspace.editor()));
            samples.push(ms(start.elapsed()));
            if self.workspace.tabs[self.workspace.active].path.as_ref() != Some(&location) {
                return Err("definition did not reach the already-open target".into());
            }
        }
        Ok(samples)
    }
}
pub fn run(reporter: &Reporter, options: &Options) -> Result<(), String> {
    let mut prepared = Prepared::new(options)?;
    reporter.rss("ten_file_index_completion",json!({"files":10,"lines_per_file":1000,"synthetic_project_words":options.intel_words,"popup_visible":true,"gui_started":false,"gpu_started":false}))?;
    if std::env::var_os("VELOCITY_REVIEW_PROFILE").is_some() {
        return profile(reporter, prepared);
    }
    let mut samples = Vec::new();
    for _ in 0..500 {
        prepared.wait_popup()?;
        let at = Instant::now();
        crate::type_through_keymap(prepared.workspace.editor_mut(), "0")?;
        black_box(prepared.scene());
        samples.push(ms(at.elapsed()));
        prepared.reset()?;
    }
    let definitions = prepared.definitions()?;
    reporter.emit(json!({"kind":"intelligence_cpu","review_ui":options.review_ui,"typing_samples_ms":samples,"typing_ms":summarize(&samples),"typing_budget_ms":[2,4],"definition_samples_ms":definitions,"definition_ms":summarize(&definitions),"definition_budget_ms":20,"files":10,"synthetic_project_words":options.intel_words,"popup_visible_before_every_key":true,"popup_painted_in_timer":true,"gui_started":false,"gpu_started":false,"boundary":"keymap/command/transaction plus CPU scene and popup; background preparation before each key; definition request, worker delivery, open/reveal/CPU scene of already-open file"}))?;
    reporter
        .emit(json!({"kind":"complete","complete":true,"gui_started":false,"gpu_started":false}))
}

#[cfg(target_os = "macos")]
#[derive(Debug)]
pub struct Native {
    prepared: Prepared,
    reporter: Reporter,
    renderer: Option<vel_render::Renderer>,
    fonts: Option<vel_render::font::Fonts>,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    injected: Option<Instant>,
    awaiting: Option<(u64, Instant, f64, f64)>,
    submit: Vec<f64>,
    scanout: Vec<f64>,
    cpu: Vec<f64>,
    memory: bool,
    finished: bool,
    words: usize,
}
#[cfg(target_os = "macos")]
impl Native {
    pub fn new(
        options: &Options,
        reporter: Reporter,
        done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<Self, String> {
        Ok(Self {
            prepared: Prepared::new(options)?,
            reporter,
            renderer: None,
            fonts: None,
            done,
            injected: None,
            awaiting: None,
            submit: Vec::new(),
            scanout: Vec::new(),
            cpu: Vec::new(),
            memory: false,
            finished: false,
            words: options.intel_words,
        })
    }
}
#[cfg(target_os = "macos")]
impl vel_platform::AppHandler for Native {
    fn on_start(&mut self, w: &vel_platform::WindowHandle) -> Result<(), String> {
        let fonts = vel_render::font::Fonts::load_named("Menlo")?;
        self.prepared.full_size = w.content_size();
        self.prepared.view = View::new(w.content_size(), fonts.cell as f64, fonts.ascent as f64);
        self.prepared.view.reveal(self.prepared.workspace.editor());
        self.fonts = Some(fonts);
        self.renderer = Some(vel_render::Renderer::new(
            w.clone(),
            false,
            &std::env::temp_dir().join("velocity-intelligence-renderer"),
            |_| {},
        )?);
        self.reporter.emit(json!({"kind":"intelligence_gui_environment","logical_size":[w.content_size().width,w.content_size().height],"actual_physical_size":self.renderer.as_ref().unwrap().physical_size(),"scale_factor":w.scale_factor(),"maximum_frames_per_second":w.maximum_frames_per_second(),"synthetic_project_words":self.words,"files":10}))
    }
    fn on_resize(&mut self, _: &vel_platform::WindowHandle, size: Size, scale: f64) {
        self.prepared.full_size = size;
        self.prepared.view.size = size;
        if let Some(renderer) = &mut self.renderer {
            renderer.resize((size.width * scale) as u32, (size.height * scale) as u32);
        }
    }
    fn on_frame(&mut self, ctx: &mut vel_platform::FrameContext<'_>) -> Result<(), String> {
        use std::sync::atomic::Ordering;
        if ctx.reason == vel_platform::FrameReason::StartupPrepare {
            let scene = self.prepared.scene();
            self.renderer
                .as_mut()
                .ok_or("renderer missing")?
                .pre_render(
                    &scene,
                    self.fonts.as_mut().ok_or("fonts missing")?,
                    ctx.window.scale_factor() as f32,
                )?;
            return Ok(());
        }
        if self
            .renderer
            .as_ref()
            .is_some_and(|r| r.has_prepared_frame())
        {
            self.renderer.as_mut().unwrap().present_prepared()?;
            self.renderer.as_mut().unwrap().release_prepared();
            ctx.window
                .request_frame_at(Instant::now() + Duration::from_millis(16));
            return Ok(());
        }
        if self.memory {
            ctx.window.display_status().check()?;
        }
        for observation in ctx.window.take_presentations() {
            if let Some((id, at, submit, cpu)) = self.awaiting
                && observation.drawable == id
            {
                self.awaiting = None;
                if observation.presented_time > 0. {
                    self.submit.push(submit);
                    self.cpu.push(cpu);
                    self.scanout
                        .push(ms(observation.scanout.saturating_duration_since(at)));
                    self.injected = None;
                    self.prepared.reset()?;
                }
            }
        }
        if self.submit.len() >= 500 {
            let definitions = self.prepared.definitions()?;
            self.reporter.emit(json!({"kind":"intelligence_gui_summary","inject_to_submit_ms":summarize(&self.submit),"inject_to_scanout_ms":summarize(&self.scanout),"cpu_command_scene_ms":summarize(&self.cpu),"definition_ms":summarize(&definitions),"typing_budget_ms":[2,4],"definition_budget_ms":20,"samples_ms":self.submit,"scanout_samples_ms":self.scanout,"cpu_samples_ms":self.cpu,"definition_samples_ms":definitions,"boundary":"scripted keymap injection; actual positive presentedTime matched by drawable; popup and 100k-word index; no HID injection"}))?;
            self.reporter.rss(
                "after_intelligence_gui",
                json!({"files":10,"synthetic_project_words":self.words}),
            )?;
            self.reporter
                .emit(json!({"kind":"complete","complete":true}))?;
            self.finished = true;
            self.done.store(true, Ordering::Release);
            ctx.window.quit();
            return Ok(());
        }
        self.prepared.poll();
        if self.awaiting.is_some() {
            ctx.window
                .request_frame_at(Instant::now() + Duration::from_millis(8));
            return Ok(());
        }
        if self.injected.is_none() {
            if !self.prepared.poll() {
                ctx.window
                    .request_frame_at(Instant::now() + Duration::from_millis(2));
                return Ok(());
            }
            let at = Instant::now();
            self.injected = Some(at);
            crate::type_through_keymap(self.prepared.workspace.editor_mut(), "0")?;
        }
        let at = self.injected.unwrap();
        let scene = self.prepared.scene();
        let cpu = ms(at.elapsed());
        if let Some(times) = self.renderer.as_mut().ok_or("renderer missing")?.render(
            &scene,
            self.fonts.as_mut().ok_or("fonts missing")?,
            ctx.window.scale_factor() as f32,
            None,
        )? {
            if let Some(id) = times.drawable {
                self.awaiting = Some((
                    id,
                    at,
                    ms(times.submitted.saturating_duration_since(at)),
                    cpu,
                ));
            }
            if !self.memory {
                self.renderer.as_ref().unwrap().wait_idle()?;
                self.reporter.rss(
                    "ten_file_index_completion_gui",
                    json!({"files":10,"synthetic_project_words":self.words,"popup_visible":true}),
                )?;
                self.memory = true;
            }
        }
        ctx.window
            .request_frame_at(Instant::now() + Duration::from_millis(8));
        Ok(())
    }
    fn on_quit_requested(&mut self, _: &vel_platform::WindowHandle) -> bool {
        if !self.finished {
            let _=self.reporter.emit(json!({"kind":"error","complete":false,"message":"intelligence benchmark closed early"}));
        }
        true
    }
}

fn profile(reporter: &Reporter, mut prepared: Prepared) -> Result<(), String> {
    let mut command = Vec::new();
    let mut view = Vec::new();
    let mut chrome = Vec::new();
    for _ in 0..500 {
        prepared.wait_popup()?;
        let at = Instant::now();
        crate::type_through_keymap(prepared.workspace.editor_mut(), "0")?;
        command.push(ms(at.elapsed()));
        let at = Instant::now();
        let content = prepared.panels.layout(prepared.full_size);
        prepared.view.size = Size {
            width: content.width,
            height: content.height,
        };
        let mut scene = prepared.view.scene(prepared.workspace.editor());
        crate::app_scene_translate(&mut scene, content.x, content.y);
        view.push(ms(at.elapsed()));
        let at = Instant::now();
        prepared.panels.paint(&prepared.workspace, &mut scene);
        black_box(scene);
        chrome.push(ms(at.elapsed()));
        prepared.reset()?;
    }
    reporter.emit(json!({"kind":"review_component_profile","review_ui":prepared.review,"command_ms":summarize(&command),"view_ms":summarize(&view),"chrome_ms":summarize(&chrome),"command_samples_ms":command,"view_samples_ms":view,"chrome_samples_ms":chrome,"gui_started":false,"gpu_started":false}))?;
    reporter
        .emit(json!({"kind":"complete","complete":true,"gui_started":false,"gpu_started":false}))
}
