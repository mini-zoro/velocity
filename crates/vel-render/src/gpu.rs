//! Shared wgpu renderer with native Metal and DX12 surfaces.
use crate::{
    atlas::*,
    cache::*,
    font::{Fonts, rgba},
    scene::*,
    timing::{MAX_MEASURED_FRAMES, Queries},
};
use bytemuck::{Pod, Zeroable};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use vel_platform::Rect;
#[cfg(target_os = "macos")]
use vel_platform::WindowHandle;
use wgpu::util::DeviceExt;

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
#[derive(Clone, Copy, Debug)]
struct Raster {
    left: i32,
    top: i32,
    color: bool,
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
/// Native renderer, reusable scene buffers, lazy array atlas and layout cache.
#[derive(Debug)]
pub struct Renderer {
    /// None for an offscreen renderer, which never presents.
    surface: Option<wgpu::Surface<'static>>,
    instance: wgpu::Instance,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    window: Option<NativeWindow>,
    pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    bind: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    screen: wgpu::Buffer,
    atlas: PagedAtlas,
    texture: wgpu::Texture,
    texture_pages: u32,
    rasters: HashMap<GlyphKey, Raster>,
    blanks: HashSet<GlyphKey>,
    instances: wgpu::Buffer,
    capacity: u64,
    data: Vec<Instance>,
    batches: BatchPlan,
    layouts: LineLayoutCache,
    queries: Option<Queries>,
    minimap_queries: Option<Queries>,
    minimap_batches: std::ops::Range<usize>,
    offscreen: Option<wgpu::Texture>,
    background: Color,
    cache: Option<wgpu::PipelineCache>,
    cache_path: Option<PathBuf>,
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
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::METAL,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        phase("wgpu_instance");
        // SAFETY: this is our live CAMetalLayer. `window` is retained in Renderer
        // until after surface drops; all use and destruction occur on the main thread.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(
                window.metal_layer_ptr(),
            ))
        }
        .map_err(|e| e.to_string())?;
        phase("surface_created");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .map_err(|e| e.to_string())?;
        phase("adapter");
        let device = DeviceParts::request(&adapter, timestamps)?;
        phase("device_queue");
        let capabilities = surface.get_capabilities(&adapter);
        if !capabilities
            .present_modes
            .contains(&wgpu::PresentMode::Fifo)
            || !capabilities.usages.contains(wgpu::TextureUsages::COPY_DST)
        {
            return Err(
                "Metal surface must support Fifo and COPY_DST for first-frame presentation".into(),
            );
        }
        let size = window.content_size();
        let scale = window.scale_factor();
        let mut config = surface
            .get_default_config(
                &adapter,
                (size.width * scale).round().max(1.) as u32,
                (size.height * scale).round().max(1.) as u32,
            )
            .ok_or("no surface configuration")?;
        config.present_mode = wgpu::PresentMode::Fifo;
        // wgpu's Metal backend uses latency + 1 drawables. With only two,
        // scanout plus one queued present can stall a display-link frame in acquire.
        config.desired_maximum_frame_latency = 2;
        config.usage |= wgpu::TextureUsages::COPY_DST;
        if capabilities
            .formats
            .contains(&wgpu::TextureFormat::Bgra8Unorm)
        {
            config.format = wgpu::TextureFormat::Bgra8Unorm;
        }
        surface.configure(&device.device, &config);
        window.drawable_surface_configured();
        phase("surface_configured");
        Self::assemble(
            instance,
            Some(surface),
            Some(window),
            &adapter,
            device,
            config,
            Some(cache_dir),
            phase,
        )
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
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: BACKEND,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        phase("wgpu_instance");
        // SAFETY: from_hwnd requires the caller to keep the window live.
        let surface = unsafe { win32_surface(&instance, window) }?;
        phase("surface_created");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            // wgpu ranks CPU adapters behind hardware, but leaves WARP eligible.
            ..Default::default()
        }))
        .map_err(|e| format!("DX12 adapter (hardware or software): {e}"))?;
        adapter_selected(&adapter.get_info())?;
        phase("adapter");
        let device = DeviceParts::request(&adapter, timestamps)?;
        phase("device_queue");
        let capabilities = surface.get_capabilities(&adapter);
        let mut config = surface
            .get_default_config(&adapter, width.max(1), height.max(1))
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
        surface.configure(&device.device, &config);
        phase("surface_configured");
        Self::assemble(
            instance,
            Some(surface),
            Some(window),
            &adapter,
            device,
            config,
            Some(cache_dir),
            phase,
        )
    }
    /// Creates a windowless renderer for developer tools. It never creates a surface,
    /// window or application object; frames are read back with [`Renderer::render_rgba`].
    pub fn offscreen(width: u32, height: u32) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: BACKEND,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            ..Default::default()
        }))
        .map_err(|e| e.to_string())?;
        let device = DeviceParts::request(&adapter, false)?;
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
        Self::assemble(instance, None, None, &adapter, device, config, None, |_| {})
    }
    #[allow(clippy::too_many_arguments)]
    fn assemble(
        instance: wgpu::Instance,
        surface: Option<wgpu::Surface<'static>>,
        window: Option<NativeWindow>,
        adapter: &wgpu::Adapter,
        parts: DeviceParts,
        config: wgpu::SurfaceConfiguration,
        cache_dir: Option<&Path>,
        mut phase: impl FnMut(&'static str),
    ) -> Result<Self, String> {
        let DeviceParts {
            device,
            queue,
            has_queries,
            has_cache,
            device_ms,
        } = parts;
        let info = adapter.get_info();
        let cache_path = cache_dir.filter(|_| has_cache).map(|cache_dir| {
            cache_dir.join(format!(
                "{}.bin",
                wgpu::util::pipeline_cache_key(&info).unwrap_or_else(|| if cfg!(windows) {
                    "dx12"
                } else {
                    "metal"
                }
                .into())
            ))
        });
        let cache_bytes = cache_path.as_ref().and_then(|p| {
            std::fs::metadata(p)
                .ok()
                .filter(|m| m.is_file() && m.len() <= 64 * 1024 * 1024)
                .and_then(|_| std::fs::read(p).ok())
        });
        let cache = if has_cache {
            // SAFETY: the cache directory is app-owned; only get_data output is
            // written here. wgpu validates adapter/version and fallback rejects stale data.
            Some(unsafe {
                device.create_pipeline_cache(&wgpu::PipelineCacheDescriptor {
                    label: Some("Velocity persisted PSO"),
                    data: cache_bytes.as_deref(),
                    fallback: true,
                })
            })
        } else {
            None
        };
        let screen = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("physical viewport"),
            contents: bytemuck::cast_slice(&[config.width as f32, config.height as f32, 0., 0.]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let texture = make_texture(&device, 1);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas linear clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("atlas array + viewport"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let bind = make_bind(&device, &bind_layout, &texture, &sampler, &screen);
        let start = Instant::now();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Velocity instanced primitives"),
            source: wgpu::ShaderSource::Wgsl(include_str!("quads.wgsl").into()),
        });
        let shader_ms = ms(start.elapsed());
        phase("shader_module");
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let attrs = wgpu::vertex_attr_array![0=>Float32x4,1=>Float32x4,2=>Float32x4,3=>Float32x4,4=>Float32x4,5=>Float32x4];
        let start = Instant::now();
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Velocity editor primitives"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attrs,
                })],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: cache.as_ref(),
        });
        let pipeline_ms = ms(start.elapsed());
        phase("pipeline_ready");
        let capacity = 1024 * 1024;
        let instances = make_instances(&device, capacity);
        let queries = has_queries.then(|| Queries::new(&device, &queue));
        let minimap_queries = has_queries.then(|| Queries::new(&device, &queue));
        let info = json!({"adapter":info.name,"backend":format!("{:?}",info.backend),"driver":info.driver,"present_mode":"fifo","desired_maximum_frame_latency":config.desired_maximum_frame_latency,"timestamp_query_enabled":has_queries,"pipeline_cache_supported":has_cache,"pipeline_cache_status":if has_cache {"wgpu disk cache enabled"} else {"wgpu 30 Metal does not implement PIPELINE_CACHE; driver cache only"},"pipeline_cache_loaded_bytes":cache_bytes.as_ref().map_or(0,Vec::len),"device_creation_ms":device_ms,"shader_module_ms":shader_ms,"pipeline_creation_ms":pipeline_ms,"surface_format":format!("{:?}",config.format)});
        Ok(Self {
            surface,
            instance,
            device,
            queue,
            config,
            window,
            pipeline,
            bind_layout,
            bind,
            sampler,
            screen,
            atlas: PagedAtlas::new(ATLAS_SIDE, MAX_PAGES),
            texture,
            texture_pages: 1,
            rasters: HashMap::new(),
            blanks: HashSet::new(),
            instances,
            capacity,
            data: Vec::new(),
            batches: BatchPlan::default(),
            layouts: LineLayoutCache::new(2048),
            queries,
            minimap_queries,
            minimap_batches: 0..0,
            offscreen: None,
            background: Scene::default().background,
            cache,
            cache_path,
            info,
        })
    }
    /// Resizes without stretching; the next frame uses fresh physical geometry.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 || (width == self.config.width && height == self.config.height)
        {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.config);
            #[cfg(target_os = "macos")]
            if let Some(window) = &self.window {
                window.drawable_surface_configured();
            }
        }
        self.queue.write_buffer(
            &self.screen,
            0,
            bytemuck::cast_slice(&[width as f32, height as f32, 0., 0.]),
        );
        self.offscreen = None;
    }
    /// Invalidates glyph/layout identities when the primary font family changes.
    pub fn reset_fonts(&mut self) {
        self.atlas = PagedAtlas::new(ATLAS_SIDE, MAX_PAGES);
        self.layouts = LineLayoutCache::new(2048);
        self.rasters.clear();
        self.blanks.clear();
        self.offscreen = None;
    }
    /// Drops pending-glyph layout results after asynchronous fallback completion.
    pub fn invalidate_layouts(&mut self) {
        self.layouts = LineLayoutCache::new(2048);
        self.offscreen = None;
    }
    /// Physical framebuffer dimensions.
    pub fn physical_size(&self) -> [u32; 2] {
        [self.config.width, self.config.height]
    }
    /// Allocation diagnostics (texture storage excludes driver overhead).
    pub fn atlas_info(&self) -> Value {
        json!({"side":ATLAS_SIDE,"pages":self.atlas.pages(),"allocated_array_layers":self.texture_pages,"allocated_texture_bytes":u64::from(ATLAS_SIDE).pow(2)*4*u64::from(self.texture_pages),"glyph_keys":self.atlas.len(),"page_evictions":self.atlas.evictions(),"layout_cache_entries":self.layouts.len(),"instance_buffer_bytes":self.capacity,"offscreen_bytes":if self.offscreen.is_some(){u64::from(self.config.width)*u64::from(self.config.height)*4}else{0},"draw_calls":self.batches.len()})
    }
    fn grow_texture(&mut self, pages: u32) {
        if pages <= self.texture_pages {
            return;
        }
        let texture = make_texture(&self.device, pages);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_texture(
            self.texture.as_image_copy(),
            texture.as_image_copy(),
            wgpu::Extent3d {
                width: ATLAS_SIDE,
                height: ATLAS_SIDE,
                depth_or_array_layers: self.texture_pages,
            },
        );
        self.queue.submit([encoder.finish()]);
        self.texture = texture;
        self.texture_pages = pages;
        self.bind = make_bind(
            &self.device,
            &self.bind_layout,
            &self.texture,
            &self.sampler,
            &self.screen,
        );
    }
    fn glyph(
        &mut self,
        fonts: &mut Fonts,
        key: GlyphKey,
    ) -> Result<Option<(AtlasEntry, Raster)>, String> {
        if self.blanks.contains(&key) {
            return Ok(None);
        }
        if let Some(entry) = self.atlas.get(key)
            && let Some(raster) = self.rasters.get(&key)
        {
            return Ok(Some((entry, *raster)));
        }
        let Some(image) = fonts.rasterize(key.face, key.id, key.px64 as f32 / 64., key.bucket)
        else {
            if self.blanks.len() >= 4096 {
                self.blanks.clear();
            }
            self.blanks.insert(key);
            return Ok(None);
        };
        let raster = Raster {
            left: image.placement.left,
            top: image.placement.top,
            color: image.content == swash::scale::image::Content::Color,
        };
        let entry = self.atlas.allocate(
            key,
            image.placement.width,
            image.placement.height,
            if raster.color {
                PageKind::Color
            } else {
                PageKind::Mask
            },
        )?;
        self.grow_texture(self.atlas.pages().max(1) as u32);
        // Upload a transparent border even when recycling a page with old pixels.
        let slot = entry.slot;
        let width = slot.width + 2;
        let height = slot.height + 2;
        let source = rgba(&image);
        let mut pixels = vec![0; (width * height * 4) as usize];
        for y in 0..slot.height as usize {
            let src = y * slot.width as usize * 4;
            let dst = ((y + 1) * width as usize + 1) * 4;
            pixels[dst..dst + slot.width as usize * 4]
                .copy_from_slice(&source[src..src + slot.width as usize * 4]);
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: slot.x - 1,
                    y: slot.y - 1,
                    z: entry.page,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        if self.rasters.len() > 32_768 {
            self.rasters.clear();
        }
        self.rasters.insert(key, raster);
        Ok(Some((entry, raster)))
    }
    fn image_tile(
        &mut self,
        image: &Image,
        tile_x: u32,
        tile_y: u32,
    ) -> Result<AtlasEntry, String> {
        if let Some(entry) = self.atlas.get_image_tile(image.id, tile_x, tile_y) {
            return Ok(entry);
        }
        if image.rgba.len() != image.width as usize * image.height as usize * 4 {
            return Err("invalid image buffer".into());
        }
        let tile_width = (image.width - tile_x).min(ATLAS_SIDE - 2);
        let tile_height = (image.height - tile_y).min(ATLAS_SIDE - 2);
        let entry =
            self.atlas
                .allocate_image_tile(image.id, tile_x, tile_y, tile_width, tile_height)?;
        self.grow_texture(self.atlas.pages().max(1) as u32);
        let width = tile_width + 2;
        let height = tile_height + 2;
        let mut pixels = vec![0; (width * height * 4) as usize];
        for y in 0..tile_height as usize {
            let src = ((y + tile_y as usize) * image.width as usize + tile_x as usize) * 4;
            let dst = ((y + 1) * width as usize + 1) * 4;
            pixels[dst..dst + tile_width as usize * 4]
                .copy_from_slice(&image.rgba[src..src + tile_width as usize * 4]);
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: entry.slot.x - 1,
                    y: entry.slot.y - 1,
                    z: entry.page,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        Ok(entry)
    }
    fn prepare(&mut self, scene: &Scene, fonts: &mut Fonts, scale: f32) -> Result<(), String> {
        if fonts.poll_fallbacks() {
            self.layouts = LineLayoutCache::new(2048);
        }
        self.atlas.begin_frame();
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
                                let entry = self.image_tile(
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
                    font_epoch: layer.font_options.identity()
                        ^ advances.identity()
                        ^ run.family.as_ref().map_or(0, |family| {
                            use std::hash::{Hash, Hasher};
                            let mut h = std::collections::hash_map::DefaultHasher::new();
                            family.hash(&mut h);
                            h.finish()
                        }),
                };
                let glyphs = if let Some(g) = self.layouts.get(&run.text, style) {
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
                    self.layouts.insert(&run.text, style, Arc::clone(&g));
                    g
                };
                for g in glyphs.iter() {
                    let (x, bucket) = subpixel_x(run.x * scale + g.x);
                    let key = GlyphKey {
                        face: g.face,
                        id: g.id,
                        px64,
                        bucket: bucket
                            | if layer.font_options.no_antialias {
                                4
                            } else {
                                0
                            },
                    };
                    if let Some((entry, raster)) = self.glyph(fonts, key)? {
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
        if required > self.capacity {
            self.capacity = required.next_power_of_two();
            self.instances = make_instances(&self.device, self.capacity);
        }
        if required > 0 {
            self.queue
                .write_buffer(&self.instances, 0, bytemuck::cast_slice(&self.data));
        }
        #[cfg(target_os = "macos")]
        if fonts.fallback_pending()
            && let Some(window) = &self.window
        {
            window.request_frame_at(Instant::now() + Duration::from_millis(8));
        }
        Ok(())
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
    ) {
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
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_vertex_buffer(0, self.instances.slice(..));
            for batch in &batches[range.clone()] {
                pass.draw(0..6, batch.instances.clone());
            }
        }
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
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
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
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.encode(&mut encoder, &view, None);
        self.queue.submit([encoder.finish()]);
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
        let _ = self.device.poll(wgpu::PollType::Poll);
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
    fn draw(&mut self, first: bool, query: Option<u32>) -> Result<Option<FrameTimes>, String> {
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
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                surface.configure(&self.device, &self.config);
                #[cfg(target_os = "macos")]
                window.drawable_surface_configured();
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                // SAFETY: the retained window still owns this CAMetalLayer on this thread.
                #[cfg(target_os = "macos")]
                let surface = unsafe {
                    self.instance.create_surface_unsafe(
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
                    unsafe { win32_surface(&self.instance, hwnd) }?
                };
                surface.configure(&self.device, &self.config);
                #[cfg(target_os = "macos")]
                window.drawable_surface_configured();
                self.surface = Some(surface);
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
        let mut encoder = self.device.create_command_encoder(&Default::default());
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
            );
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
        self.queue.submit([encoder.finish()]);
        let submitted = Instant::now();
        let present_start = Instant::now();
        self.queue.present(texture);
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
    /// back as tightly packed, straight sRGB RGBA8 rows (top row first).
    pub fn render_rgba(
        &mut self,
        scene: &Scene,
        fonts: &mut Fonts,
        scale: f32,
    ) -> Result<Vec<u8>, String> {
        self.prepare(scene, fonts, scale)?;
        let (width, height) = (self.config.width, self.config.height);
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
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
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("offscreen readback rows"),
            size: u64::from(row) * u64::from(height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.encode(
            &mut encoder,
            &texture.create_view(&Default::default()),
            None,
        );
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
        self.queue.submit([encoder.finish()]);
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
        self.queries
            .as_ref()
            .map(|q| q.read(&self.device))
            .transpose()
    }
    /// Isolated minimap render-pass timestamps, excluding the main editor and tabs.
    pub fn minimap_gpu_times(&self) -> Result<Option<Vec<f64>>, String> {
        self.minimap_queries
            .as_ref()
            .filter(|q| q.recorded > 0)
            .map(|q| q.read(&self.device))
            .transpose()
    }
    /// Waits for GPU completion outside any measured frame.
    pub fn wait_idle(&self) -> Result<(), String> {
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(15)),
            })
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    /// Atomically saves actual wgpu cache data if the adapter supports it.
    /// On wgpu 30 Metal this is a no-op and `info` reports the unsupported feature.
    pub fn persist_pipeline_cache(&self) -> Result<(), String> {
        if let Some((bytes, path)) = self
            .cache
            .as_ref()
            .and_then(|c| c.get_data())
            .zip(self.cache_path.as_ref())
        {
            let parent = path.parent().ok_or("invalid cache path")?;
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            let temp = path.with_extension(format!("{}.tmp", std::process::id()));
            std::fs::write(&temp, bytes)
                .and_then(|()| std::fs::rename(&temp, path))
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
/// Device, queue and the optional features negotiated for one adapter.
struct DeviceParts {
    device: wgpu::Device,
    queue: wgpu::Queue,
    has_queries: bool,
    has_cache: bool,
    device_ms: f64,
}
impl DeviceParts {
    fn request(adapter: &wgpu::Adapter, timestamps: bool) -> Result<Self, String> {
        let has_queries =
            timestamps && adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        let has_cache = adapter.features().contains(wgpu::Features::PIPELINE_CACHE);
        let mut features = wgpu::Features::empty();
        if has_queries {
            features |= wgpu::Features::TIMESTAMP_QUERY;
        }
        if has_cache {
            features |= wgpu::Features::PIPELINE_CACHE;
        }
        let start = Instant::now();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("Velocity Metal"),
            required_features: features,
            required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .map_err(|e| e.to_string())?;
        Ok(Self {
            device,
            queue,
            has_queries,
            has_cache,
            device_ms: ms(start.elapsed()),
        })
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
fn make_texture(device: &wgpu::Device, pages: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("growing RGBA glyph array"),
        size: wgpu::Extent3d {
            width: ATLAS_SIDE,
            height: ATLAS_SIDE,
            depth_or_array_layers: pages,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}
fn make_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    texture: &wgpu::Texture,
    sampler: &wgpu::Sampler,
    screen: &wgpu::Buffer,
) -> wgpu::BindGroup {
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("glyph pages and viewport"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: screen.as_entire_binding(),
            },
        ],
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
