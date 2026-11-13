//! Event-driven shell and unattended benchmark state machine.
use crate::{
    cli::{Mode, Options},
    editor::{Editor, Motion},
    font::Fonts,
    layout::View,
    render::{Renderer, SCROLL_FRAMES},
    report::{Clock, Reporter, ms},
    stats::{TypingSchedule, summarize, sweep_positions},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalPosition, LogicalSize},
    event::{ElementState, Ime, MouseButton, MouseScrollDelta, StartCause, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{Window, WindowId},
};

struct State {
    renderer: Renderer,
    fonts: Fonts,
    editor: Editor,
    view: View,
    scale: f32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    First,
    Scroll,
    Typing,
    Sweep,
    Interactive,
    Done,
}
struct Pending {
    id: u64,
    at: Instant,
    source: &'static str,
    action: String,
    scheduled: Option<Instant>,
}
#[derive(Serialize)]
struct Latency {
    id: u64,
    source: &'static str,
    action: String,
    event_main_ms: f64,
    event_to_submit_ms: f64,
    event_to_present_return_ms: f64,
    timer_lateness_ms: Option<f64>,
    frame: u64,
}
#[derive(Serialize)]
struct ScrollFrame {
    index: u32,
    event_to_submit_ms: f64,
    event_to_present_return_ms: f64,
    acquire_ms: f64,
    encode_submit_ms: f64,
    present_call_ms: f64,
    since_previous_present_return_ms: Option<f64>,
    actual_scroll_points: f32,
    glyph_instances: u32,
    rect_instances: u32,
    physical_size: [u32; 2],
    suboptimal: bool,
}

