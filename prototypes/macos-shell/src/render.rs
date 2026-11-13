//! Metal-only wgpu renderer: one instanced glyph draw and one rectangle draw.
use crate::{
    atlas::{Shelf, Slot, subpixel_x},
    cli::Present,
    editor::cells,
    font::{Fonts, rgba},
    layout::Scene,
    report::{Clock, ms},
};
use bytemuck::{Pod, Zeroable};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use unicode_segmentation::UnicodeSegmentation;
use wgpu::util::DeviceExt;
use winit::window::Window;

pub const SCROLL_FRAMES: u32 = 600;
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Quad {
    rect: [f32; 4],
    uv: [f32; 4],
    color: [f32; 4],
    clip: [f32; 4],
}
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct GlyphKey {
    face: u16,
    id: u16,
    px64: u32,
    bucket: u8,
}
#[derive(Clone, Copy)]
struct Entry {
    slot: Slot,
    left: i32,
    top: i32,
    color: bool,
}
struct Atlas {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    packer: Shelf,
    entries: HashMap<GlyphKey, Option<Entry>>,
    white: Slot,
    side: u32,
}
impl Atlas {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, side: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let mut packer = Shelf::new(side);
        let white = packer.allocate(1, 1).expect("atlas fits a white texel");
        upload(queue, &texture, white, &[255; 4]);
        Self {
            texture,
            view,
            packer,
            entries: HashMap::new(),
            white,
            side,
        }
    }
    fn glyph(
        &mut self,
        fonts: &mut Fonts,
        queue: &wgpu::Queue,
        key: GlyphKey,
    ) -> Result<Option<Entry>, String> {
        if let Some(entry) = self.entries.get(&key) {
            return Ok(*entry);
        }
        let image = fonts.rasterize(key.face, key.id, key.px64 as f32 / 64., key.bucket);
        let entry = if let Some(image) =
            image.filter(|i| i.placement.width > 0 && i.placement.height > 0)
        {
            let slot = self
                .packer
                .allocate(image.placement.width, image.placement.height)
                .ok_or("atlas_full")?;
            upload(queue, &self.texture, slot, &rgba(&image));
            Some(Entry {
                slot,
                left: image.placement.left,
                top: image.placement.top,
                color: image.content == swash::scale::image::Content::Color,
            })
        } else {
            None
        };
        self.entries.insert(key, entry);
        Ok(entry)
    }
}
fn upload(queue: &wgpu::Queue, texture: &wgpu::Texture, slot: Slot, data: &[u8]) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: slot.x,
                y: slot.y,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(slot.width * 4),
            rows_per_image: Some(slot.height),
        },
        wgpu::Extent3d {
            width: slot.width,
            height: slot.height,
            depth_or_array_layers: 1,
        },
    );
}
struct Queries {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    period: f32,
    recorded: u32,
}
impl Queries {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self {
            set: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("600 render-pass timestamp pairs"),
                ty: wgpu::QueryType::Timestamp,
                count: SCROLL_FRAMES * 2,
            }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("aligned query resolve"),
                size: SCROLL_FRAMES as u64 * 256,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("deferred packed timestamp readback"),
                size: SCROLL_FRAMES as u64 * 16,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            period: queue.get_timestamp_period(),
            recorded: 0,
        }
    }
    fn read(&self, device: &wgpu::Device) -> Result<Vec<f64>, String> {
        let (tx, rx) = mpsc::channel();
        self.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(15)),
            })
            .map_err(|e| e.to_string())?;
        rx.recv_timeout(Duration::from_secs(1))
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let bytes = self
            .readback
            .slice(..)
            .get_mapped_range()
            .map_err(|e| e.to_string())?;
        let times = decode_timestamps(&bytes, self.recorded as usize, self.period)?;
        drop(bytes);
        self.readback.unmap();
        Ok(times)
    }
}
/// Convert native GPU ticks to milliseconds; zero/reversed timestamps are explicit errors.
fn decode_timestamps(bytes: &[u8], count: usize, period: f32) -> Result<Vec<f64>, String> {
    if !period.is_finite() || period <= 0. || bytes.len() < count * 16 {
        return Err("invalid timestamp buffer/period".into());
    }
    bytes
        .as_chunks::<16>()
        .0
        .iter()
        .take(count)
        .map(|b| {
            let a = u64::from_ne_bytes(b[..8].try_into().unwrap());
            let z = u64::from_ne_bytes(b[8..16].try_into().unwrap());
            if z < a || (a == 0 && z == 0) {
                Err(format!(
                    "unavailable/nonmonotonic GPU timestamp pair: {a}, {z}"
                ))
            } else {
                Ok((z - a) as f64 * period as f64 / 1_000_000.)
            }
        })
        .collect()
}

