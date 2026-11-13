//! Native application handler and buffered benchmark observations.
use crate::{
    bench::{Bench, SCROLL_FRAMES, TYPING_INSERTIONS},
    cli::{Mode, Options},
    editor::Editor,
    report::{Clock, Reporter, ms},
    stats::{summarize, sweep_positions},
    view::View,
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
use vel_platform::*;
use vel_render::{FrameTimes, Renderer, font::Fonts};
use vel_text::Text;

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
    editor: Editor,
    view: View,
    fonts: Option<Fonts>,
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
}
impl App {
    pub fn new(
        options: Options,
        clock: Clock,
        reporter: Reporter,
        text: Text,
        done: Arc<AtomicBool>,
    ) -> Self {
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
            editor: Editor::new(text),
            view,
            fonts: None,
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
        }
    }
    fn log_input(&self, kind: &str, event: impl std::fmt::Debug) {
        if self.options.log_input {
            let _=self.reporter.emit(json!({"kind":"input","event_kind":kind,"event":format!("{event:?}"),"selected_utf16":[self.editor.selected_utf16().location,self.editor.selected_utf16().length],"marked_utf16":self.editor.marked_utf16().map(|r|[r.location,r.length])}));
        }
    }
    fn changed(&mut self, w: &WindowHandle, received: Instant, reveal: bool) {
        self.input_at.get_or_insert(received);
        if reveal {
            self.view.reveal(&self.editor);
        }
        w.invalidate_character_coordinates();
        w.request_frame();
    }
    fn input(&mut self, w: &WindowHandle, input: TextInput, received: Instant) {
        self.log_input("text", &input);
        self.editor.text_input(input);
        self.changed(w, received, true);
    }
    fn start_report(&mut self, w: &WindowHandle, p: Presentation) -> Result<(), String> {
        self.clock.mark_at("first_frame_scanout", p.scanout);
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
        self.reporter.emit(json!({"kind":"startup","milestones":self.clock.milestones,"before_main_ms":self.clock.before_main_ms,"process_birth_error":self.clock.birth_error,"process_birth_method":"proc_pidinfo(PROC_PIDTBSDINFO) anchored at main wall snapshot; monotonic deltas thereafter","actual_physical_size":renderer.physical_size(),"logical_size":[w.content_size().width,w.content_size().height],"scale_factor":w.scale_factor(),"maximum_frames_per_second":w.maximum_frames_per_second(),"present_mode":"fifo","scanout_main_ms":ms(p.scanout.saturating_duration_since(self.clock.entered)),"scanout_process_ms":self.clock.before_main_ms.map(|n|n+ms(p.scanout.saturating_duration_since(self.clock.entered))),"presented_time_seconds":p.presented_time,"presented_callback_main_ms":ms(p.callback.saturating_duration_since(self.clock.entered)),"scanout_to_callback_ms":ms(p.callback.saturating_duration_since(p.scanout)),"drawable":p.drawable,"startup_path":"offscreen text frame submitted before orderFront; acquire/copy/present immediately in the same applicationDidFinishLaunching callback","surface_retries":self.retries,"boundary":"MTLDrawable.addPresentedHandler + presentedTime; queue.present return reported separately"}))?;
        self.reporter.emit(json!({"kind":"environment","renderer":renderer.info,"fonts":self.fonts.as_ref().map(Fonts::metadata),"atlas":renderer.atlas_info(),"log_input":self.options.log_input,"platform":"native AppKit objc2; macOS 14+","wgpu":"30.0.2"}))?;
        renderer.wait_idle()?;
        self.reporter.rss("after_first_frame",json!({"lines":self.editor.text.len_lines(),"bytes":self.editor.text.len_bytes(),"atlas":renderer.atlas_info()}))?;
        renderer.persist_pipeline_cache()?;
        self.startup_done = true;
        match self.options.mode {
            Mode::Startup => self.finish(w)?,
            Mode::Interactive => {}
            mode => {
                let mut bench = Bench::new(
                    mode,
                    self.options.seed,
                    sweep_positions(self.editor.text.len_lines(), self.view.rows()),
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
                self.reporter.emit(json!({"kind":"typing_summary","insertions":self.samples.len(),"unit":"ms","inject_to_submit":summarize(&submit),"inject_to_present_return":summarize(&present),"timer_lateness":summarize(&late),"seed":self.options.seed,"interval_ms_inclusive":[30,90],"injection_boundary":"one-shot NSTimer -> same TextInput handler as native insertText; no OS/HID injection","presentation_boundary":"queue.present return; not scanout","surface_retries":self.retries,"external_input_events":self.external_input}))?;
            }
            Mode::Memory => {
                self.renderer
                    .as_ref()
                    .ok_or("renderer missing")?
                    .wait_idle()?;
                self.reporter.rss("after_full_sweep",json!({"viewports":self.bench.as_ref().map(|b|b.completed),"atlas":self.renderer.as_ref().map(Renderer::atlas_info),"lines":self.editor.text.len_lines(),"bytes":self.editor.text.len_bytes()}))?;
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
        let fonts = Fonts::load()?;
        self.clock.mark("primary_font_ready");
        self.view = View::new(w.content_size(), fonts.cell as f64, fonts.ascent as f64);
        let cache_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("output/pipeline-cache");
        self.renderer = Some(Renderer::new(
            w.clone(),
            self.options.mode == Mode::Scroll,
            &cache_dir,
            |phase| self.clock.mark(phase),
        )?);
        self.fonts = Some(fonts);
        Ok(())
    }
    fn on_key(&mut self, _w: &WindowHandle, event: KeyEvent) {
        self.log_input("key", &event);
        if event.state == KeyState::Down {
            if self.options.mode.automatic() {
                self.external_input += 1;
            } else if self.input_at.is_some() {
                self.input_at = Some(event.received);
            }
        }
    }
    fn on_text_input(&mut self, w: &WindowHandle, event: TextInput) {
        if self.options.mode.automatic() {
            self.external_input += 1;
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
        let at = Instant::now();
        match command {
            Command::Copy => {
                w.set_clipboard_text(&self.editor.selected_text());
                return;
            }
            Command::Cut => {
                w.set_clipboard_text(&self.editor.selected_text());
                self.editor.text_input(TextInput::Insert {
                    text: String::new(),
                    replacement: None,
                });
            }
            Command::Paste => {
                if let Some(text) = w.clipboard_text() {
                    self.editor.text_input(TextInput::Insert {
                        text,
                        replacement: None,
                    });
                }
            }
            Command::PageUp | Command::PageDown => {
                for _ in 0..self.view.rows() {
                    self.editor.command(if command == Command::PageUp {
                        Command::Up(false)
                    } else {
                        Command::Down(false)
                    });
                }
            }
            _ => self.editor.command(command),
        }
        self.changed(w, at, true);
    }
    fn on_mouse(&mut self, w: &WindowHandle, event: MouseEvent) {
        if self.options.mode.automatic() {
            self.external_input += 1;
            return;
        }
        if event.button != 0 {
            return;
        }
        let at = self.view.hit_test(&self.editor, event.position);
        match event.kind {
            MouseKind::Down if event.click_count >= 3 => self.editor.select_line(at),
            MouseKind::Down if event.click_count == 2 => self.editor.select_word(at),
            MouseKind::Down => self.editor.set_caret(at, event.modifiers.shift()),
            MouseKind::Drag => self.editor.set_caret(at, true),
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
        self.view
            .scroll(&self.editor, -event.delta_x * scale, -event.delta_y * scale);
        self.changed(w, Instant::now(), false);
    }
    fn on_resize(&mut self, w: &WindowHandle, size: Size, scale: f64) {
        self.view.size = size;
        self.view.clamp_scroll(&self.editor);
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
        let observations = w.take_presentations();
        if !self.startup_done
            && let Some(p) = observations.iter().min_by_key(|p| p.scanout)
        {
            self.start_report(w, *p)?;
            return Ok(());
        }
        if ctx.reason == FrameReason::StartupPrepare {
            let scene = self.view.scene(&self.editor);
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
        if !self.first_present {
            // orderFront may constrain an oversized window or change its backing
            // scale. Rebuild immediately in this same callback if that invalidated
            // the prepared texture; never copy stale-sized pixels or await redraw.
            if !self
                .renderer
                .as_ref()
                .ok_or("renderer missing")?
                .has_prepared_frame()
            {
                let scene = self.view.scene(&self.editor);
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
            } else {
                self.retries += 1;
                w.request_frame_at(Instant::now() + Duration::from_millis(16));
            }
            return Ok(());
        }
        if !self.startup_done {
            w.request_frame_at(Instant::now() + Duration::from_millis(2));
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
                    let input = TextInput::Insert {
                        text: text.into(),
                        replacement: None,
                    };
                    self.input(w, input, self.bench_received.unwrap());
                }
            } else if !b.pending {
                b.pending = true;
                self.bench_received = Some(ctx.received);
                if b.mode == Mode::Memory {
                    self.view.scroll_y = b.positions[b.completed] as f64 * self.view.line_height;
                    self.view.clamp_scroll(&self.editor);
                } else {
                    let max = (self.editor.text.len_lines() as f64 * self.view.line_height
                        - self.view.size.height
                        + 8.)
                        .max(0.);
                    if self.view.scroll_y + 20. >= max {
                        self.view.scroll_y = 0.;
                    }
                    self.view.scroll(&self.editor, 0., 20.);
                }
            }
            if let Some(b) = self.bench.as_ref()
                && b.mode == Mode::Scroll
                && b.warmup == 0
            {
                query = Some(b.completed as u32);
            }
        }
        let scene = self.view.scene(&self.editor);
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
    fn on_quit_requested(&mut self) -> bool {
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
            text: self.editor.text.clone(),
            selected: self.editor.selected_utf16(),
            marked: self.editor.marked_utf16(),
            caret: self.view.rect_at(&self.editor, self.editor.caret),
        }
    }
    fn text_input_rect(&self, range: Utf16Range) -> (Rect, Utf16Range) {
        self.view.range_rect(&self.editor, range)
    }
    fn character_index_for_point(&self, point: Point) -> Option<usize> {
        Some(
            self.editor
                .text
                .byte_to_utf16(self.view.hit_test(&self.editor, point)),
        )
    }
}
