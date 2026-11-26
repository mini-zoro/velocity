//! Shared wgpu renderer with native Metal and DX12 surfaces.
use crate::{
    atlas::*,
    cache::*,
    font::{FontStore, Fonts},
    scene::*,
    timing::{MAX_MEASURED_FRAMES, Queries},
};
use bytemuck::{Pod, Zeroable};
use serde_json::{Value, json};
use std::{
    path::Path,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use vel_platform::Rect;
#[cfg(target_os = "macos")]
use vel_platform::WindowHandle;
use wgpu::util::DeviceExt;

#[path = "gpu_context.rs"]
mod context;
#[path = "gpu_atlas.rs"]
mod shared_atlas;
use context::{CreationTimes, GpuContext};
use shared_atlas::GpuAtlas;

#[cfg(target_os = "macos")]
type NativeWindow = WindowHandle;
#[cfg(windows)]
type NativeWindow = std::num::NonZeroIsize;
#[cfg(target_os = "macos")]
const BACKEND: wgpu::Backends = wgpu::Backends::METAL;
#[cfg(windows)]
const BACKEND: wgpu::Backends = wgpu::Backends::DX12;

#[cfg(windows)]
unsafe fn win32_surface(
    instance: &wgpu::Instance,
    hwnd: NativeWindow,
) -> Result<wgpu::Surface<'static>, String> {
    use raw_window_handle::{
        RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowsDisplayHandle,
    };
    // SAFETY: the caller keeps hwnd valid on this thread through surface destruction.
    unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
            // The instance is created without a display handle, so the surface must name
            // one: with `None`, wgpu 30 refuses to create any surface ("No DisplayHandle").
            raw_display_handle: Some(RawDisplayHandle::Windows(WindowsDisplayHandle::new())),
            raw_window_handle: RawWindowHandle::Win32(Win32WindowHandle::new(hwnd)),
        })
    }
    .map_err(|error| error.to_string())
}

