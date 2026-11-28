//! Device resources shared by all renderers on the UI thread.
use super::{BACKEND, GpuAtlas, Instance, ms};
use crate::cache::LineLayoutCache;
use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

thread_local! {
    // The native window thread is unique. These weak slots neither keep the
    // last renderer alive nor share devices between independent test threads.
    static NATIVE: RefCell<Weak<GpuContext>> = const { RefCell::new(Weak::new()) };
    static HEADLESS: RefCell<Weak<GpuContext>> = const { RefCell::new(Weak::new()) };
}

#[derive(Debug)]
pub(super) struct GpuContext {
    pub(super) instance: wgpu::Instance,
    pub(super) device: wgpu::Device,
    pub(super) queue: wgpu::Queue,
    pub(super) atlas: RefCell<GpuAtlas>,
    pub(super) layouts: RefCell<LineLayoutCache>,
    pub(super) atlas_layout: wgpu::BindGroupLayout,
    pub(super) screen_layout: wgpu::BindGroupLayout,
    pub(super) sampler: wgpu::Sampler,
    adapter: wgpu::Adapter,
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    pipelines: RefCell<HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>>,
    cache: Option<wgpu::PipelineCache>,
    cache_path: Option<PathBuf>,
    pub(super) cache_loaded_bytes: usize,
    pub(super) has_queries: bool,
    lost: Arc<AtomicBool>,
}

#[derive(Default)]
pub(super) struct CreationTimes {
    pub(super) device_ms: f64,
    pub(super) shader_ms: f64,
    pub(super) pipeline_ms: f64,
}

