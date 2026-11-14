//! Native application handler and buffered benchmark observations.
use crate::{
    bench::{Bench, SCROLL_FRAMES, TYPING_INSERTIONS},
    cli::{Mode, Options},
    report::{Clock, FirstScanout, Reporter, ms},
    stats::{summarize, sweep_positions},
    view::View,
    workspace::Workspace,
};
use serde::Serialize;
use serde_json::json;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use vel_config::{Value, settings::Layer};
use vel_platform::*;
use vel_render::scene::{Layer as SceneLayer, SolidRect, TextRun};
use vel_render::{FrameTimes, Renderer, font::Fonts};

#[derive(Clone, Debug, Serialize)]
struct Sample {
    index: usize,
    event_to_submit_ms: f64,
    event_to_present_return_ms: f64,
    acquire_ms: f64,
    encode_submit_ms: f64,
    present_call_ms: f64,
    timer_lateness_ms: Option<f64>,
}
#[derive(Debug)]
pub struct App {
    options: Options,
    clock: Clock,
    reporter: Reporter,
    workspace: Workspace,
    view: View,
    fonts: Option<Fonts>,
    font_face: String,
    renderer: Option<Renderer>,
    first_present: bool,
    startup_done: bool,
    bench: Option<Bench>,
    samples: Vec<Sample>,
    retries: usize,
    input_at: Option<Instant>,
    bench_received: Option<Instant>,
    done: Arc<AtomicBool>,
    external_input: usize,
    first_scanout: FirstScanout,
    waiting_since: Option<Instant>,
}
impl App {
    pub fn new(
        options: Options,
        clock: Clock,
        reporter: Reporter,
        mut workspace: Workspace,
        done: Arc<AtomicBool>,
    ) -> Self {
        if options.mode == Mode::Typing {
            crate::install_carets(workspace.editor_mut(), options.carets);
        }
        let view = View::new(
            Size {
                width: options.width as f64,
                height: options.height as f64,
            },
            8.,
            12.,
        );
        Self {
            options,
            clock,
            reporter,
            workspace,
            view,
            fonts: None,
            font_face: String::new(),
            renderer: None,
            first_present: false,
            startup_done: false,
            bench: None,
            samples: Vec::new(),
            retries: 0,
            input_at: None,
            bench_received: None,
            done,
            external_input: 0,
            first_scanout: FirstScanout::default(),
            waiting_since: None,
        }
    }
    fn show_error(&self, w: &WindowHandle, message: &str) {
        let _ = self
            .reporter
            .emit(json!({"kind":"editor_error","message":message}));
        w.alert("Velocity", message, &["OK"]);
    }
    fn run_named(&mut self, w: &WindowHandle, name: &str, args: &Value) {
        self.run_named_depth(w, name, args, 0);
    }
    fn run_named_depth(&mut self, w: &WindowHandle, name: &str, args: &Value, depth: usize) {
        if depth >= 32 {
            self.show_error(w, "command chain exceeds 32 levels");
            return;
        }
        if name == "chain" {
            if let Some(commands) = args["commands"].as_array() {
                for command in commands {
                    if let Some(name) = command[0].as_str() {
                        self.run_named_depth(
                            w,
                            name,
                            command.get(1).unwrap_or(&json!({})),
                            depth + 1,
                        );
                    } else {
                        self.show_error(w, "chain entry needs [name, args]");
                        return;
                    }
                }
            } else {
                self.show_error(w, "chain needs commands");
            }
            return;
        }
        if !self.command_enabled(name, &args.to_string()) {
            return;
        }
        self.workspace.editor_mut().clipboard = w.clipboard_text().unwrap_or_default();
        let before = self.workspace.editor().clipboard.clone();
        if let Err(error) = self.workspace.editor_mut().run(name, args) {
            self.show_error(w, &error);
        }
        if self.workspace.editor().clipboard != before {
            w.set_clipboard_text(&self.workspace.editor().clipboard);
        }
        self.handle_actions(w);
        self.changed(w, Instant::now(), name != "scroll_lines");
    }
    fn save_tab(&mut self, w: &WindowHandle, index: usize, save_as: bool) -> bool {
        let path = if save_as || self.workspace.tabs[index].path.is_none() {
            let suggested = self.workspace.tabs[index]
                .path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Untitled.txt".into());
            let Some(path) = w.save_file(&suggested) else {
                return false;
            };
            Some(path)
        } else {
            None
        };
        match self.workspace.save(index, path) {
            Ok(()) => true,
            Err(error) => {
                self.show_error(w, &error);
                false
            }
        }
    }
    fn can_close(&mut self, w: &WindowHandle, index: usize) -> bool {
        self.workspace.tabs[index].editor.unmark();
        if !self.workspace.tabs[index].dirty() {
            return true;
        }
        match w.alert(
            "Save changes?",
            &self.workspace.tabs[index].label(),
            &["Save", "Cancel", "Discard"],
        ) {
            0 => self.save_tab(w, index, false),
            2 => true,
            _ => false,
        }
    }
    fn handle_actions(&mut self, w: &WindowHandle) {
        let actions = std::mem::take(&mut self.workspace.editor_mut().actions);
        for (name, args) in actions {
            let previous = self.workspace.active;
            self.workspace.tabs[previous].scroll = (self.view.scroll_x, self.view.scroll_y);
            let result = self.apply_action(w, &name, &args);
            if let Err(error) = result {
                self.show_error(w, &error);
            }
            if self.workspace.active != previous
                || matches!(
                    name.as_str(),
                    "open_file" | "prompt_open_file" | "new_file" | "close" | "close_file"
                )
            {
                let scroll = self.workspace.tabs[self.workspace.active].scroll;
                self.view.scroll_x = scroll.0;
                self.view.scroll_y = scroll.1;
            }
        }
        self.workspace.editor_mut().sync();
        let tab = &self.workspace.tabs[self.workspace.active];
        w.set_title(&format!(
            "{} — {} · {} — Velocity",
            tab.label(),
            tab.format.encoding,
            tab.format.line_ending.name()
        ));
    }
    fn apply_action(&mut self, w: &WindowHandle, name: &str, args: &Value) -> Result<(), String> {
        let active = self.workspace.active;
        match name {
            "new_file" => self.workspace.new_file()?,
            "open_file" => self.workspace.open(crate::cli::FileLocation::parse(
                args["file"].as_str().ok_or("open_file needs file")?,
            ))?,
            "prompt_open_file" => {
                for path in w.open_files() {
                    self.workspace.open(crate::cli::FileLocation {
                        path,
                        line: 1,
                        column: 1,
                    })?;
                }
            }
            "save" => {
                self.save_tab(w, active, false);
            }
            "prompt_save_as" => {
                self.save_tab(w, active, true);
            }
            "save_all" => {
                for index in 0..self.workspace.tabs.len() {
                    if !self.save_tab(w, index, false) {
                        break;
                    }
                }
            }
            "close" | "close_file" => {
                if self.can_close(w, active) {
                    self.workspace.close(active)?;
                }
            }
            "next_view" => self
                .workspace
                .select((active + 1) % self.workspace.tabs.len()),
            "prev_view" => self
                .workspace
                .select((active + self.workspace.tabs.len() - 1) % self.workspace.tabs.len()),
            "select_by_index" => self.workspace.select(
                args["index"]
                    .as_u64()
                    .ok_or("select_by_index needs index")? as usize,
            ),
            "new_window" => {
                std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
                    .spawn()
                    .map_err(|e| e.to_string())?;
            }
            "close_window" => {
                if self.on_quit_requested(w) {
                    w.quit();
                }
            }
            "increase_font_size" | "decrease_font_size" | "reset_font_size" => {
                let size = self.workspace.editor().settings.editor().font_size;
                let size = if name == "reset_font_size" {
                    13.
                } else {
                    (size
                        + if name == "increase_font_size" {
                            1.
                        } else {
                            -1.
                        })
                    .clamp(6., 96.)
                };
                self.workspace
                    .editor_mut()
                    .settings
                    .set(Layer::View, "font_size", json!(size));
            }
            "toggle_setting" => {
                let key = args["setting"]
                    .as_str()
                    .ok_or("toggle_setting needs setting")?;
                let value = self.workspace.editor().settings.bool(key, false);
                self.workspace
                    .editor_mut()
                    .settings
                    .set(Layer::View, key, json!(!value));
            }
            "set_line_endings" => {
                self.workspace.tabs[active].format.line_ending = vel_fileio::LineEnding::parse(
                    args["type"].as_str().ok_or("set_line_endings needs type")?,
                )?;
            }
            "set_encoding" => {
                let requested = args["encoding"]
                    .as_str()
                    .ok_or("set_encoding needs encoding")?;
                let codec = vel_fileio::encoding(requested)?;
                self.workspace.tabs[active].format.encoding = codec.name().into();
                self.workspace.tabs[active].format.bom = requested
                    .eq_ignore_ascii_case("utf-8 with bom")
                    || codec.name().starts_with("UTF-16");
            }
            "scroll_lines" => {
                let amount = args["amount"].as_f64().unwrap_or(1.);
                self.view
                    .scroll(self.workspace.editor(), 0., -amount * self.view.line_height);
            }
            "show_at_center" => {
                let caret = self
                    .view
                    .rect_at(self.workspace.editor(), self.workspace.editor().caret);
                self.view.scroll(
                    self.workspace.editor(),
                    0.,
                    caret.y - self.view.size.height / 2.,
                );
            }
            _ => return Err(format!("unknown window action {name}")),
        }
        Ok(())
    }
    fn tab_width(&self) -> f64 {
        (self.view.size.width / self.workspace.tabs.len() as f64).clamp(60., 200.)
    }
    fn scene(&mut self) -> vel_render::scene::Scene {
        let mut scene = self.view.scene(self.workspace.editor());
        let mut tabs = SceneLayer::default();
        let all = Rect {
            x: 0.,
            y: 0.,
            width: self.view.size.width,
            height: crate::view::TAB_HEIGHT,
        };
        tabs.rects.push(SolidRect {
            rect: all,
            radius: 0.,
            color: [0.1, 0.11, 0.14, 1.],
            clip: all,
        });
        for (index, tab) in self.workspace.tabs.iter().enumerate() {
            let x = index as f64 * self.tab_width();
            if x >= all.width {
                break;
            }
            let rect = Rect {
                x,
                y: 0.,
                width: self.tab_width() - 1.,
                height: all.height,
            };
            tabs.rects.push(SolidRect {
                rect,
                radius: 0.,
                color: if index == self.workspace.active {
                    [0.2, 0.22, 0.27, 1.]
                } else {
                    [0.12, 0.13, 0.16, 1.]
                },
                clip: all,
            });
            let mut label = tab.label();
            if let Some((read, total)) = tab.progress() {
                label = format!("Loading {}%", read.saturating_mul(100) / total.max(1));
            }
            tabs.text.push(TextRun {
                text: label,
                x: (x + 8.) as f32,
                baseline: 20.,
                size: 12.,
                color: [0.85, 0.88, 0.93, 1.],
                clip: Rect {
                    width: rect.width - 24.,
                    ..rect
                },
            });
            tabs.text.push(TextRun {
                text: "×".into(),
                x: (x + rect.width - 16.) as f32,
                baseline: 20.,
                size: 12.,
                color: [0.7, 0.72, 0.77, 1.],
                clip: rect,
            });
        }
        scene.layers.push(tabs);
        scene
    }
    fn log_input(&self, kind: &str, event: impl std::fmt::Debug) {
        if self.options.log_input {
            let _=self.reporter.emit(json!({"kind":"input","event_kind":kind,"event":format!("{event:?}"),"selected_utf16":[self.workspace.editor().selected_utf16().location,self.workspace.editor().selected_utf16().length],"marked_utf16":self.workspace.editor().marked_utf16().map(|r|[r.location,r.length])}));
        }
    }
    fn changed(&mut self, w: &WindowHandle, received: Instant, reveal: bool) {
        self.input_at.get_or_insert(received);
        if reveal {
            self.view.reveal(self.workspace.editor());
        }
        w.invalidate_character_coordinates();
        w.request_frame();
    }
    fn input(&mut self, w: &WindowHandle, input: TextInput, received: Instant) {
        self.log_input("text", &input);
        self.workspace.editor_mut().text_input(input);
        self.changed(w, received, true);
    }
    fn start_report(&mut self, w: &WindowHandle, p: Presentation) -> Result<(), String> {
        self.clock.mark_at("first_real_scanout", p.scanout);
        self.renderer
            .as_mut()
            .ok_or("renderer missing")?
            .release_prepared();
        // Milestones can be received after a later phase; sort by actual monotonic
        // timestamp before emitting, keeping callback receipt in a separate field.
        self.clock
            .milestones
            .sort_by(|a, b| a.main_ms.total_cmp(&b.main_ms));
        let mut previous = 0.;
        for m in &mut self.clock.milestones {
            m.delta_ms = m.main_ms - previous;
            previous = m.main_ms;
        }
        let renderer = self.renderer.as_ref().ok_or("renderer missing")?;
        self.reporter.emit(json!({"kind":"startup","milestones":self.clock.milestones,"before_main_ms":self.clock.before_main_ms,"process_birth_error":self.clock.birth_error,"process_birth_method":"proc_pidinfo(PROC_PIDTBSDINFO) anchored at main wall snapshot; monotonic deltas thereafter","actual_physical_size":renderer.physical_size(),"logical_size":[w.content_size().width,w.content_size().height],"scale_factor":w.scale_factor(),"maximum_frames_per_second":w.maximum_frames_per_second(),"present_mode":"fifo","scanout_main_ms":ms(p.scanout.saturating_duration_since(self.clock.entered)),"scanout_process_ms":self.clock.before_main_ms.map(|n|n+ms(p.scanout.saturating_duration_since(self.clock.entered))),"presented_time_seconds":p.presented_time,"presented_callback_main_ms":ms(p.callback.saturating_duration_since(self.clock.entered)),"scanout_to_callback_ms":ms(p.callback.saturating_duration_since(p.scanout)),"drawable":p.drawable,"startup_path":"offscreen text frame submitted before orderFront; acquire/copy/present immediately in the same applicationDidFinishLaunching callback","surface_retries":self.retries,"skipped_drawables":self.first_scanout.skipped_drawables,"first_callback_main_ms":self.first_scanout.first_callback.map(|t|ms(t.saturating_duration_since(self.clock.entered))),"first_real_scanout":p.presented_time,"boundary":"MTLDrawable.addPresentedHandler + presentedTime; queue.present return reported separately"}))?;
        self.reporter.emit(json!({"kind":"environment","renderer":renderer.info,"fonts":self.fonts.as_ref().map(Fonts::metadata),"atlas":renderer.atlas_info(),"log_input":self.options.log_input,"platform":"native AppKit objc2; macOS 14+","wgpu":"30.0.2"}))?;
        renderer.wait_idle()?;
        self.reporter.rss("after_first_frame",json!({"lines":self.workspace.editor().text.len_lines(),"bytes":self.workspace.editor().text.len_bytes(),"atlas":renderer.atlas_info()}))?;
        renderer.persist_pipeline_cache()?;
        self.startup_done = true;
        match self.options.mode {
            Mode::Startup => self.finish(w)?,
            Mode::Open => {
                if self.workspace.tabs.iter().all(|t| t.loading.is_none()) {
                    self.finish(w)?;
                } else {
                    w.request_frame_at(Instant::now() + Duration::from_millis(16));
                }
            }
            Mode::Interactive => {
                w.request_frame_at(Instant::now() + Duration::from_millis(200));
            }
            mode => {
                let mut bench = Bench::new(
                    mode,
                    self.options.seed,
                    sweep_positions(self.workspace.editor().text.len_lines(), self.view.rows()),
                );
                if mode == Mode::Typing {
                    let at = bench.schedule_next(Instant::now());
                    w.request_frame_at(at);
                } else {
                    w.set_animating(true);
                }
                self.bench = Some(bench);
            }
        }
        Ok(())
    }
    fn finish(&mut self, w: &WindowHandle) -> Result<(), String> {
        w.set_animating(false);
        self.renderer
            .as_ref()
            .ok_or("renderer missing")?
            .persist_pipeline_cache()?;
        self.reporter.emit(json!({"kind":"final_render_state","atlas":self.renderer.as_ref().map(Renderer::atlas_info),"fonts":self.fonts.as_ref().map(Fonts::metadata),"external_input_events":self.external_input}))?;
        self.reporter
            .emit(json!({"kind":"complete","complete":true,"output":self.reporter.path}))?;
        self.done.store(true, Ordering::Release);
        w.quit();
        Ok(())
    }
    fn bench_report(&mut self, w: &WindowHandle) -> Result<(), String> {
        let mode = self.options.mode;
        match mode {
            Mode::Scroll => {
                if self.samples.len() != SCROLL_FRAMES {
                    return Err(format!(
                        "expected {SCROLL_FRAMES} scroll samples, got {}",
                        self.samples.len()
                    ));
                }
                let gpu = self
                    .renderer
                    .as_ref()
                    .ok_or("renderer missing")?
                    .gpu_times()?;
                for (i, sample) in self.samples.iter().enumerate() {
                    self.reporter.emit(json!({"kind":"scroll_frame","sample":sample,"gpu_render_pass_ms":gpu.as_ref().and_then(|g|g.get(i))}))?;
                }
                let cpu: Vec<_> = self.samples.iter().map(|s| s.event_to_submit_ms).collect();
                let present: Vec<_> = self
                    .samples
                    .iter()
                    .map(|s| s.event_to_present_return_ms)
                    .collect();
                self.reporter.emit(json!({"kind":"scroll_summary","frames":self.samples.len(),"unit":"ms","cpu_event_to_submit":summarize(&cpu),"event_to_present_return":summarize(&present),"gpu_render_pass":gpu.as_deref().and_then(summarize),"gpu_status":if gpu.is_some(){"timestamp_query"}else{"TIMESTAMP_QUERY unavailable"},"gpu_scope":"render pass only: clear + all scene batches; excludes uploads, resolve/readback and presentation","readback":"deferred until all 600 measured frames submitted; 60 display-linked warmup frames excluded","surface_retries":self.retries,"external_input_events":self.external_input}))?;
            }
            Mode::Typing => {
                if self.samples.len() != TYPING_INSERTIONS {
                    return Err("typing benchmark incomplete".into());
                }
                for sample in &self.samples {
                    self.reporter
                        .emit(json!({"kind":"typing_sample","sample":sample}))?;
                }
                let submit: Vec<_> = self.samples.iter().map(|s| s.event_to_submit_ms).collect();
                let present: Vec<_> = self
                    .samples
                    .iter()
                    .map(|s| s.event_to_present_return_ms)
                    .collect();
                let late: Vec<_> = self
                    .samples
                    .iter()
                    .filter_map(|s| s.timer_lateness_ms)
                    .collect();
                self.reporter.emit(json!({"kind":"typing_summary","insertions":self.samples.len(),"unit":"ms","inject_to_submit":summarize(&submit),"inject_to_present_return":summarize(&present),"timer_lateness":summarize(&late),"seed":self.options.seed,"interval_ms_inclusive":[30,90],"carets":self.options.carets,"injection_boundary":"one-shot NSTimer -> keymap lookup -> command -> vel-buffer -> render; no OS/HID injection","presentation_boundary":"queue.present return; not scanout","surface_retries":self.retries,"external_input_events":self.external_input}))?;
            }
            Mode::Memory => {
                self.renderer
                    .as_ref()
                    .ok_or("renderer missing")?
                    .wait_idle()?;
                self.reporter.rss("after_full_sweep",json!({"viewports":self.bench.as_ref().map(|b|b.completed),"atlas":self.renderer.as_ref().map(Renderer::atlas_info),"lines":self.workspace.editor().text.len_lines(),"bytes":self.workspace.editor().text.len_bytes()}))?;
                self.reporter.emit(json!({"kind":"memory_summary","sweep_complete":true,"visited_viewports":self.bench.as_ref().map(|b|b.completed),"external_input_events":self.external_input}))?;
            }
            _ => {}
        }
        self.reporter
            .rss("after_benchmark", json!({"mode":mode.name()}))?;
        self.finish(w)
    }
    fn record(
        &mut self,
        w: &WindowHandle,
        times: FrameTimes,
        received: Instant,
    ) -> Result<(), String> {
        if !self.first_present {
            self.first_present = true;
            self.clock.mark_at("first_submit", times.submitted);
            self.clock
                .mark_at("first_present_returned", times.presented);
            w.request_frame_at(Instant::now() + Duration::from_millis(2));
            return Ok(());
        }
        if let Some(b) = self.bench.as_mut() {
            if !b.pending {
                return Ok(());
            }
            let at = self.bench_received.take().unwrap_or(received);
            if b.warmup == 0 && self.options.mode != Mode::Memory {
                self.samples.push(Sample {
                    index: b.completed,
                    event_to_submit_ms: ms(times.submitted.saturating_duration_since(at)),
                    event_to_present_return_ms: ms(times.presented.saturating_duration_since(at)),
                    acquire_ms: times.acquire_ms,
                    encode_submit_ms: times.encode_submit_ms,
                    present_call_ms: times.present_call_ms,
                    timer_lateness_ms: b
                        .due
                        .take()
                        .map(|due| ms(at.saturating_duration_since(due))),
                });
            }
            b.submitted();
            self.input_at = None;
            if b.finished() {
                return self.bench_report(w);
            }
            if b.mode == Mode::Typing {
                let deadline = b.schedule_next(at);
                w.request_frame_at(deadline);
            }
        } else if let Some(at) = self.input_at.take()
            && self.options.log_input
        {
            self.reporter.emit(json!({"kind":"input_latency","receipt_to_submit_ms":ms(times.submitted.saturating_duration_since(at)),"receipt_to_present_return_ms":ms(times.presented.saturating_duration_since(at))}))?;
        }
        Ok(())
    }
}
impl AppHandler for App {
    fn on_platform_phase(&mut self, phase: &'static str) {
        self.clock.mark(phase);
    }
    fn on_start(&mut self, w: &WindowHandle) -> Result<(), String> {
        w.set_menu(&native_menu(&self.workspace.menus));
        self.reporter.emit(json!({"kind":"resources","key_bindings":self.workspace.editor().keymap.len(),"mouse_bindings":self.workspace.mousemap.len(),"palette_entries":self.workspace.palette.len()}))?;
        let fonts = Fonts::load_named(&self.workspace.editor().settings.editor().font_face)?;
        self.clock.mark("primary_font_ready");
        self.view = View::new(w.content_size(), fonts.cell as f64, fonts.ascent as f64);
        self.view.reveal(self.workspace.editor());
        let cache_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("output/pipeline-cache");
        self.renderer = Some(Renderer::new(
            w.clone(),
            self.options.mode == Mode::Scroll,
            &cache_dir,
            |phase| self.clock.mark(phase),
        )?);
        for record in std::mem::take(&mut self.workspace.telemetry) {
            self.reporter.emit(record)?;
        }
        self.font_face = self.workspace.editor().preferences.font_face.clone();
        self.fonts = Some(fonts);
        Ok(())
    }
    fn on_key(&mut self, w: &WindowHandle, event: KeyEvent) -> bool {
        self.log_input("key", &event);
        if self.options.mode.automatic() {
            if event.state == KeyState::Down {
                self.external_input += 1;
            }
            return true;
        }
        let Some(key) = crate::editor::binding_key(&event) else {
            return false;
        };
        match self.workspace.editor_mut().resolve_key(&key) {
            vel_config::keymap::Resolution::Unbound => false,
            vel_config::keymap::Resolution::Pending => true,
            vel_config::keymap::Resolution::Command(invocation) => {
                self.run_named(w, &invocation.command, &invocation.args);
                self.input_at = Some(event.received);
                true
            }
        }
    }
    fn on_named_command(&mut self, w: &WindowHandle, name: &str, args: &str) {
        if !self.options.mode.automatic() {
            match serde_json::from_str(args) {
                Ok(args) => self.run_named(w, name, &args),
                Err(e) => self.show_error(w, &e.to_string()),
            }
        }
    }
    fn command_enabled(&self, name: &str, args: &str) -> bool {
        let Ok(args) = serde_json::from_str::<Value>(args) else {
            return false;
        };
        self.workspace
            .editor()
            .registry
            .is_enabled(name, &args, self.workspace.editor())
            && !(self.workspace.tabs[self.workspace.active].loading.is_some()
                && (self.workspace.editor().registry.kind(name)
                    == Some(vel_commands::Kind::TextCommand)
                    || matches!(name, "save" | "save_all")))
    }
    fn on_text_input(&mut self, w: &WindowHandle, event: TextInput) {
        if self.options.mode.automatic() {
            self.external_input += 1;
            return;
        }
        if self.workspace.editor().read_only {
            return;
        }
        self.input(w, event, Instant::now());
    }
    fn on_command(&mut self, w: &WindowHandle, command: Command) {
        self.log_input("command", command);
        if self.options.mode.automatic() {
            self.external_input += 1;
            return;
        }
        if command == Command::Cancel && self.workspace.editor().marked.is_some() {
            self.workspace.editor_mut().cancel_composition();
            self.changed(w, Instant::now(), true);
            return;
        }
        let (name, args) = crate::editor::selector_command(command);
        self.run_named(w, name, &args);
    }
    fn on_mouse(&mut self, w: &WindowHandle, event: MouseEvent) {
        if self.options.mode.automatic() {
            self.external_input += 1;
            return;
        }
        if event.kind == MouseKind::Down && event.position.y < crate::view::TAB_HEIGHT {
            let index = (event.position.x / self.tab_width()).max(0.) as usize;
            if index < self.workspace.tabs.len() {
                if event.position.x % self.tab_width() > self.tab_width() - 20. {
                    if self.can_close(w, index) {
                        let _ = self.workspace.close(index);
                    }
                } else {
                    self.workspace.tabs[self.workspace.active].scroll =
                        (self.view.scroll_x, self.view.scroll_y);
                    self.workspace.select(index);
                }
                let scroll = self.workspace.tabs[self.workspace.active].scroll;
                self.view.scroll_x = scroll.0;
                self.view.scroll_y = scroll.1;
                self.changed(w, Instant::now(), false);
            }
            return;
        }
        let command = self
            .workspace
            .mousemap
            .iter()
            .rev()
            .find(|binding| {
                let native_button = match event.button {
                    0 => 1,
                    1 => 2,
                    2 => 3,
                    n => n + 1,
                };
                let expected = [
                    ("ctrl", event.modifiers.control()),
                    ("alt", event.modifiers.option()),
                    ("shift", event.modifiers.shift()),
                    ("super", event.modifiers.command()),
                ]
                .into_iter()
                .filter(|(_, on)| *on)
                .map(|(name, _)| name)
                .collect::<std::collections::BTreeSet<_>>();
                let modifiers = binding
                    .modifiers
                    .iter()
                    .map(String::as_str)
                    .collect::<std::collections::BTreeSet<_>>();
                binding.button == native_button
                    && binding.count == event.click_count
                    && modifiers == expected
                    && self.workspace.editor().context_matches(&binding.context)
            })
            .and_then(|b| match event.kind {
                MouseKind::Down => b
                    .press_command
                    .as_ref()
                    .map(|n| (n.clone(), b.press_args.clone())),
                MouseKind::Up => b.command.as_ref().map(|n| (n.clone(), b.args.clone())),
                _ => None,
            });
        if let Some((name, args)) = command {
            self.run_named(w, &name, &args);
            return;
        }
        if event.button != 0 {
            return;
        }
        let at = self.view.hit_test(self.workspace.editor(), event.position);
        match event.kind {
            MouseKind::Down if event.click_count >= 3 => {
                self.workspace.editor_mut().select_line(at)
            }
            MouseKind::Down if event.click_count == 2 => {
                self.workspace.editor_mut().select_word(at)
            }
            MouseKind::Down if event.modifiers.command() => {
                self.workspace.editor_mut().add_caret(at)
            }
            MouseKind::Down => self
                .workspace
                .editor_mut()
                .set_caret(at, event.modifiers.shift()),
            MouseKind::Drag => self.workspace.editor_mut().set_caret(at, true),
            MouseKind::Up => {}
        }
        self.changed(w, Instant::now(), false);
    }
    fn on_scroll(&mut self, w: &WindowHandle, event: ScrollEvent) {
        if self.options.mode.automatic() {
            self.external_input += 1;
            return;
        }
        let scale = if event.precise {
            1.
        } else {
            self.view.line_height
        };
        self.view.scroll(
            self.workspace.editor(),
            -event.delta_x * scale,
            -event.delta_y * scale,
        );
        self.changed(w, Instant::now(), false);
    }
    fn on_resize(&mut self, w: &WindowHandle, size: Size, scale: f64) {
        self.view.size = size;
        self.view.clamp_scroll(self.workspace.editor());
        if let Some(r) = self.renderer.as_mut() {
            r.resize(
                (size.width * scale).round() as u32,
                (size.height * scale).round() as u32,
            );
        }
        w.request_frame();
    }
    fn on_appearance(&mut self, w: &WindowHandle, appearance: Appearance) {
        self.view.appearance = appearance;
        w.request_frame();
    }
    fn on_frame(&mut self, ctx: &mut FrameContext<'_>) -> Result<(), String> {
        if self.done.load(Ordering::Acquire) {
            return Ok(());
        }
        let w = ctx.window;
        if self.startup_done && matches!(self.options.mode, Mode::Interactive | Mode::Open) {
            let loading_before = self.workspace.tabs.iter().any(|t| t.loading.is_some());
            let active_was_loading = self.workspace.tabs[self.workspace.active].loading.is_some();
            let changed = self.workspace.poll();
            if active_was_loading && self.workspace.tabs[self.workspace.active].loading.is_none() {
                self.view.reveal(self.workspace.editor());
            }
            if changed {
                w.set_menu(&native_menu(&self.workspace.menus));
            }
            for record in std::mem::take(&mut self.workspace.telemetry) {
                self.reporter.emit(record)?;
            }
            if self.options.mode == Mode::Open
                && self.workspace.tabs.iter().all(|t| t.loading.is_none())
            {
                if !self.workspace.messages.is_empty() {
                    return Err(self.workspace.messages.join("; "));
                }
                return self.finish(w);
            }
            let conflicts: Vec<_> = self
                .workspace
                .tabs
                .iter()
                .enumerate()
                .filter(|(_, t)| t.external.is_some())
                .map(|(i, _)| i)
                .collect();
            for index in conflicts {
                let reload = w.alert(
                    "File changed on disk",
                    &format!(
                        "{} has unsaved edits. Reloading discards them.",
                        self.workspace.tabs[index].label()
                    ),
                    &["Keep Edits", "Reload"],
                ) == 1;
                self.workspace.reload_external(index, reload);
            }
            for message in std::mem::take(&mut self.workspace.messages) {
                self.show_error(w, &message);
            }
            w.request_frame_at(Instant::now() + Duration::from_millis(200));
            if ctx.reason == FrameReason::Timer && !changed && !loading_before {
                return Ok(());
            }
        }
        if self.font_face != self.workspace.editor().preferences.font_face {
            let name = self.workspace.editor().preferences.font_face.clone();
            match Fonts::load_named(&name) {
                Ok(fonts) => {
                    self.view = View::new(self.view.size, fonts.cell as f64, fonts.ascent as f64);
                    self.fonts = Some(fonts);
                    self.renderer
                        .as_mut()
                        .ok_or("renderer missing")?
                        .reset_fonts();
                }
                Err(error) => self.show_error(w, &error),
            }
            self.font_face = name;
        }
        let observations = w.take_presentations();
        if !self.startup_done {
            let before_callback = self.first_scanout.first_callback;
            let skipped = self.first_scanout.skipped_drawables;
            let real = self.first_scanout.observe(&observations);
            if before_callback.is_none()
                && let Some(callback) = self.first_scanout.first_callback
            {
                self.clock.mark_at("first_presented_callback", callback);
            }
            if skipped != self.first_scanout.skipped_drawables {
                self.reporter.emit(json!({"kind":"drawable_skipped","skipped_drawables":self.first_scanout.skipped_drawables,"first_callback_main_ms":self.first_scanout.first_callback.map(|t|ms(t.saturating_duration_since(self.clock.entered)))}))?;
            }
            if let Some(p) = real {
                self.start_report(w, p)?;
                return Ok(());
            }
        }
        if ctx.reason == FrameReason::StartupPrepare {
            let scene = self.scene();
            self.clock.mark("first_scene_ready");
            self.renderer
                .as_mut()
                .ok_or("renderer missing")?
                .pre_render(
                    &scene,
                    self.fonts.as_mut().ok_or("fonts missing")?,
                    w.scale_factor() as f32,
                )?;
            self.clock.mark("first_offscreen_submitted");
            return Ok(());
        }
        if !self.startup_done {
            let waiting_since = *self.waiting_since.get_or_insert_with(Instant::now);
            let status = w.display_status();
            // AppKit can report occluded briefly immediately after orderFront.
            if status.display_asleep
                || status.screen_locked
                || waiting_since.elapsed() >= Duration::from_millis(1000)
            {
                status.check()?;
            }
            // orderFront may constrain an oversized window or change its backing
            // scale. Rebuild immediately in this same callback if that invalidated
            // the prepared texture; never copy stale-sized pixels or await redraw.
            if !self
                .renderer
                .as_ref()
                .ok_or("renderer missing")?
                .has_prepared_frame()
            {
                let scene = self.scene();
                self.renderer
                    .as_mut()
                    .ok_or("renderer missing")?
                    .pre_render(
                        &scene,
                        self.fonts.as_mut().ok_or("fonts missing")?,
                        w.scale_factor() as f32,
                    )?;
                self.clock
                    .mark("first_frame_reprepared_after_native_resize");
            }
            if let Some(times) = self
                .renderer
                .as_mut()
                .ok_or("renderer missing")?
                .present_prepared()?
            {
                self.record(w, times, ctx.received)?;
                w.request_frame_at(Instant::now() + Duration::from_millis(16));
            } else {
                self.retries += 1;
                w.request_frame_at(Instant::now() + Duration::from_millis(16));
            }
            return Ok(());
        }
        let mut query = None;
        if let Some(b) = self.bench.as_mut() {
            if b.mode == Mode::Typing {
                if !b.pending && !b.typing_due(ctx.received) {
                    if let Some(at) = b.next {
                        w.request_frame_at(at);
                    }
                    return Ok(());
                }
                if self.bench_received.is_none() {
                    let text = match b.completed % 8 {
                        0 => "你",
                        1 => "🦀",
                        2 => "é",
                        3 => "\n",
                        _ => "x",
                    };
                    self.bench_received = Some(Instant::now());
                    crate::type_through_keymap(self.workspace.editor_mut(), text)?;
                    self.changed(w, self.bench_received.unwrap(), true);
                }
            } else if !b.pending {
                b.pending = true;
                self.bench_received = Some(ctx.received);
                if b.mode == Mode::Memory {
                    self.view.scroll_y = b.positions[b.completed] as f64 * self.view.line_height;
                    self.view.clamp_scroll(self.workspace.editor());
                } else {
                    let max = (self.workspace.editor().text.len_lines() as f64
                        * self.view.line_height
                        - self.view.size.height
                        + 8.)
                        .max(0.);
                    if self.view.scroll_y + 20. >= max {
                        self.view.scroll_y = 0.;
                    }
                    self.view.scroll(self.workspace.editor(), 0., 20.);
                }
            }
            if let Some(b) = self.bench.as_ref()
                && b.mode == Mode::Scroll
                && b.warmup == 0
            {
                query = Some(b.completed as u32);
            }
        }
        let scene = self.scene();
        let result = self.renderer.as_mut().ok_or("renderer missing")?.render(
            &scene,
            self.fonts.as_mut().ok_or("fonts missing")?,
            w.scale_factor() as f32,
            query,
        )?;
        if let Some(times) = result {
            self.record(w, times, ctx.received)?;
        } else {
            self.retries += 1;
            w.request_frame_at(Instant::now() + Duration::from_millis(16));
        }
        Ok(())
    }
    fn on_quit_requested(&mut self, w: &WindowHandle) -> bool {
        if self.options.mode == Mode::Interactive {
            for index in 0..self.workspace.tabs.len() {
                if !self.can_close(w, index) {
                    return false;
                }
            }
        }
        let record = if self.options.mode.automatic() {
            json!({"kind":"error","stage":"user_close","message":"automatic benchmark closed before completion","complete":false})
        } else {
            self.done.store(true, Ordering::Release);
            json!({"kind":"complete","complete":true})
        };
        let _ = self.reporter.emit(record);
        true
    }
    fn text_input_state(&self) -> TextInputState {
        TextInputState {
            text: self.workspace.editor().text.clone(),
            selected: self.workspace.editor().selected_utf16(),
            marked: self.workspace.editor().marked_utf16(),
            caret: self
                .view
                .rect_at(self.workspace.editor(), self.workspace.editor().caret),
        }
    }
    fn text_input_rect(&self, range: Utf16Range) -> (Rect, Utf16Range) {
        self.view.range_rect(self.workspace.editor(), range)
    }
    fn character_index_for_point(&self, point: Point) -> Option<usize> {
        Some(
            self.workspace
                .editor()
                .text
                .byte_to_utf16(self.view.hit_test(self.workspace.editor(), point)),
        )
    }
}

fn native_menu(items: &[vel_config::ui::MenuItem]) -> Vec<MenuEntry> {
    items
        .iter()
        .filter(|item| {
            item.platform.as_deref() != Some("!OSX")
                && item.platform.as_deref() != Some("Windows")
                && item.platform.as_deref() != Some("Linux")
        })
        .map(|item| MenuEntry {
            caption: item.caption.clone(),
            command: item.command.clone(),
            args: item.args.to_string(),
            children: native_menu(&item.children),
        })
        .collect()
}