pub fn run(options: Options, clock: Clock, reporter: Reporter) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let mut clock = clock;
    clock.mark("event_loop_ready");
    let mut app = App {
        jitter: TypingSchedule::new(options.seed),
        options,
        clock,
        reporter,
        window: None,
        state: None,
        error: None,
        stage: Stage::First,
        dirty: true,
        redraw_pending: false,
        retry_at: None,
        modifiers: ModifiersState::empty(),
        pending: Vec::new(),
        latencies: Vec::new(),
        scroll_frames: Vec::new(),
        input_id: 0,
        frame_id: 0,
        injected: 0,
        next_injection: None,
        sweep: Vec::new(),
        sweep_index: 0,
        sweep_covered: 0,
        scroll_pending: None,
        atlas_warmed: false,
        mouse_down: false,
        mouse: [0., 0.],
        external_input: 0,
        occluded: false,
        last_scroll_present: None,
        surface_retries: 0,
    };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    if let Some(error) = app.error {
        return Err(error);
    }
    if app.options.mode.automatic() && app.stage != Stage::Done {
        return Err("event loop exited before automatic mode completed".into());
    }
    Ok(())
}
struct App {
    options: Options,
    clock: Clock,
    reporter: Reporter,
    window: Option<Arc<Window>>,
    state: Option<State>,
    error: Option<String>,
    stage: Stage,
    dirty: bool,
    redraw_pending: bool,
    retry_at: Option<Instant>,
    modifiers: ModifiersState,
    pending: Vec<Pending>,
    latencies: Vec<Latency>,
    scroll_frames: Vec<ScrollFrame>,
    input_id: u64,
    frame_id: u64,
    jitter: TypingSchedule,
    injected: u32,
    next_injection: Option<Instant>,
    sweep: Vec<usize>,
    sweep_index: usize,
    sweep_covered: usize,
    scroll_pending: Option<f32>,
    atlas_warmed: bool,
    mouse_down: bool,
    mouse: [f32; 2],
    external_input: u64,
    occluded: bool,
    last_scroll_present: Option<Instant>,
    surface_retries: u64,
}
impl App {
    fn fail(&mut self, event_loop: &ActiveEventLoop, message: String) {
        let _=self.reporter.emit(json!({"kind":"error","stage":format!("{:?}",self.stage),"message":message,"complete":false}));
        eprintln!("vel-shell: {message}");
        self.error = Some(message);
        event_loop.exit();
    }
    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Velocity · macOS shell prototype")
                        .with_inner_size(LogicalSize::new(self.options.width, self.options.height)),
                )
                .map_err(|e| e.to_string())?,
        );
        self.clock.mark("window_created");
        window.set_ime_allowed(true);
        let renderer = Renderer::new(
            Arc::clone(&window),
            self.options.present,
            self.options.mode == Mode::Scroll,
            &mut self.clock,
        )?;
        let editor = Editor::load(self.options.file.as_deref(), self.options.lines)?;
        self.clock.mark("file_loaded");
        let fonts = Fonts::load()?;
        self.clock.mark("font_loaded");
        let scale = window.scale_factor() as f32;
        let logical = window.inner_size().to_logical::<f32>(scale as f64);
        let view = View::new(logical.width, logical.height, fonts.cell, fonts.ascent);
        self.window = Some(window);
        self.state = Some(State {
            renderer,
            editor,
            fonts,
            view,
            scale,
        });
        self.update_ime_area();
        Ok(())
    }
    fn update_ime_area(&self) {
        if let Some((s, w)) = self.state.as_ref().zip(self.window.as_ref()) {
            let r = s.view.candidate_rect(&s.editor);
            w.set_ime_cursor_area(
                LogicalPosition::new(r.x as f64, r.y as f64),
                LogicalSize::new(r.w as f64, r.h as f64),
            );
        }
    }
    fn dirty(&mut self) {
        self.dirty = true;
        self.update_ime_area();
    }
    /// This is the common event-handler entry for native and timer-injected editing.
    fn input(
        &mut self,
        at: Instant,
        source: &'static str,
        action: Edit,
        scheduled: Option<Instant>,
    ) {
        let Some(s) = self.state.as_mut() else {
            return;
        };
        let label = match action {
            Edit::Text(ref text) => {
                s.editor.insert(text);
                format!("insert:{text}")
            }
            Edit::Backspace => {
                s.editor.backspace();
                "backspace".into()
            }
            Edit::Move(m, select) => {
                s.editor.move_cursor(m, select);
                format!("move:{m:?}")
            }
        };
        s.view.reveal(&s.editor);
        if source == "synthetic" || self.options.log_input {
            self.input_id += 1;
            self.pending.push(Pending {
                id: self.input_id,
                at,
                source,
                action: label,
                scheduled,
            });
        }
        self.dirty();
    }
    fn log_event(&self, event: &str, data: Value, at: Instant) {
        if self.options.log_input && let Err(e)=self.reporter.emit(json!({"kind":"input_event","event":event,"event_main_ms":ms(at-self.clock.entered),"data":data})) {eprintln!("input logging failed: {e}");}
    }
    fn ime(&mut self, event: Ime, at: Instant) {
        self.log_event("ime", json!({"event":format!("{event:?}")}), at);
        if self.options.mode.automatic() {
            if matches!(&event,Ime::Preedit(text,_)|Ime::Commit(text) if !text.is_empty()) {
                self.external_input += 1;
            }
            return;
        }
        let Some(s) = self.state.as_mut() else {
            return;
        };
        let label = match event {
            Ime::Enabled => {
                s.editor.ime.enabled = true;
                "enabled"
            }
            Ime::Preedit(text, range) => {
                s.editor.ime.update(text, range);
                s.view.reveal(&s.editor);
                "preedit"
            }
            Ime::Commit(text) => {
                s.editor.commit_ime(&text);
                s.view.reveal(&s.editor);
                "commit"
            }
            Ime::Disabled => {
                s.editor.ime.enabled = false;
                s.editor.ime.clear();
                "disabled"
            }
        };
        if self.options.log_input {
            self.input_id += 1;
            self.pending.push(Pending {
                id: self.input_id,
                at,
                source: "ime",
                action: label.into(),
                scheduled: None,
            });
        }
        self.dirty();
    }
    fn timer(&mut self) {
        if self.stage != Stage::Typing {
            return;
        }
        while self.injected < 500
            && self
                .next_injection
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            let scheduled = self.next_injection.unwrap();
            let received = Instant::now();
            let text = char::from(b'a' + (self.injected % 26) as u8).to_string();
            self.input(received, "synthetic", Edit::Text(text), Some(scheduled));
            self.injected += 1;
            self.next_injection = if self.injected < 500 {
                Some(scheduled + Duration::from_millis(self.jitter.next_delay_ms()))
            } else {
                None
            };
        }
    }
    fn redraw(&mut self, event_loop: &ActiveEventLoop, received: Instant) -> Result<(), String> {
        self.redraw_pending = false;
        let Some(window) = self.window.as_ref().cloned() else {
            return Ok(());
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            self.dirty = false;
            self.retry_at = None;
            return Ok(());
        }
        let Some(s) = self.state.as_mut() else {
            return Ok(());
        };
        if self.stage == Stage::Scroll && self.scroll_pending.is_none() {
            let before = s.view.scroll_y;
            s.view.scroll(0., s.view.line_height, s.editor.lines.len());
            self.scroll_pending = Some(s.view.scroll_y - before);
        }
        if self.stage == Stage::Sweep
            && let Some(&row) = self.sweep.get(self.sweep_index)
        {
            s.view.scroll_y = row as f32 * s.view.line_height;
            s.view.clamp_scroll(s.editor.lines.len());
        }
        let scene = s.view.scene(&mut s.editor);
        window.set_ime_cursor_area(
            LogicalPosition::new(scene.ime_rect.x as f64, scene.ime_rect.y as f64),
            LogicalSize::new(scene.ime_rect.w as f64, scene.ime_rect.h as f64),
        );
        let covered = scene.first_row + scene.visible_rows;
        let prepared = s
            .renderer
            .prepare(&scene, &mut s.fonts, s.scale, s.view.gutter)?;
        if !self.atlas_warmed {
            self.clock.mark("atlas_warmed");
            self.atlas_warmed = true;
        }
        let query = if self.stage == Stage::Scroll {
            Some(self.scroll_frames.len() as u32)
        } else {
            None
        };
        let result = match s.renderer.draw(&prepared, query, &window) {
            Err(e) if e == "surface_lost" => {
                s.renderer.recreate_surface(Arc::clone(&window))?;
                None
            }
            r => r?,
        };
        let Some(frame) = result else {
            self.surface_retries += 1;
            self.retry_at = Some(Instant::now() + Duration::from_millis(16));
            self.dirty = true;
            return Ok(());
        };
        self.retry_at = None;
        self.dirty = false;
        self.frame_id += 1;
        for pending in self.pending.drain(..) {
            let latency = Latency {
                id: pending.id,
                source: pending.source,
                action: pending.action,
                event_main_ms: ms(pending.at - self.clock.entered),
                event_to_submit_ms: ms(frame.submitted - pending.at),
                event_to_present_return_ms: ms(frame.presented - pending.at),
                timer_lateness_ms: pending
                    .scheduled
                    .map(|due| ms(pending.at.saturating_duration_since(due))),
                frame: self.frame_id,
            };
            if latency.source != "synthetic" {
                self.reporter.emit(json!({"kind":"input_latency","sample":latency,"boundary":"event receive -> queue submit / queue.present return; not display scanout"}))?;
            }
            self.latencies.push(latency);
        }
        match self.stage {
            Stage::First => {
                self.clock.mark_at("first_frame_encoded", frame.encoded);
                self.clock
                    .mark_at("first_present_returned", frame.presented);
                self.first_frame_report()?;
                match self.options.mode {
                    Mode::Startup => self.complete(event_loop)?,
                    Mode::Scroll => {
                        self.stage = Stage::Scroll;
                        self.dirty = true;
                    }
                    Mode::Typing => {
                        self.stage = Stage::Typing;
                        self.next_injection = Some(
                            Instant::now() + Duration::from_millis(self.jitter.next_delay_ms()),
                        );
                    }
                    Mode::Memory => self.start_sweep(),
                    Mode::Interactive => self.stage = Stage::Interactive,
                    Mode::Metal => unreachable!("raw Metal has its own handler"),
                }
            }
            Stage::Scroll => {
                self.scroll_frames.push(ScrollFrame {
                    index: self.scroll_frames.len() as u32,
                    event_to_submit_ms: ms(frame.submitted - received),
                    event_to_present_return_ms: ms(frame.presented - received),
                    acquire_ms: frame.acquire_ms,
                    encode_submit_ms: frame.encode_submit_ms,
                    present_call_ms: frame.present_call_ms,
                    since_previous_present_return_ms: self
                        .last_scroll_present
                        .map(|previous| ms(frame.presented - previous)),
                    actual_scroll_points: self.scroll_pending.take().unwrap_or(0.),
                    glyph_instances: prepared.glyph_count,
                    rect_instances: prepared.rect_count,
                    physical_size: [size.width, size.height],
                    suboptimal: frame.suboptimal,
                });
                self.last_scroll_present = Some(frame.presented);
                if self.scroll_frames.len() == SCROLL_FRAMES as usize {
                    self.scroll_report()?;
                    self.start_sweep();
                } else {
                    self.dirty = true;
                }
            }
            Stage::Typing if self.injected == 500 && self.pending.is_empty() => {
                self.typing_report()?;
                self.complete(event_loop)?;
            }
            Stage::Sweep => {
                self.sweep_covered = self.sweep_covered.max(covered);
                self.sweep_index += 1;
                if self.sweep_index == self.sweep.len() {
                    let s = self.state.as_ref().unwrap();
                    s.renderer.wait_idle()?;
                    self.reporter.rss("after_whole_file_scroll",json!({"loaded_lines":s.editor.lines.len(),"covered_through_line":self.sweep_covered,"sweep_frames":self.sweep_index,"whole_loaded_buffer_visited":self.sweep_covered>=s.editor.lines.len(),"file_was_truncated":s.editor.truncated,"atlas":s.renderer.atlas_info()}))?;
                    self.complete(event_loop)?;
                } else {
                    self.dirty = true;
                }
            }
            _ => {}
        }
        self.update_ime_area();
        Ok(())
    }
    fn first_frame_report(&self) -> Result<(), String> {
        let s = self.state.as_ref().unwrap();
        let window = self.window.as_ref().unwrap();
        s.renderer.wait_idle()?;
        let size = window.inner_size();
        let monitor = window.current_monitor();
        self.reporter.rss("after_first_frame",json!({"loaded_lines":s.editor.lines.len(),"source":s.editor.source,"file_was_truncated":s.editor.truncated,"atlas":s.renderer.atlas_info()}))?;
        self.reporter.emit(json!({"kind":"environment","requested_logical_size":[self.options.width,self.options.height],"actual_physical_size":[size.width,size.height],"actual_logical_size":[s.view.width,s.view.height],"scale_factor":s.scale,"monitor_name":monitor.as_ref().and_then(|m|m.name()),"monitor_refresh_millihertz":monitor.as_ref().and_then(|m|m.refresh_rate_millihertz()),"monitor_physical_size":monitor.as_ref().map(|m|[m.size().width,m.size().height]),"renderer":s.renderer.info,"fonts":s.fonts.metadata(),"input_file":s.editor.source,"loaded_bytes":s.editor.bytes_loaded,"loaded_lines":s.editor.lines.len(),"truncated":s.editor.truncated,"seed":self.options.seed,"log_input":self.options.log_input,"os":std::env::consts::OS,"arch":std::env::consts::ARCH}))?;
        self.reporter.emit(json!({"kind":"startup","milestones":self.clock.milestones,"process_birth_method":"proc_pidinfo(PROC_PIDTBSDINFO) wall birth, anchored to main wall snapshot then monotonic Instant deltas","process_birth_error":self.clock.birth_error,"before_main_ms":self.clock.before_main_ms,"boundary":"first queue.present() return, not compositor completion or display scanout","atlas_warmed_means":"CPU rasterized + queue.write_texture scheduled; upload submitted with first frame"}))
    }
    fn start_sweep(&mut self) {
        let s = self.state.as_ref().unwrap();
        self.sweep = sweep_positions(s.editor.lines.len(), s.view.fully_visible_rows());
        self.sweep_index = 0;
        self.sweep_covered = 0;
        self.stage = Stage::Sweep;
        self.dirty = true;
    }
    fn scroll_report(&self) -> Result<(), String> {
        let gpu = self.state.as_ref().unwrap().renderer.gpu_times()?;
        if let Some(times) = &gpu
            && times.len() != self.scroll_frames.len()
        {
            return Err("GPU sample count mismatch".into());
        }
        for (i, sample) in self.scroll_frames.iter().enumerate() {
            self.reporter.emit(json!({"kind":"scroll_frame","sample":sample,"gpu_render_pass_ms":gpu.as_ref().map(|g|g[i])}))?;
        }
        let samples =
            |f: fn(&ScrollFrame) -> f64| self.scroll_frames.iter().map(f).collect::<Vec<_>>();
        self.reporter.emit(json!({"kind":"scroll_summary","frames":self.scroll_frames.len(),"unit":"ms","cpu_event_to_submit":summarize(&samples(|s|s.event_to_submit_ms)),"event_to_present_return":summarize(&samples(|s|s.event_to_present_return_ms)),"surface_acquire":summarize(&samples(|s|s.acquire_ms)),"encode_submit":summarize(&samples(|s|s.encode_submit_ms)),"present_call":summarize(&samples(|s|s.present_call_ms)),"present_return_interval":summarize(&self.scroll_frames.iter().filter_map(|s|s.since_previous_present_return_ms).collect::<Vec<_>>()),"surface_retries":self.surface_retries,"gpu_render_pass":gpu.as_ref().and_then(|v|summarize(v)),"gpu_status":if gpu.is_some(){"timestamp_query"}else{"TIMESTAMP_QUERY unavailable on this adapter"},"gpu_scope":"render pass: clear + rects + glyphs; excludes atlas/buffer uploads, resolve/readback, presentation","readback":"deferred until all 600 frames are submitted; no per-frame GPU wait","boundary_clamped_frames":self.scroll_frames.iter().filter(|s|s.actual_scroll_points!=20.).count(),"external_input_events":self.external_input}))
    }
    fn typing_report(&self) -> Result<(), String> {
        let samples = self
            .latencies
            .iter()
            .filter(|s| s.source == "synthetic")
            .collect::<Vec<_>>();
        if samples.len() != 500 {
            return Err(format!(
                "expected 500 typing samples, got {}",
                samples.len()
            ));
        }
        for sample in &samples {
            self.reporter
                .emit(json!({"kind":"typing_sample","sample":sample}))?;
        }
        let submits = samples
            .iter()
            .map(|s| s.event_to_submit_ms)
            .collect::<Vec<_>>();
        let presents = samples
            .iter()
            .map(|s| s.event_to_present_return_ms)
            .collect::<Vec<_>>();
        let lateness = samples
            .iter()
            .filter_map(|s| s.timer_lateness_ms)
            .collect::<Vec<_>>();
        self.reporter.emit(json!({"kind":"typing_summary","insertions":self.injected,"unit":"ms","inject_to_submit":summarize(&submits),"inject_to_present_return":summarize(&presents),"timer_lateness":summarize(&lateness),"seed":self.options.seed,"interval_ms_inclusive":[30,90],"external_input_events":self.external_input,"injection_boundary":"new_events timer handler -> shared input() handler, same editing path as KeyboardInput; not an OS/HID event","presentation_boundary":"queue.present return; not display scanout"}))?;
        self.state.as_ref().unwrap().renderer.wait_idle()?;
        self.reporter.rss(
            "after_typing",
            self.state.as_ref().unwrap().renderer.atlas_info(),
        )
    }
    fn complete(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        if let Some(s) = &self.state {
            self.reporter.emit(json!({"kind":"final_render_state","fonts":s.fonts.metadata(),"atlas":s.renderer.atlas_info()}))?;
        }
        self.reporter.emit(json!({"kind":"complete","complete":true,"presented_frames":self.frame_id,"surface_retries":self.surface_retries,"external_input_events":self.external_input,"output":self.reporter.path}))?;
        self.stage = Stage::Done;
        event_loop.exit();
        Ok(())
    }
}
#[derive(Debug)]
enum Edit {
    Text(String),
    Backspace,
    Move(Motion, bool),
}
impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none()
            && let Err(e) = self.initialize(event_loop)
        {
            self.fail(event_loop, e);
        }
    }
    fn new_events(&mut self, _: &ActiveEventLoop, _: StartCause) {
        self.timer();
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let at = Instant::now();
        if self.window.as_ref().is_none_or(|w| w.id() != id) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                if self.options.mode.automatic() {
                    self.fail(
                        event_loop,
                        "automatic benchmark window was closed before completion".into(),
                    );
                } else {
                    event_loop.exit();
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.redraw(event_loop, at) {
                    self.fail(event_loop, e);
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(s) = self.state.as_mut() {
                    s.renderer.resize(size.width, size.height);
                    let logical = size.to_logical::<f32>(s.scale as f64);
                    s.view.width = logical.width.max(1.);
                    s.view.height = logical.height.max(1.);
                    s.view.clamp_scroll(s.editor.lines.len());
                }
                self.dirty();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(s) = self.state.as_mut() {
                    s.scale = scale_factor as f32;
                    s.renderer.clear_atlas();
                    let size = self.window.as_ref().unwrap().inner_size();
                    s.renderer.resize(size.width, size.height);
                    let logical = size.to_logical::<f32>(scale_factor);
                    s.view.width = logical.width.max(1.);
                    s.view.height = logical.height.max(1.);
                }
                self.dirty();
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.log_event(
                    "modifiers",
                    json!({"state":format!("{:?}",mods.state())}),
                    at,
                );
                self.modifiers = mods.state();
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } => {
                self.log_event("keyboard",json!({"physical_key":format!("{:?}",event.physical_key),"logical_key":format!("{:?}",event.logical_key),"text":event.text.as_deref(),"state":format!("{:?}",event.state),"repeat":event.repeat,"is_synthetic":is_synthetic}),at);
                if event.state != ElementState::Pressed || is_synthetic {
                    return;
                }
                if self.options.mode.automatic() {
                    self.external_input += 1;
                    return;
                }
                if self
                    .state
                    .as_ref()
                    .is_some_and(|s| s.editor.ime.composing())
                {
                    return;
                }
                let select = self.modifiers.shift_key();
                let action = match event.logical_key {
                    Key::Named(NamedKey::ArrowLeft) => Some(Edit::Move(Motion::Left, select)),
                    Key::Named(NamedKey::ArrowRight) => Some(Edit::Move(Motion::Right, select)),
                    Key::Named(NamedKey::ArrowUp) => Some(Edit::Move(Motion::Up, select)),
                    Key::Named(NamedKey::ArrowDown) => Some(Edit::Move(Motion::Down, select)),
                    Key::Named(NamedKey::Home) => Some(Edit::Move(Motion::Home, select)),
                    Key::Named(NamedKey::End) => Some(Edit::Move(Motion::End, select)),
                    Key::Named(NamedKey::Backspace) => Some(Edit::Backspace),
                    Key::Named(NamedKey::Enter) => Some(Edit::Text("\n".into())),
                    Key::Named(NamedKey::Tab) => Some(Edit::Text("\t".into())),
                    Key::Named(NamedKey::PageUp | NamedKey::PageDown) => {
                        if let Some(s) = self.state.as_mut() {
                            let sign = if event.logical_key == Key::Named(NamedKey::PageUp) {
                                -1.
                            } else {
                                1.
                            };
                            s.view.scroll(
                                0.,
                                sign * s.view.fully_visible_rows() as f32 * s.view.line_height,
                                s.editor.lines.len(),
                            );
                        }
                        self.dirty();
                        None
                    }
                    _ if !self.modifiers.super_key() && !self.modifiers.control_key() => event
                        .text
                        .as_deref()
                        .filter(|t| !t.chars().any(char::is_control))
                        .map(|t| Edit::Text(t.into())),
                    _ => None,
                };
                if let Some(action) = action {
                    self.input(at, "keyboard", action, None);
                }
            }
            WindowEvent::Ime(event) => self.ime(event, at),
            WindowEvent::MouseWheel { delta, .. } if !self.options.mode.automatic() => {
                if let Some(s) = self.state.as_mut() {
                    let (x, y) = match delta {
                        MouseScrollDelta::PixelDelta(p) => {
                            (-p.x as f32 / s.scale, -p.y as f32 / s.scale)
                        }
                        MouseScrollDelta::LineDelta(x, y) => {
                            (-x * s.view.cell, -y * s.view.line_height)
                        }
                    };
                    s.view.scroll(x, y, s.editor.lines.len());
                }
                self.dirty();
            }
            WindowEvent::CursorMoved { position, .. } if !self.options.mode.automatic() => {
                let scale = self.state.as_ref().unwrap().scale;
                self.mouse = [position.x as f32 / scale, position.y as f32 / scale];
                if self.mouse_down {
                    let s = self.state.as_mut().unwrap();
                    if !s.editor.ime.composing() {
                        let cursor = s.view.hit_test(&s.editor, self.mouse[0], self.mouse[1]);
                        s.editor.place_cursor(cursor, true);
                        self.dirty();
                    }
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } if !self.options.mode.automatic() => {
                self.mouse_down = state == ElementState::Pressed;
                if self.mouse_down {
                    let s = self.state.as_mut().unwrap();
                    if !s.editor.ime.composing() {
                        let cursor = s.view.hit_test(&s.editor, self.mouse[0], self.mouse[1]);
                        s.editor.place_cursor(cursor, self.modifiers.shift_key());
                        self.dirty();
                    }
                }
            }
            WindowEvent::Focused(focused) => {
                self.window.as_ref().unwrap().set_ime_allowed(focused);
                if !focused && let Some(s) = self.state.as_mut() {
                    s.editor.ime.clear();
                }
                self.dirty();
            }
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                self.retry_at = None;
                if !occluded {
                    self.dirty();
                }
            }
            _ => {}
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.stage == Stage::Done || self.error.is_some() {
            return;
        }
        let now = Instant::now();
        let retry_ready = self.retry_at.is_none_or(|deadline| now >= deadline);
        if retry_ready {
            self.retry_at = None;
        }
        if self.dirty
            && !self.occluded
            && !self.redraw_pending
            && retry_ready
            && let Some(window) = self.window.as_ref()
        {
            window.request_redraw();
            self.redraw_pending = true;
        }
        let deadline = self.next_injection.into_iter().chain(self.retry_at).min();
        event_loop.set_control_flow(deadline.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }
}
