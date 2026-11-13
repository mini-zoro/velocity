//! Minimal native Metal startup comparison, deliberately a clear-only lower bound.
use crate::{
    cli::{Options, Present},
    report::{Clock, Reporter},
};
use objc2::{
    rc::{Retained, autoreleasepool},
    runtime::ProtocolObject,
};
use objc2_app_kit::NSView;
use objc2_core_foundation::CGSize;
use objc2_metal::{
    MTLClearColor, MTLCommandBuffer, MTLCommandBufferStatus, MTLCommandEncoder, MTLCommandQueue,
    MTLCreateSystemDefaultDevice, MTLDevice, MTLLoadAction, MTLPixelFormat,
    MTLRenderPassDescriptor, MTLStoreAction,
};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};
use serde_json::json;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Window, WindowId},
};

pub fn run(options: Options, clock: Clock, reporter: Reporter) -> Result<(), String> {
    if options.present == Present::Mailbox {
        return Err("raw CAMetalLayer comparison supports fifo/immediate, not mailbox".into());
    }
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let mut clock = clock;
    clock.mark("event_loop_ready");
    let mut app = MetalApp {
        options,
        clock,
        reporter,
        window: None,
        layer: None,
        queue: None,
        error: None,
        done: false,
    };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    app.error.map_or_else(
        || {
            if app.done {
                Ok(())
            } else {
                Err("raw Metal exited without a frame".into())
            }
        },
        Err,
    )
}
struct MetalApp {
    options: Options,
    clock: Clock,
    reporter: Reporter,
    window: Option<Window>,
    layer: Option<Retained<CAMetalLayer>>,
    queue: Option<Retained<ProtocolObject<dyn MTLCommandQueue>>>,
    error: Option<String>,
    done: bool,
}
impl MetalApp {
    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let window = event_loop
            .create_window(
                Window::default_attributes()
                    .with_title("Velocity · raw Metal clear-only startup")
                    .with_inner_size(LogicalSize::new(self.options.width, self.options.height)),
            )
            .map_err(|e| e.to_string())?;
        self.clock.mark("window_created");
        let device =
            MTLCreateSystemDefaultDevice().ok_or("MTLCreateSystemDefaultDevice returned nil")?;
        self.clock.mark("metal_device");
        let queue = device
            .newCommandQueue()
            .ok_or("Metal command queue unavailable")?;
        self.clock.mark("metal_queue");
        let layer = CAMetalLayer::new();
        layer.setDevice(Some(&device));
        layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        layer.setFramebufferOnly(true);
        layer.setDisplaySyncEnabled(self.options.present == Present::Fifo);
        layer.setMaximumDrawableCount(2);
        layer.setContentsScale(window.scale_factor());
        let size = window.inner_size();
        layer.setDrawableSize(CGSize::new(size.width as f64, size.height as f64));
        let handle = window.window_handle().map_err(|e| e.to_string())?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return Err("raw Metal requires an AppKit NSView".into());
        };
        // SAFETY: winit owns this live NSView; we borrow it only on the main event-loop thread.
        let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
        view.setWantsLayer(true);
        view.setLayer(Some(&layer));
        layer.setFrame(view.bounds());
        self.clock.mark("metal_layer_configured");
        self.layer = Some(layer);
        self.queue = Some(queue);
        self.window = Some(window);
        Ok(())
    }
    fn frame(&mut self) -> Result<(), String> {
        let layer = self.layer.as_ref().unwrap();
        let drawable = layer
            .nextDrawable()
            .ok_or("CAMetalLayer nextDrawable returned nil; window must be visible")?;
        self.clock.mark("metal_drawable_acquired");
        let descriptor = MTLRenderPassDescriptor::new();
        // SAFETY: index zero is within Metal's array of eight color attachments.
        let attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
        attachment.setTexture(Some(&drawable.texture()));
        attachment.setLoadAction(MTLLoadAction::Clear);
        attachment.setStoreAction(MTLStoreAction::Store);
        attachment.setClearColor(MTLClearColor {
            red: 0.075,
            green: 0.085,
            blue: 0.105,
            alpha: 1.,
        });
        let commands = self
            .queue
            .as_ref()
            .unwrap()
            .commandBuffer()
            .ok_or("Metal command buffer unavailable")?;
        let encoder = commands
            .renderCommandEncoderWithDescriptor(&descriptor)
            .ok_or("Metal render encoder unavailable")?;
        encoder.endEncoding();
        self.clock.mark("first_frame_encoded");
        self.window.as_ref().unwrap().pre_present_notify();
        commands.presentDrawable(ProtocolObject::from_ref(&*drawable));
        self.clock.mark("metal_present_scheduled");
        commands.commit();
        self.clock.mark("first_present_returned");
        commands.waitUntilCompleted();
        if commands.status() == MTLCommandBufferStatus::Error {
            return Err(format!("Metal command failed: {:?}", commands.error()));
        }
        self.reporter.rss(
            "after_first_frame",
            json!({"renderer":"raw Metal","clear_only":true}),
        )?;
        let window = self.window.as_ref().unwrap();
        let size = window.inner_size();
        self.reporter.emit(json!({"kind":"metal_startup","milestones":self.clock.milestones,"before_main_ms":self.clock.before_main_ms,"process_birth_error":self.clock.birth_error,"actual_physical_size":[size.width,size.height],"scale_factor":window.scale_factor(),"present_mode":self.options.present.name(),"boundary":"presentDrawable scheduled + commandBuffer commit returned; no compositor/scanout timing","workload":"clear only, no text/font/atlas/pipeline; lower bound, not like-for-like text renderer","process_birth_method":"proc_pidinfo(PROC_PIDTBSDINFO), anchored to main wall snapshot with subsequent monotonic deltas"}))?;
        self.reporter
            .emit(json!({"kind":"complete","complete":true,"output":self.reporter.path}))?;
        self.done = true;
        Ok(())
    }
    fn fail(&mut self, event_loop: &ActiveEventLoop, e: String) {
        let _ = self
            .reporter
            .emit(json!({"kind":"error","stage":"raw_metal","message":e,"complete":false}));
        eprintln!("raw Metal: {e}");
        self.error = Some(e);
        event_loop.exit();
    }
}
impl ApplicationHandler for MetalApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            match autoreleasepool(|_| self.initialize(event_loop)) {
                Ok(()) => self.window.as_ref().unwrap().request_redraw(),
                Err(e) => self.fail(event_loop, e),
            }
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.window.as_ref().is_none_or(|w| w.id() != id) || self.done {
            return;
        }
        match event {
            WindowEvent::RedrawRequested => match autoreleasepool(|_| self.frame()) {
                Ok(()) => event_loop.exit(),
                Err(e) => self.fail(event_loop, e),
            },
            WindowEvent::Resized(size) => {
                if let Some(layer) = &self.layer {
                    layer.setDrawableSize(CGSize::new(size.width as f64, size.height as f64));
                }
            }
            WindowEvent::CloseRequested => {
                self.fail(event_loop, "raw Metal benchmark window closed early".into())
            }
            _ => {}
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::Wait);
    }
}