const ATLAS_SIDE: u32 = 1024;
const MAX_PAGES: usize = 8;
const MIN_INSTANCE_BYTES: u64 = 1024 * 1024;
const INSTANCE_SHRINK_FRAMES: u32 = 120;
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Instance {
    rect: [f32; 4],
    uv: [f32; 4],
    color: Color,
    clip: [f32; 4],
    params: [f32; 4],
    meta: [f32; 4],
}
/// CPU presentation boundaries for one successful frame.
#[derive(Clone, Debug)]
pub struct FrameTimes {
    /// Native drawable sequence, permitting exact frame-to-scanout correlation.
    pub drawable: Option<u64>,
    /// GPU commands submitted (not completed).
    pub submitted: Instant,
    /// queue.present returned (not scanout).
    pub presented: Instant,
    /// Drawable acquisition time in milliseconds.
    pub acquire_ms: f64,
    /// Native pool limit for this acquisition (two when resuming from idle).
    pub drawable_limit: usize,
    /// Encode + submit time, excluding acquisition.
    pub encode_submit_ms: f64,
    /// Present-call duration.
    pub present_call_ms: f64,
}
/// Native renderer with its own surface, viewport and scene buffers.
#[derive(Debug)]
pub struct Renderer {
    // Drop the surface before releasing its native window, and the per-window
    // resources before releasing the last strong reference to the device.
    surface: Option<wgpu::Surface<'static>>,
    window: Option<NativeWindow>,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    screen: wgpu::Buffer,
    screen_bind: wgpu::BindGroup,
    prepared_generation: Option<u64>,
    instances: wgpu::Buffer,
    capacity: u64,
    small_frames: u32,
    small_peak: u64,
    data: Vec<Instance>,
    batches: BatchPlan,
    queries: Option<Queries>,
    minimap_queries: Option<Queries>,
    minimap_batches: std::ops::Range<usize>,
    offscreen: Option<wgpu::Texture>,
    background: Color,
    gpu: Rc<GpuContext>,
    /// Adapter, feature support, pipeline timings and cache status.
    pub info: Value,
}
impl Renderer {
    /// Creates a Fifo surface directly from our CAMetalLayer. `phase` records
    /// initialization milestones without coupling this crate to a JSONL sink.
    #[cfg(target_os = "macos")]
    pub fn new(
        window: WindowHandle,
        timestamps: bool,
        cache_dir: &Path,
        mut phase: impl FnMut(&'static str),
    ) -> Result<Self, String> {
        let (gpu, surface, config, times) = GpuContext::for_surface(
            |instance| {
                // SAFETY: this is our live CAMetalLayer. Renderer retains the window
                // until after the surface drops; all use is on the main thread.
                unsafe {
                    instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(
                        window.metal_layer_ptr(),
                    ))
                }
                .map_err(|e| e.to_string())
            },
            |surface, adapter, device| {
                let capabilities = surface.get_capabilities(adapter);
                if !capabilities
                    .present_modes
                    .contains(&wgpu::PresentMode::Fifo)
                    || !capabilities.usages.contains(wgpu::TextureUsages::COPY_DST)
                {
                    return Err(
                        "Metal surface must support Fifo and COPY_DST for first-frame presentation"
                            .into(),
                    );
                }
                let size = window.content_size();
                let scale = window.scale_factor();
                let mut config = surface
                    .get_default_config(
                        adapter,
                        (size.width * scale).round().max(1.) as u32,
                        (size.height * scale).round().max(1.) as u32,
                    )
                    .ok_or("no surface configuration")?;
                config.present_mode = wgpu::PresentMode::Fifo;
                // Metal uses latency + 1 drawables. With only two, scanout plus
                // one queued present can stall a display-link frame in acquire.
                config.desired_maximum_frame_latency = 2;
                config.usage |= wgpu::TextureUsages::COPY_DST;
                if capabilities
                    .formats
                    .contains(&wgpu::TextureFormat::Bgra8Unorm)
                {
                    config.format = wgpu::TextureFormat::Bgra8Unorm;
                }
                surface.configure(device, &config);
                window.drawable_surface_configured();
                Ok(config)
            },
            timestamps,
            cache_dir,
            |_| Ok(()),
            &mut phase,
        )?;
        Ok(Self::assemble(
            gpu,
            Some((surface, window)),
            config,
            timestamps,
            times,
            phase,
        ))
    }
    /// Creates a DX12 renderer from an existing native window and physical pixel size.
    ///
    /// # Safety
    /// The HWND must identify a live window on this thread. Keep the window alive,
    /// and use and drop this renderer on the window thread before destroying it.
    #[cfg(windows)]
    pub unsafe fn from_hwnd(
        hwnd: *mut std::ffi::c_void,
        width: u32,
        height: u32,
        timestamps: bool,
        cache_dir: &Path,
        mut phase: impl FnMut(&'static str),
        adapter_selected: impl FnOnce(&wgpu::AdapterInfo) -> Result<(), String>,
    ) -> Result<Self, String> {
        let window = NativeWindow::new(hwnd as isize).ok_or("null window handle")?;
        let (gpu, surface, config, times) = GpuContext::for_surface(
            // SAFETY: from_hwnd requires the caller to keep the window live.
            |instance| unsafe { win32_surface(instance, window) },
            |surface, adapter, device| {
                let capabilities = surface.get_capabilities(adapter);
                let mut config = surface
                    .get_default_config(adapter, width.max(1), height.max(1))
                    .ok_or("no DX12 surface configuration")?;
                if !capabilities.usages.contains(wgpu::TextureUsages::COPY_DST)
                    || !capabilities
                        .formats
                        .contains(&wgpu::TextureFormat::Bgra8Unorm)
                {
                    return Err("DX12 surface must support BGRA8 and COPY_DST".into());
                }
                config.present_mode = wgpu::PresentMode::Fifo;
                config.usage |= wgpu::TextureUsages::COPY_DST;
                config.format = wgpu::TextureFormat::Bgra8Unorm;
                surface.configure(device, &config);
                Ok(config)
            },
            timestamps,
            cache_dir,
            adapter_selected,
            &mut phase,
        )?;
        Ok(Self::assemble(
            gpu,
            Some((surface, window)),
            config,
            timestamps,
            times,
            phase,
        ))
    }
    /// Creates a windowless renderer for developer tools. It never creates a surface,
    /// window or application object; frames are read back with [`Renderer::render_rgba`].
    pub fn offscreen(width: u32, height: u32) -> Result<Self, String> {
        let (gpu, times) = GpuContext::offscreen()?;
        // Same format as the presented surface, so pixels match a window.
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            format: wgpu::TextureFormat::Bgra8Unorm,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 1,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: Vec::new(),
        };
        Ok(Self::assemble(gpu, None, config, false, times, |_| {}))
    }
    fn assemble(
        gpu: Rc<GpuContext>,
        target: Option<(wgpu::Surface<'static>, NativeWindow)>,
        config: wgpu::SurfaceConfiguration,
        timestamps: bool,
        mut times: CreationTimes,
        mut phase: impl FnMut(&'static str),
    ) -> Self {
        let screen = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("physical viewport"),
                contents: bytemuck::cast_slice(&[
                    config.width as f32,
                    config.height as f32,
                    0.,
                    0.,
                ]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let screen_bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("window viewport"),
            layout: &gpu.screen_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: screen.as_entire_binding(),
            }],
        });
        let pipeline = gpu.pipeline(config.format, &mut times);
        phase("pipeline_ready");
        let capacity = MIN_INSTANCE_BYTES;
        let instances = make_instances(&gpu.device, capacity);
        let has_queries = timestamps && gpu.has_queries;
        let queries = has_queries.then(|| Queries::new(&gpu.device, &gpu.queue));
        let minimap_queries = has_queries.then(|| Queries::new(&gpu.device, &gpu.queue));
        let adapter = gpu.adapter_info();
        let has_cache = gpu.has_cache();
        let info = json!({
            "adapter":adapter.name,
            "backend":format!("{:?}",adapter.backend),
            "driver":adapter.driver,
            "present_mode":"fifo",
            "desired_maximum_frame_latency":config.desired_maximum_frame_latency,
            "timestamp_query_enabled":has_queries,
            "pipeline_cache_supported":has_cache,
            "pipeline_cache_status":if has_cache {"wgpu disk cache enabled"} else {"wgpu 30 Metal does not implement PIPELINE_CACHE; driver cache only"},
            "pipeline_cache_loaded_bytes":gpu.cache_loaded_bytes,
            "device_creation_ms":times.device_ms,
            "shader_module_ms":times.shader_ms,
            "pipeline_creation_ms":times.pipeline_ms,
            "surface_format":format!("{:?}",config.format)
        });
        let (surface, window) = target.unzip();
        Self {
            surface,
            window,
            config,
            pipeline,
            screen,
            screen_bind,
            prepared_generation: None,
            instances,
            capacity,
            small_frames: 0,
            small_peak: 0,
            data: Vec::new(),
            batches: BatchPlan::default(),
            queries,
            minimap_queries,
            minimap_batches: 0..0,
            offscreen: None,
            background: Scene::default().background,
            gpu,
            info,
        }
    }
    /// Resizes without stretching; the next frame uses fresh physical geometry.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0
            || height == 0
            || (width == self.config.width && height == self.config.height)
            || self.gpu.check_device().is_err()
        {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        if let Some(surface) = &self.surface {
            surface.configure(&self.gpu.device, &self.config);
            #[cfg(target_os = "macos")]
            if let Some(window) = &self.window {
                window.drawable_surface_configured();
            }
        }
        self.gpu.queue.write_buffer(
            &self.screen,
            0,
            bytemuck::cast_slice(&[width as f32, height as f32, 0., 0.]),
        );
        self.offscreen = None;
    }
    /// Resets shared atlas placements and drops this renderer's shaped layouts.
    pub fn reset_fonts(&mut self) {
        self.gpu.atlas.borrow_mut().reset(false);
        self.gpu.trim_layouts();
        self.invalidate_prepared();
    }
    /// A prepared first frame no longer represents this window's font settings.
    /// Layout entries are keyed by font identity and fallback generation.
    pub fn invalidate_prepared(&mut self) {
        self.offscreen = None;
    }
    /// Retains only enough instance storage for the latest scene. Under pressure,
    /// also drops the atlas texture; the next actual draw recreates and fills it.
    /// Returns the released shared atlas bytes and this window's prior instance bytes.
    pub fn trim_memory(&mut self, pressure: bool) -> (u64, u64) {
        let instance_bytes = self.instance_bytes();
        let required = std::mem::size_of_val(self.data.as_slice()) as u64;
        let capacity = required.next_power_of_two().max(MIN_INSTANCE_BYTES);
        if capacity < self.capacity && self.gpu.check_device().is_ok() {
            self.instances = make_instances(&self.gpu.device, capacity);
            self.capacity = capacity;
        }
        self.data
            .shrink_to((self.capacity as usize) / size_of::<Instance>());
        self.batches.shrink();
        self.gpu.trim_layouts();
        self.small_frames = 0;
        self.small_peak = 0;
        let atlas_bytes = if pressure {
            self.invalidate_prepared();
            let mut atlas = self.gpu.atlas.borrow_mut();
            let bytes = atlas.bytes();
            // The memory guard visits every window; only the first one drops
            // (and accounts for) the shared texture.
            if bytes > 0 {
                atlas.reset(true);
            }
            bytes
        } else {
            0
        };
        // In-flight commands keep their resources until completion. Reap what
        // has already finished, never wait for a drawable or for the device.
        let _ = self.gpu.device.poll(wgpu::PollType::Poll);
        (atlas_bytes, instance_bytes)
    }
    /// Shared atlas texture bytes and this renderer's GPU/CPU instance bytes.
    pub fn memory_cache_bytes(&self) -> (u64, u64) {
        (self.gpu.atlas.borrow().bytes(), self.instance_bytes())
    }
    fn instance_bytes(&self) -> u64 {
        self.capacity + (self.data.capacity() * size_of::<Instance>()) as u64
    }
    /// Physical framebuffer dimensions.
    pub fn physical_size(&self) -> [u32; 2] {
        [self.config.width, self.config.height]
    }
    /// Allocation diagnostics (texture storage excludes driver overhead).
    pub fn atlas_info(&self) -> Value {
        let atlas = self.gpu.atlas.borrow();
        json!({"side":ATLAS_SIDE,"pages":atlas.atlas.pages(),"allocated_array_layers":atlas.allocated_layers(),"allocated_texture_bytes":atlas.bytes(),"glyph_keys":atlas.atlas.len(),"page_evictions":atlas.atlas.evictions(),"generation":atlas.atlas.generation(),"layout_cache_entries":self.gpu.layouts.borrow().len(),"instance_buffer_bytes":self.capacity,"offscreen_bytes":if self.offscreen.is_some(){u64::from(self.config.width)*u64::from(self.config.height)*4}else{0},"draw_calls":self.batches.len()})
    }

    fn prepare(&mut self, scene: &Scene, fonts: &mut Fonts, scale: f32) -> Result<(), String> {
        self.gpu.check_device()?;
        fonts.poll_fallbacks();
        let mut fonts = fonts.for_frame();
        let gpu = Rc::clone(&self.gpu);
        let mut atlas = gpu.atlas.borrow_mut();
        atlas.grow_texture(&gpu, 1);
        self.prepared_generation = None;
        let warm = !atlas.atlas.is_empty();
        let mut result = self.build_instances(scene, &mut fonts, scale, &gpu, &mut atlas);
        if warm && matches!(&result, Err(error) if error == ALL_PAGES_PINNED) {
            // A hot atlas can pin fragments across every page even if this
            // frame fits by itself. Repack once on the existing allocation.
            self.gpu.check_device()?;
            atlas.reset(false);
            result = self.build_instances(scene, &mut fonts, scale, &gpu, &mut atlas);
        }
        if result.is_err() {
            self.discard_staged_writes();
        }
        result?;
        self.prepared_generation = Some(atlas.atlas.generation());
        #[cfg(target_os = "macos")]
        if fonts.fallback_pending()
            && let Some(window) = &self.window
        {
            window.request_frame_at(Instant::now() + Duration::from_millis(8));
        }
        Ok(())
    }

    fn build_instances(
        &mut self,
        scene: &Scene,
        fonts: &mut FontStore,
        scale: f32,
        gpu: &GpuContext,
        atlas: &mut GpuAtlas,
    ) -> Result<(), String> {
        atlas.atlas.begin_frame();
        let scale_bits = scale.to_bits();
        let font_set = fonts.layout_identity();
        let mut layouts = gpu.layouts.borrow_mut();
        self.data.clear();
        self.batches.clear();
        self.minimap_batches = 0..0;
        self.background = scene.background;
        for (layer_index, layer) in scene.layers.iter().enumerate() {
            let first = self.data.len() as u32;
            for r in &layer.shadows {
                self.data.push(quad(
                    r.rect,
                    r.clip,
                    r.color,
                    scale,
                    [r.radius * scale, r.blur * scale, 0., 0.],
                    PrimitiveKind::Shadow,
                ));
            }
            self.batch(layer_index, PrimitiveKind::Shadow, first);
            let first = self.data.len() as u32;
            for surface in &layer.surfaces {
                match surface {
                    Surface::CssBox(r) => {
                        let widths = r.borders.map(|b| b.width * scale);
                        if r.background[3] > 0. {
                            let mut q = quad(
                                r.rect,
                                r.clip,
                                r.background,
                                scale,
                                r.radii.map(|n| n * scale),
                                PrimitiveKind::Rect,
                            );
                            q.meta[0] = 4.;
                            q.uv = widths;
                            self.data.push(q);
                        }
                        for (i, border) in r.borders.iter().enumerate() {
                            if border.width <= 0. || border.color[3] <= 0. {
                                continue;
                            }
                            let mut q = quad(
                                r.rect,
                                r.clip,
                                border.color,
                                scale,
                                r.radii.map(|n| n * scale),
                                PrimitiveKind::Rect,
                            );
                            q.meta[0] = 4.;
                            q.meta[1] = (i + 1) as f32;
                            q.uv = widths;
                            self.data.push(q);
                        }
                    }
                    Surface::Fill(r) => self.data.push(quad(
                        r.rect,
                        r.clip,
                        r.color,
                        scale,
                        [r.radius * scale, 0., 0., 0.],
                        PrimitiveKind::Rect,
                    )),
                    Surface::Image(r) => {
                        let tile = (ATLAS_SIDE - 2) as f32;
                        if r.source[2] <= 0. || r.source[3] <= 0. {
                            continue;
                        }
                        let end_x = (r.source[0] + r.source[2]).min(r.image.width as f32);
                        let end_y = (r.source[1] + r.source[3]).min(r.image.height as f32);
                        for ty in (r.source[1] / tile).floor().max(0.) as u32
                            ..(end_y / tile).ceil() as u32
                        {
                            for tx in (r.source[0] / tile).floor().max(0.) as u32
                                ..(end_x / tile).ceil() as u32
                            {
                                let left = r.source[0].max(tx as f32 * tile);
                                let top = r.source[1].max(ty as f32 * tile);
                                let width = end_x.min((tx + 1) as f32 * tile) - left;
                                let height = end_y.min((ty + 1) as f32 * tile) - top;
                                let rect = Rect {
                                    x: r.rect.x
                                        + f64::from((left - r.source[0]) / r.source[2])
                                            * r.rect.width,
                                    y: r.rect.y
                                        + f64::from((top - r.source[1]) / r.source[3])
                                            * r.rect.height,
                                    width: f64::from(width / r.source[2]) * r.rect.width,
                                    height: f64::from(height / r.source[3]) * r.rect.height,
                                };
                                if rect.x + rect.width <= r.clip.x
                                    || rect.x >= r.clip.x + r.clip.width
                                    || rect.y + rect.height <= r.clip.y
                                    || rect.y >= r.clip.y + r.clip.height
                                {
                                    continue;
                                }
                                let entry = atlas.image_tile(
                                    gpu,
                                    &r.image,
                                    tx * (ATLAS_SIDE - 2),
                                    ty * (ATLAS_SIDE - 2),
                                )?;
                                let mut q = quad(
                                    rect,
                                    r.clip,
                                    r.color,
                                    scale,
                                    [0.; 4],
                                    PrimitiveKind::Glyph,
                                );
                                let side = ATLAS_SIDE as f32;
                                q.uv = [
                                    (entry.slot.x as f32 + left - tx as f32 * tile) / side,
                                    (entry.slot.y as f32 + top - ty as f32 * tile) / side,
                                    width / side,
                                    height / side,
                                ];
                                q.meta[1] = entry.page as f32;
                                self.data.push(q);
                            }
                        }
                    }
                }
            }
            self.batch(layer_index, PrimitiveKind::Surface, first);

            let first = self.data.len() as u32;
            for r in &layer.rects {
                let mut q = quad(
                    r.rect,
                    r.clip,
                    r.color,
                    scale,
                    [r.radius.max(0.) * scale, 0., 0., 0.],
                    PrimitiveKind::Rect,
                );
                if r.is_stippled_guide() {
                    q.meta[0] = 5.;
                    q.params = if r.radius == -3. {
                        [6. * scale, 3. * scale, 0., 0.]
                    } else {
                        [4. * scale, scale, 0., 0.]
                    };
                } else if r.is_outline() {
                    q.meta[0] = 6.;
                    q.params = [scale, 0., 0., 0.];
                }
                self.data.push(q);
            }
            for contour in &layer.contours {
                let translate = |p: vel_platform::Point| vel_platform::Point {
                    x: p.x + contour.offset.x,
                    y: p.y + contour.offset.y,
                };
                if contour.fill[3] > 0. {
                    for [a, b, c] in contour.triangles.iter() {
                        let a = translate(contour.points[*a]);
                        let b = translate(contour.points[*b]);
                        let c = translate(contour.points[*c]);
                        let mut q = quad(
                            Rect {
                                x: a.x,
                                y: a.y,
                                width: 1.,
                                height: 1.,
                            },
                            contour.clip,
                            contour.fill,
                            scale,
                            [
                                b.x as f32 * scale,
                                b.y as f32 * scale,
                                c.x as f32 * scale,
                                c.y as f32 * scale,
                            ],
                            PrimitiveKind::Rect,
                        );
                        q.meta[0] = 8.;
                        self.data.push(q);
                    }
                }
                if contour.border[3] > 0. {
                    for (a, b) in contour
                        .points
                        .iter()
                        .zip(contour.points.iter().cycle().skip(1))
                        .take(contour.points.len())
                    {
                        let (a, b) = (translate(*a), translate(*b));
                        let padding = contour.border_width / 2. + 1.;
                        let bounds = Rect {
                            x: a.x.min(b.x) - padding,
                            y: a.y.min(b.y) - padding,
                            width: (b.x - a.x).abs() + 2. * padding,
                            height: (b.y - a.y).abs() + 2. * padding,
                        };
                        let mut q = quad(
                            bounds,
                            contour.clip,
                            contour.border,
                            scale,
                            [
                                (a.x - bounds.x) as f32 * scale,
                                (a.y - bounds.y) as f32 * scale,
                                (b.x - bounds.x) as f32 * scale,
                                (b.y - bounds.y) as f32 * scale,
                            ],
                            PrimitiveKind::Rect,
                        );
                        q.meta[0] = 7.;
                        q.meta[2] = contour.border_width as f32 * scale * 0.5;
                        self.data.push(q);
                    }
                }
            }
            self.batch(layer_index, PrimitiveKind::Rect, first);
            let first = self.data.len() as u32;
            fonts.set_options(layer.font_options);
            for (index, run, glow) in layer
                .glow
                .iter()
                .enumerate()
                .map(|(i, r)| (i, r, true))
                .chain(layer.text.iter().enumerate().map(|(i, r)| (i, r, false)))
            {
                let advances = layer.wide_run_advances(index, glow);
                let px64 = (run.size * scale * 64.).round() as u32;
                let style = LayoutStyle {
                    font_style: run.style,
                    px64,
                    font_set,
                    font_epoch: layer.font_options.identity()
                        ^ advances.identity()
                        ^ run.family.as_ref().map_or(0, |family| {
                            use std::hash::{Hash, Hasher};
                            let mut h = std::collections::hash_map::DefaultHasher::new();
                            family.hash(&mut h);
                            h.finish()
                        }),
                };
                let glyphs = if let Some(g) = layouts.get(&run.text, style) {
                    g
                } else {
                    let g: Arc<[_]> = if let Some(family) = &run.family {
                        fonts
                            .shape_ui(&run.text, family, px64 as f32 / 64., run.style)
                            .glyphs
                            .into()
                    } else {
                        fonts
                            .shape_editor(&run.text, px64 as f32 / 64., run.style, advances)
                            .into()
                    };
                    layouts.insert(&run.text, style, Arc::clone(&g));
                    g
                };
                for g in glyphs.iter() {
                    let (x, bucket) = subpixel_x(run.x * scale + g.x);
                    let key = GlyphKey {
                        face: fonts.raster_identity(g.face),
                        id: g.id,
                        px64,
                        scale_bits,
                        bucket: bucket
                            | if layer.font_options.no_antialias {
                                4
                            } else {
                                0
                            },
                    };
                    if let Some((entry, raster)) = atlas.glyph(gpu, fonts, g.face, key)? {
                        let s = entry.slot;
                        let side = ATLAS_SIDE as f32;
                        let color = run
                            .highlight
                            .as_ref()
                            .and_then(|highlight| highlight.color_at(g.source))
                            .unwrap_or(run.color);
                        let mut q = quad(
                            Rect {
                                x: (x + raster.left as f32) as f64,
                                y: ((run.baseline * scale).round() - raster.top as f32 - g.y)
                                    as f64,
                                width: s.width as f64,
                                height: s.height as f64,
                            },
                            run.clip,
                            if raster.color {
                                [1., 1., 1., color[3]]
                            } else {
                                color
                            },
                            1.,
                            [0.; 4],
                            PrimitiveKind::Glyph,
                        );
                        q.clip = clip(run.clip, scale);
                        q.uv = [
                            s.x as f32 / side,
                            s.y as f32 / side,
                            s.width as f32 / side,
                            s.height as f32 / side,
                        ];
                        if glow {
                            let pad = 3. * scale;
                            q.params = [q.uv[0], q.uv[1], q.uv[0] + q.uv[2], q.uv[1] + q.uv[3]];
                            q.rect[0] -= pad;
                            q.rect[1] -= pad;
                            q.rect[2] += 2. * pad;
                            q.rect[3] += 2. * pad;
                            q.uv[0] -= pad / side;
                            q.uv[1] -= pad / side;
                            q.uv[2] += 2. * pad / side;
                            q.uv[3] += 2. * pad / side;
                            q.meta[0] = 9.;
                            q.meta[2] = scale / side;
                            q.color[3] *= 0.65;
                        }
                        q.meta[1] = entry.page as f32;
                        self.data.push(q);
                    }
                }
            }
            self.batch(layer_index, PrimitiveKind::Glyph, first);
            let first = self.data.len() as u32;
            for u in &layer.underlines {
                self.data.push(quad(
                    u.rect,
                    u.clip,
                    u.color,
                    scale,
                    [
                        0.,
                        0.,
                        u.thickness * scale,
                        if u.stippled {
                            2.
                        } else {
                            f32::from(u.squiggle)
                        },
                    ],
                    PrimitiveKind::Underline,
                ));
            }
            self.batch(layer_index, PrimitiveKind::Underline, first);
            if layer.minimap {
                let first = self
                    .batches
                    .as_slice()
                    .partition_point(|b| b.layer < layer_index);
                self.minimap_batches = first..self.batches.len();
            }
        }
        let required = std::mem::size_of_val(self.data.as_slice()) as u64;
        self.resize_instances(required);
        if required > 0 {
            self.gpu
                .queue
                .write_buffer(&self.instances, 0, bytemuck::cast_slice(&self.data));
        }
        Ok(())
    }

    fn resize_instances(&mut self, required: u64) {
        if required > self.capacity {
            self.capacity = required.next_power_of_two();
            self.instances = make_instances(&self.gpu.device, self.capacity);
        } else if self.capacity > MIN_INSTANCE_BYTES && required < self.capacity / 4 {
            self.small_peak = self.small_peak.max(required);
            self.small_frames += 1;
            if self.small_frames < INSTANCE_SHRINK_FRAMES {
                return;
            }
            // Two times the recent high-water mark leaves room for normal scrolling.
            self.capacity = (self.small_peak * 2)
                .next_power_of_two()
                .max(MIN_INSTANCE_BYTES);
            self.instances = make_instances(&self.gpu.device, self.capacity);
            self.data
                .shrink_to(self.capacity as usize / size_of::<Instance>());
            self.batches.shrink();
        } else if self.small_frames == 0 {
            return;
        }
        self.small_frames = 0;
        self.small_peak = 0;
    }
    fn batch(&mut self, layer: usize, kind: PrimitiveKind, first: u32) {
        let end = self.data.len() as u32;
        if end > first {
            self.batches
                .push(layer, kind, first..end)
                .expect("renderer builds contiguous batches in painter order");
        }
    }
    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        query: Option<u32>,
    ) -> Result<(), String> {
        let atlas = self.gpu.atlas.borrow();
        if self.prepared_generation != Some(atlas.atlas.generation()) {
            return Err("atlas placements changed since frame preparation".into());
        }
        let atlas_bind = &atlas.atlas_texture.as_ref().expect("prepared atlas").bind;
        let batches = self.batches.as_slice();
        let separate = query.is_some() && !self.minimap_batches.is_empty();
        let ranges = if separate {
            vec![
                0..self.minimap_batches.start,
                self.minimap_batches.clone(),
                self.minimap_batches.end..batches.len(),
            ]
        } else {
            std::iter::once(0..batches.len()).collect()
        };
        let c = self.background;
        for (part, range) in ranges.iter().enumerate() {
            let timestamp_writes = if separate && part == 1 {
                self.minimap_queries.as_ref().zip(query).map(|(q, i)| {
                    wgpu::RenderPassTimestampWrites {
                        query_set: &q.set,
                        beginning_of_pass_write_index: Some(i * 2),
                        end_of_pass_write_index: Some(i * 2 + 1),
                    }
                })
            } else {
                self.queries
                    .as_ref()
                    .zip(query)
                    .map(|(q, i)| wgpu::RenderPassTimestampWrites {
                        query_set: &q.set,
                        beginning_of_pass_write_index: (part == 0).then_some(i * 2),
                        end_of_pass_write_index: (part + 1 == ranges.len()).then_some(i * 2 + 1),
                    })
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(if separate && part == 1 {
                    "minimap only"
                } else {
                    "editor primitives"
                }),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if part == 0 {
                            wgpu::LoadOp::Clear(wgpu::Color {
                                r: c[0] as f64,
                                g: c[1] as f64,
                                b: c[2] as f64,
                                a: c[3] as f64,
                            })
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                timestamp_writes,
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, atlas_bind, &[]);
            pass.set_bind_group(1, &self.screen_bind, &[]);
            pass.set_vertex_buffer(0, self.instances.slice(..));
            for batch in &batches[range.clone()] {
                pass.draw(0..6, batch.instances.clone());
            }
        }
        Ok(())
    }
    /// Builds and submits the complete first text frame while the window is
    /// hidden. No drawable acquisition occurs on this path.
    pub fn pre_render(
        &mut self,
        scene: &Scene,
        fonts: &mut Fonts,
        scale: f32,
    ) -> Result<(), String> {
        self.prepare(scene, fonts, scale)?;
        let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("first frame before orderFront"),
            size: wgpu::Extent3d {
                width: self.config.width,
                height: self.config.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        self.encode(&mut encoder, &view, None)?;
        self.gpu.queue.submit([encoder.finish()]);
        self.gpu.check_device()?;
        self.offscreen = Some(texture);
        Ok(())
    }
    /// Whether a correctly sized first-frame texture is ready to present.
    pub fn has_prepared_frame(&self) -> bool {
        self.offscreen.is_some()
    }
    /// Releases the startup texture after a positive presentedTime observation.
    pub fn release_prepared(&mut self) {
        if let Some(texture) = self.offscreen.take() {
            texture.destroy();
        }
        // Reap completed submission references without waiting for the
        // device or for another frame.
        let _ = self.gpu.device.poll(wgpu::PollType::Poll);
    }
    /// Presents the prebuilt first frame, retaining it for skipped-drawable retries.
    pub fn present_prepared(&mut self) -> Result<Option<FrameTimes>, String> {
        self.draw(true, None)
    }
    /// Draws a normal scene and optionally records a GPU render-pass timestamp.
    pub fn render(
        &mut self,
        scene: &Scene,
        fonts: &mut Fonts,
        scale: f32,
        query: Option<u32>,
    ) -> Result<Option<FrameTimes>, String> {
        self.prepare(scene, fonts, scale)?;
        self.draw(false, query)
    }
    /// Submits nothing but this frame's staged buffer and texture writes. Without a
    /// drawable (a minimized or fully covered window) the frame is dropped after
    /// `prepare`, and wgpu keeps each write's staging buffer until some submission
    /// carries it: a hidden window polling every 200 ms grew by megabytes per second
    /// until it was shown again (2.5 GB peak, bug report 2026-11-25).
    fn discard_staged_writes(&self) {
        self.gpu
            .queue
            .submit(std::iter::empty::<wgpu::CommandBuffer>());
    }
    fn draw(&mut self, first: bool, query: Option<u32>) -> Result<Option<FrameTimes>, String> {
        self.gpu.check_device()?;
        if query.is_some_and(|i| i >= MAX_MEASURED_FRAMES) {
            return Err("GPU query index exceeds benchmark capacity".into());
        }
        let (Some(surface), Some(window)) = (&self.surface, &self.window) else {
            return Err("an offscreen renderer cannot present".into());
        };
        let start = Instant::now();
        #[cfg(target_os = "macos")]
        let drawable_limit = window.begin_drawable_frame(start);
        #[cfg(windows)]
        let drawable_limit = self.config.desired_maximum_frame_latency as usize;
        let texture = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                self.discard_staged_writes();
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.gpu.check_device()?;
                surface.configure(&self.gpu.device, &self.config);
                #[cfg(target_os = "macos")]
                window.drawable_surface_configured();
                self.discard_staged_writes();
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.gpu.check_device()?;
                // SAFETY: the retained window still owns this CAMetalLayer on this thread.
                #[cfg(target_os = "macos")]
                let surface = unsafe {
                    self.gpu.instance.create_surface_unsafe(
                        wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(window.metal_layer_ptr()),
                    )
                }
                .map_err(|e| e.to_string())?;
                #[cfg(windows)]
                let surface = {
                    let hwnd = *window;
                    // DXGI only permits one flip-model swap chain per HWND.
                    drop(self.surface.take());
                    // SAFETY: the caller of from_hwnd keeps hwnd live.
                    unsafe { win32_surface(&self.gpu.instance, hwnd) }?
                };
                surface.configure(&self.gpu.device, &self.config);
                #[cfg(target_os = "macos")]
                window.drawable_surface_configured();
                self.surface = Some(surface);
                self.discard_staged_writes();
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err("surface acquisition validation error".into());
            }
        };
        let acquired = Instant::now();
        #[cfg(target_os = "macos")]
        let drawable = window.last_acquired_drawable();
        #[cfg(windows)]
        let drawable = None;
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        if first {
            let offscreen = self
                .offscreen
                .as_ref()
                .ok_or("first frame was not prepared")?;
            encoder.copy_texture_to_texture(
                offscreen.as_image_copy(),
                texture.texture.as_image_copy(),
                wgpu::Extent3d {
                    width: self.config.width,
                    height: self.config.height,
                    depth_or_array_layers: 1,
                },
            );
        } else {
            self.encode(
                &mut encoder,
                &texture.texture.create_view(&Default::default()),
                query,
            )?;
        }
        if let Some((q, i)) = self.queries.as_mut().zip(query) {
            encoder.resolve_query_set(&q.set, i * 2..i * 2 + 2, &q.resolve, u64::from(i) * 256);
            encoder.copy_buffer_to_buffer(
                &q.resolve,
                u64::from(i) * 256,
                &q.readback,
                u64::from(i) * 16,
                16,
            );
            q.recorded = q.recorded.max(i + 1);
        }
        if !first
            && !self.minimap_batches.is_empty()
            && let Some((q, i)) = self.minimap_queries.as_mut().zip(query)
        {
            encoder.resolve_query_set(&q.set, i * 2..i * 2 + 2, &q.resolve, u64::from(i) * 256);
            encoder.copy_buffer_to_buffer(
                &q.resolve,
                u64::from(i) * 256,
                &q.readback,
                u64::from(i) * 16,
                16,
            );
            q.recorded = q.recorded.max(i + 1);
        }
        self.gpu.queue.submit([encoder.finish()]);
        self.gpu.check_device()?;
        let submitted = Instant::now();
        let present_start = Instant::now();
        self.gpu.queue.present(texture);
        let presented = Instant::now();
        Ok(Some(FrameTimes {
            drawable,
            submitted,
            presented,
            acquire_ms: ms(acquired - start),
            drawable_limit,
            encode_submit_ms: ms(submitted - acquired),
            present_call_ms: ms(presented - present_start),
        }))
    }
    /// Renders one scene into a fresh offscreen texture of the configured size and reads it
    /// back as tightly packed sRGB RGBA8 rows (top row first).
    pub fn render_rgba(
        &mut self,
        scene: &Scene,
        fonts: &mut Fonts,
        scale: f32,
    ) -> Result<Vec<u8>, String> {
        self.read_scene_rgba(scene, fonts, scale, self.physical_size())
    }
    /// Reads a small transparent scene with the window's existing device and glyph atlas,
    /// without resizing its surface. With a transparent clear, pixels are premultiplied RGBA.
    pub fn render_rgba_at_size(
        &mut self,
        scene: &Scene,
        fonts: &mut Fonts,
        scale: f32,
        size: [u32; 2],
    ) -> Result<Vec<u8>, String> {
        self.gpu.check_device()?;
        let limit = self.gpu.device.limits().max_texture_dimension_2d;
        if size.into_iter().any(|extent| extent == 0 || extent > limit) {
            return Err("Invalid offscreen image size".into());
        }
        let window_size = self.physical_size();
        let set_extent = |extent: [u32; 2]| [extent[0] as f32, extent[1] as f32, 0., 0.];
        self.gpu
            .queue
            .write_buffer(&self.screen, 0, bytemuck::cast_slice(&set_extent(size)));
        let result = self.read_scene_rgba(scene, fonts, scale, size);
        self.gpu.queue.write_buffer(
            &self.screen,
            0,
            bytemuck::cast_slice(&set_extent(window_size)),
        );
        result
    }
    fn read_scene_rgba(
        &mut self,
        scene: &Scene,
        fonts: &mut Fonts,
        scale: f32,
        [width, height]: [u32; 2],
    ) -> Result<Vec<u8>, String> {
        self.prepare(scene, fonts, scale)?;
        let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen readback"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("offscreen readback rows"),
            size: u64::from(row) * u64::from(height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        self.encode(
            &mut encoder,
            &texture.create_view(&Default::default()),
            None,
        )?;
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.gpu.queue.submit([encoder.finish()]);
        self.gpu.check_device()?;
        let (sender, receiver) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.wait_idle()?;
        receiver
            .recv_timeout(Duration::from_secs(15))
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let mapped = buffer
            .slice(..)
            .get_mapped_range()
            .map_err(|e| e.to_string())?;
        let bgra = self.config.format == wgpu::TextureFormat::Bgra8Unorm;
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height as usize {
            let start = y * row as usize;
            for px in mapped[start..start + width as usize * 4].as_chunks::<4>().0 {
                if bgra {
                    pixels.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                } else {
                    pixels.extend_from_slice(px);
                }
            }
        }
        drop(mapped);
        buffer.unmap();
        Ok(pixels)
    }
    /// Reads all recorded timestamp pairs once after the measured run.
    pub fn gpu_times(&self) -> Result<Option<Vec<f64>>, String> {
        self.gpu.check_device()?;
        self.queries
            .as_ref()
            .map(|q| q.read(&self.gpu.device))
            .transpose()
    }
    /// Isolated minimap render-pass timestamps, excluding the main editor and tabs.
    pub fn minimap_gpu_times(&self) -> Result<Option<Vec<f64>>, String> {
        self.gpu.check_device()?;
        self.minimap_queries
            .as_ref()
            .filter(|q| q.recorded > 0)
            .map(|q| q.read(&self.gpu.device))
            .transpose()
    }
    /// Waits for GPU completion outside any measured frame.
    pub fn wait_idle(&self) -> Result<(), String> {
        self.gpu.check_device()?;
        self.gpu
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(15)),
            })
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    /// Saves actual shared wgpu cache data if the adapter supports it.
    pub fn persist_pipeline_cache(&self) -> Result<(), String> {
        self.gpu.persist_pipeline_cache()
    }
}
fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.
}
fn make_instances(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("reused scene instances"),
        size,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn clip(r: Rect, s: f32) -> [f32; 4] {
    [
        r.x as f32 * s,
        r.y as f32 * s,
        (r.x + r.width) as f32 * s,
        (r.y + r.height) as f32 * s,
    ]
}
fn quad(r: Rect, c: Rect, color: Color, s: f32, params: [f32; 4], kind: PrimitiveKind) -> Instance {
    Instance {
        rect: [
            r.x as f32 * s,
            r.y as f32 * s,
            r.width as f32 * s,
            r.height as f32 * s,
        ],
        uv: [0.; 4],
        color,
        clip: clip(c, s),
        params,
        meta: [
            match kind {
                PrimitiveKind::Surface => 1.,
                PrimitiveKind::Shadow => 0.,
                PrimitiveKind::Rect => 1.,
                PrimitiveKind::Glyph => 2.,
                PrimitiveKind::Underline => 3.,
            },
            0.,
            0.,
            0.,
        ],
    }
}

#[cfg(all(test, target_os = "macos"))]
#[path = "gpu_memory_tests.rs"]
mod memory_tests;