impl GpuContext {
    pub(super) fn for_surface(
        create_surface: impl FnOnce(&wgpu::Instance) -> Result<wgpu::Surface<'static>, String>,
        configure: impl FnOnce(
            &wgpu::Surface<'static>,
            &wgpu::Adapter,
            &wgpu::Device,
        ) -> Result<wgpu::SurfaceConfiguration, String>,
        timestamps: bool,
        cache_dir: &Path,
        adapter_selected: impl FnOnce(&wgpu::AdapterInfo) -> Result<(), String>,
        phase: &mut impl FnMut(&'static str),
    ) -> Result<
        (
            Rc<Self>,
            wgpu::Surface<'static>,
            wgpu::SurfaceConfiguration,
            CreationTimes,
        ),
        String,
    > {
        let existing = NATIVE.with_borrow(Weak::upgrade);
        let instance = match &existing {
            Some(gpu) => {
                gpu.check_device()?;
                gpu.instance.clone()
            }
            None => {
                let instance = Self::make_instance();
                phase("wgpu_instance");
                instance
            }
        };
        let surface = create_surface(&instance)?;
        phase("surface_created");
        if let Some(gpu) = existing {
            adapter_selected(&gpu.adapter_info())?;
            let config = configure(&surface, &gpu.adapter, &gpu.device)?;
            phase("surface_configured");
            return Ok((gpu, surface, config, CreationTimes::default()));
        }
        let adapter = Self::request_adapter(&instance, Some(&surface))?;
        adapter_selected(&adapter.get_info())?;
        phase("adapter");
        let mut times = CreationTimes::default();
        let parts = DeviceParts::request(&adapter, timestamps, &mut times)?;
        phase("device_queue");
        let config = configure(&surface, &adapter, &parts.device)?;
        phase("surface_configured");
        let gpu = Rc::new(Self::new(
            instance,
            adapter,
            parts,
            Some(cache_dir),
            &mut times,
        ));
        phase("shader_module");
        NATIVE.with_borrow_mut(|slot| *slot = Rc::downgrade(&gpu));
        Ok((gpu, surface, config, times))
    }

    pub(super) fn offscreen() -> Result<(Rc<Self>, CreationTimes), String> {
        // A headless-only client must not dictate the native adapter or disk
        // cache path in a process that has yet to open its first window.
        if let Some(gpu) = NATIVE
            .with_borrow(Weak::upgrade)
            .or_else(|| HEADLESS.with_borrow(Weak::upgrade))
        {
            gpu.check_device()?;
            return Ok((gpu, CreationTimes::default()));
        }
        let instance = Self::make_instance();
        let adapter = Self::request_adapter(&instance, None)?;
        let mut times = CreationTimes::default();
        let parts = DeviceParts::request(&adapter, false, &mut times)?;
        let gpu = Rc::new(Self::new(instance, adapter, parts, None, &mut times));
        HEADLESS.with_borrow_mut(|slot| *slot = Rc::downgrade(&gpu));
        Ok((gpu, times))
    }

    fn make_instance() -> wgpu::Instance {
        wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: BACKEND,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        })
    }

    fn request_adapter(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'static>>,
    ) -> Result<wgpu::Adapter, String> {
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: surface,
            // DX12 ranks CPU adapters behind hardware, but leaves WARP eligible.
            ..Default::default()
        }))
        .map_err(|e| {
            if cfg!(windows) {
                format!("DX12 adapter (hardware or software): {e}")
            } else {
                e.to_string()
            }
        })
    }

    fn new(
        instance: wgpu::Instance,
        adapter: wgpu::Adapter,
        parts: DeviceParts,
        cache_dir: Option<&Path>,
        times: &mut CreationTimes,
    ) -> Self {
        let DeviceParts {
            device,
            queue,
            has_queries,
            has_cache,
        } = parts;
        let lost = Arc::new(AtomicBool::new(false));
        let callback_flag = Arc::clone(&lost);
        device.set_device_lost_callback(move |_, _| callback_flag.store(true, Ordering::Release));

        let cache_path = cache_dir.filter(|_| has_cache).map(|dir| {
            let info = adapter.get_info();
            let key = wgpu::util::pipeline_cache_key(&info)
                .unwrap_or_else(|| if cfg!(windows) { "dx12" } else { "metal" }.into());
            dir.join(format!("{key}.bin"))
        });
        let cache_bytes = cache_path.as_ref().and_then(|p| {
            std::fs::metadata(p)
                .ok()
                .filter(|m| m.is_file() && m.len() <= 64 * 1024 * 1024)
                .and_then(|_| std::fs::read(p).ok())
        });
        let cache = has_cache.then(|| {
            // SAFETY: only get_data output is written here. wgpu validates the
            // adapter/version, and fallback rejects stale data.
            unsafe {
                device.create_pipeline_cache(&wgpu::PipelineCacheDescriptor {
                    label: Some("Velocity persisted PSO"),
                    data: cache_bytes.as_deref(),
                    fallback: true,
                })
            }
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas linear clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let atlas_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glyph atlas"),
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
                // RGBA pages: color glyphs and images. Binding 0 holds R8 coverage.
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let screen_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let start = Instant::now();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Velocity instanced primitives"),
            source: wgpu::ShaderSource::Wgsl(include_str!("quads.wgsl").into()),
        });
        times.shader_ms = ms(start.elapsed());
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&atlas_layout), Some(&screen_layout)],
            immediate_size: 0,
        });
        Self {
            instance,
            device,
            queue,
            atlas: RefCell::new(GpuAtlas::new()),
            layouts: RefCell::new(LineLayoutCache::new(2048)),
            atlas_layout,
            screen_layout,
            sampler,
            adapter,
            shader,
            layout,
            pipelines: RefCell::default(),
            cache,
            cache_path,
            cache_loaded_bytes: cache_bytes.as_ref().map_or(0, Vec::len),
            has_queries,
            lost,
        }
    }

    pub(super) fn pipeline(
        &self,
        format: wgpu::TextureFormat,
        times: &mut CreationTimes,
    ) -> wgpu::RenderPipeline {
        self.pipelines
            .borrow_mut()
            .entry(format)
            .or_insert_with(|| {
                let attrs = wgpu::vertex_attr_array![0=>Float32x4,1=>Float32x4,2=>Float32x4,3=>Float32x4,4=>Float32x4,5=>Float32x4];
                let start = Instant::now();
                let pipeline = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("Velocity editor primitives"),
                    layout: Some(&self.layout),
                    vertex: wgpu::VertexState {
                        module: &self.shader,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: size_of::<Instance>() as u64,
                            step_mode: wgpu::VertexStepMode::Instance,
                            attributes: &attrs,
                        })],
                    },
                    primitive: Default::default(),
                    depth_stencil: None,
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &self.shader,
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: self.cache.as_ref(),
                });
                times.pipeline_ms = ms(start.elapsed());
                pipeline
            })
            .clone()
    }

    pub(super) fn adapter_info(&self) -> wgpu::AdapterInfo {
        self.adapter.get_info()
    }

    pub(super) fn has_cache(&self) -> bool {
        self.cache.is_some()
    }

    pub(super) fn trim_layouts(&self) {
        let mut layouts = self.layouts.borrow_mut();
        if !layouts.is_empty() {
            *layouts = LineLayoutCache::new(2048);
        }
    }

    pub(super) fn check_device(&self) -> Result<(), String> {
        if self.lost.load(Ordering::Acquire) {
            // Both native event loops already treat a renderer error as fatal.
            // Do not recover a single surface onto a different device.
            return Err(
                "shared rendering device lost; restart Velocity to redraw all windows".into(),
            );
        }
        Ok(())
    }

    pub(super) fn persist_pipeline_cache(&self) -> Result<(), String> {
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

struct DeviceParts {
    device: wgpu::Device,
    queue: wgpu::Queue,
    has_queries: bool,
    has_cache: bool,
}

impl DeviceParts {
    fn request(
        adapter: &wgpu::Adapter,
        timestamps: bool,
        times: &mut CreationTimes,
    ) -> Result<Self, String> {
        let available = adapter.features();
        let has_queries = timestamps && available.contains(wgpu::Features::TIMESTAMP_QUERY);
        let has_cache = available.contains(wgpu::Features::PIPELINE_CACHE);
        let mut features = wgpu::Features::empty();
        features.set(wgpu::Features::TIMESTAMP_QUERY, has_queries);
        features.set(wgpu::Features::PIPELINE_CACHE, has_cache);
        let start = Instant::now();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("Velocity renderer"),
            required_features: features,
            required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .map_err(|e| e.to_string())?;
        times.device_ms = ms(start.elapsed());
        Ok(Self {
            device,
            queue,
            has_queries,
            has_cache,
        })
    }
}