pub struct Prepared {
    quads: Vec<Quad>,
    pub rect_count: u32,
    pub glyph_count: u32,
}
pub struct FrameTimes {
    pub encoded: Instant,
    pub submitted: Instant,
    pub presented: Instant,
    pub acquire_ms: f64,
    pub encode_submit_ms: f64,
    pub present_call_ms: f64,
    pub suboptimal: bool,
}

pub struct Renderer {
    instance: wgpu::Instance,
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    bind: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    screen: wgpu::Buffer,
    atlas: Atlas,
    instances: wgpu::Buffer,
    instance_capacity: u64,
    queries: Option<Queries>,
    pub info: Value,
}
impl Renderer {
    pub fn new(
        window: Arc<Window>,
        present: Present,
        timestamps: bool,
        clock: &mut Clock,
    ) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::METAL,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        clock.mark("wgpu_instance");
        let surface = instance
            .create_surface(Arc::clone(&window))
            .map_err(|e| e.to_string())?;
        clock.mark("surface_created");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .map_err(|e| format!("Metal adapter: {e}"))?;
        clock.mark("adapter");
        let supported = adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        let required_features = if supported && timestamps {
            wgpu::Features::TIMESTAMP_QUERY
        } else {
            wgpu::Features::empty()
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("vel-shell Metal"),
            required_features,
            required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .map_err(|e| e.to_string())?;
        clock.mark("device_queue");
        let capabilities = surface.get_capabilities(&adapter);
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface has no default configuration")?;
        let mode = match present {
            Present::Fifo => wgpu::PresentMode::Fifo,
            Present::Mailbox => wgpu::PresentMode::Mailbox,
            Present::Immediate => wgpu::PresentMode::Immediate,
        };
        if !capabilities.present_modes.contains(&mode) {
            return Err(format!(
                "unsupported present mode {}; available: {:?}",
                present.name(),
                capabilities.present_modes
            ));
        }
        config.present_mode = mode;
        config.desired_maximum_frame_latency = 1;
        if capabilities
            .formats
            .contains(&wgpu::TextureFormat::Bgra8Unorm)
        {
            config.format = wgpu::TextureFormat::Bgra8Unorm;
        }
        surface.configure(&device, &config);
        clock.mark("surface_configured");
        let screen = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("screen physical size"),
            contents: bytemuck::cast_slice(&[config.width as f32, config.height as f32, 0., 0.]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let atlas = Atlas::new(&device, &queue, 2048);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas linear clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("atlas and screen"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
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
        let bind = make_bind(&device, &bind_layout, &atlas.view, &sampler, &screen);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("instanced glyphs and rectangles"),
            source: wgpu::ShaderSource::Wgsl(include_str!("quads.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let attrs = wgpu::vertex_attr_array![0=>Float32x4,1=>Float32x4,2=>Float32x4,3=>Float32x4];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quad pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Quad>() as u64,
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
            cache: None,
        });
        let instance_capacity = 1024 * 1024;
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad instances"),
            size: instance_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let queries = if supported && timestamps {
            Some(Queries::new(&device, &queue))
        } else {
            None
        };
        let info = adapter.get_info();
        let info = json!({"adapter":info.name,"backend":format!("{:?}",info.backend),"device_type":format!("{:?}",info.device_type),"driver":info.driver,"driver_info":info.driver_info,"timestamp_query_supported":supported,"timestamp_query_enabled":supported&&timestamps,"timestamp_period_ns":queries.as_ref().map(|q|q.period),"available_present_modes":capabilities.present_modes.iter().map(|m|format!("{m:?}")).collect::<Vec<_>>(),"present_mode":present.name(),"desired_maximum_frame_latency":1,"surface_format":format!("{:?}",config.format)});
        clock.mark("renderer_ready");
        Ok(Self {
            instance,
            surface,
            device,
            queue,
            config,
            pipeline,
            bind_layout,
            bind,
            sampler,
            screen,
            atlas,
            instances,
            instance_capacity,
            queries,
            info,
        })
    }
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.queue.write_buffer(
            &self.screen,
            0,
            bytemuck::cast_slice(&[width as f32, height as f32, 0., 0.]),
        );
    }
    pub fn recreate_surface(&mut self, window: Arc<Window>) -> Result<(), String> {
        self.surface = self
            .instance
            .create_surface(window)
            .map_err(|e| e.to_string())?;
        self.surface.configure(&self.device, &self.config);
        Ok(())
    }
    pub fn clear_atlas(&mut self) {
        self.atlas = Atlas::new(&self.device, &self.queue, self.atlas.side);
        self.bind = make_bind(
            &self.device,
            &self.bind_layout,
            &self.atlas.view,
            &self.sampler,
            &self.screen,
        );
    }
    pub fn atlas_info(&self) -> Value {
        json!({"side":self.atlas.side,"allocated_texture_bytes":self.atlas.side as u64*self.atlas.side as u64*4,"cached_glyph_keys":self.atlas.entries.len(),"instance_buffer_bytes":self.instance_capacity})
    }
    pub fn prepare(
        &mut self,
        scene: &Scene,
        fonts: &mut Fonts,
        scale: f32,
        gutter: f32,
    ) -> Result<Prepared, String> {
        loop {
            match self.prepare_once(scene, fonts, scale, gutter) {
                Ok(prepared) => return Ok(prepared),
                Err(e)
                    if e == "atlas_full"
                        && self.atlas.side
                            < 4096.min(self.device.limits().max_texture_dimension_2d) =>
                {
                    self.atlas = Atlas::new(&self.device, &self.queue, self.atlas.side * 2);
                    self.bind = make_bind(
                        &self.device,
                        &self.bind_layout,
                        &self.atlas.view,
                        &self.sampler,
                        &self.screen,
                    );
                }
                Err(e) => {
                    return Err(format!(
                        "{e}; glyph atlas limit reached, no stale UVs rendered"
                    ));
                }
            }
        }
    }
    fn prepare_once(
        &mut self,
        scene: &Scene,
        fonts: &mut Fonts,
        scale: f32,
        gutter: f32,
    ) -> Result<Prepared, String> {
        let mut quads = Vec::with_capacity(scene.text.len() + scene.rects.len());
        let clip_all = [0., 0., self.config.width as f32, self.config.height as f32];
        let clip_text = [gutter * scale, 0., clip_all[2], clip_all[3]];
        let side = self.atlas.side as f32;
        let white = self.atlas.white;
        let white_uv = [
            (white.x as f32 + 0.5) / side,
            (white.y as f32 + 0.5) / side,
            0.,
            0.,
        ];
        for (i, r) in scene.rects.iter().enumerate() {
            if r.rect.w > 0. && r.rect.h > 0. {
                quads.push(Quad {
                    rect: [
                        r.rect.x * scale,
                        r.rect.y * scale,
                        r.rect.w * scale,
                        r.rect.h * scale,
                    ],
                    uv: white_uv,
                    color: r.color,
                    clip: if i == 1 { clip_all } else { clip_text },
                });
            }
        }
        let rect_count = quads.len() as u32;
        let px64 = (13. * scale * 64.).round() as u32;
        for run in &scene.text {
            let mut col = 0;
            for text in run.text.graphemes(true) {
                let x = (run.x + col as f32 * fonts.cell) * scale;
                let baseline = (run.baseline * scale).round();
                for g in fonts.shape(text, px64 as f32 / 64.) {
                    let (x, bucket) = subpixel_x(x + g.x);
                    let key = GlyphKey {
                        face: g.face,
                        id: g.id,
                        px64,
                        bucket,
                    };
                    if let Some(entry) = self.atlas.glyph(fonts, &self.queue, key)? {
                        let a = entry.slot;
                        let color = if entry.color {
                            [1., 1., 1., 1.]
                        } else {
                            run.color
                        };
                        quads.push(Quad {
                            rect: [
                                x + entry.left as f32,
                                baseline - g.y - entry.top as f32,
                                a.width as f32,
                                a.height as f32,
                            ],
                            uv: [
                                a.x as f32 / side,
                                a.y as f32 / side,
                                a.width as f32 / side,
                                a.height as f32 / side,
                            ],
                            color,
                            clip: if run.gutter { clip_all } else { clip_text },
                        });
                    }
                }
                col += cells(text);
            }
        }
        let glyph_count = quads.len() as u32 - rect_count;
        let size = (quads.len() * std::mem::size_of::<Quad>()) as u64;
        if size > self.instance_capacity {
            self.instance_capacity = size.next_power_of_two();
            self.instances = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("grown quad instances"),
                size: self.instance_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        self.queue
            .write_buffer(&self.instances, 0, bytemuck::cast_slice(&quads));
        Ok(Prepared {
            quads,
            rect_count,
            glyph_count,
        })
    }
    /// Acquire happens before encoding. Retryable surface results do not consume a sample.
    pub fn draw(
        &mut self,
        prepared: &Prepared,
        query: Option<u32>,
        window: &Window,
    ) -> Result<Option<FrameTimes>, String> {
        let start = Instant::now();
        let mut suboptimal = false;
        let texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                suboptimal = true;
                t
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Lost => return Err("surface_lost".into()),
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err("surface acquisition validation error".into());
            }
        };
        let acquired = Instant::now();
        let view = texture.texture.create_view(&Default::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("vel-shell frame"),
            });
        let timestamp_writes =
            self.queries
                .as_ref()
                .zip(query)
                .map(|(q, i)| wgpu::RenderPassTimestampWrites {
                    query_set: &q.set,
                    beginning_of_pass_write_index: Some(i * 2),
                    end_of_pass_write_index: Some(i * 2 + 1),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear + rects + all glyphs"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.075,
                            g: 0.085,
                            b: 0.105,
                            a: 1.,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                timestamp_writes,
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_vertex_buffer(0, self.instances.slice(..));
            pass.draw(0..6, 0..prepared.rect_count);
            pass.draw(0..6, prepared.rect_count..prepared.quads.len() as u32);
        }
        if let Some((q, i)) = self.queries.as_mut().zip(query) {
            encoder.resolve_query_set(&q.set, i * 2..i * 2 + 2, &q.resolve, i as u64 * 256);
            encoder.copy_buffer_to_buffer(
                &q.resolve,
                i as u64 * 256,
                &q.readback,
                i as u64 * 16,
                16,
            );
            q.recorded = q.recorded.max(i + 1);
        }
        let commands = encoder.finish();
        let encoded = Instant::now();
        self.queue.submit([commands]);
        let submitted = Instant::now();
        window.pre_present_notify();
        let present_start = Instant::now();
        self.queue.present(texture);
        let presented = Instant::now();
        if suboptimal {
            self.surface.configure(&self.device, &self.config);
        }
        Ok(Some(FrameTimes {
            encoded,
            submitted,
            presented,
            acquire_ms: ms(acquired - start),
            encode_submit_ms: ms(submitted - acquired),
            present_call_ms: ms(presented - present_start),
            suboptimal,
        }))
    }
    pub fn gpu_times(&self) -> Result<Option<Vec<f64>>, String> {
        self.queries
            .as_ref()
            .map(|q| q.read(&self.device))
            .transpose()
    }
    pub fn wait_idle(&self) -> Result<(), String> {
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(15)),
            })
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}
fn make_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    screen: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("atlas bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gpu_ticks_are_converted_with_device_period() {
        let mut bytes = Vec::new();
        for n in [100u64, 2100, 5000, 9000] {
            bytes.extend_from_slice(&n.to_ne_bytes());
        }
        assert_eq!(
            decode_timestamps(&bytes, 2, 2.).unwrap(),
            vec![0.004, 0.008]
        );
        assert!(decode_timestamps(&bytes, 3, 2.).is_err());
        assert!(decode_timestamps(&[0; 16], 1, 1.).is_err());
        assert!(decode_timestamps(&bytes, 2, 0.).is_err());
    }
}
