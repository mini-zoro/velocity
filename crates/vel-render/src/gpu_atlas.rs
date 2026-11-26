//! One device-wide atlas. Queue submissions are serial on the window thread;
//! page reuse never races a render pass that was already submitted.
use super::{ATLAS_SIDE, AtlasEntry, GlyphKey, GpuContext, Image, MAX_PAGES, PageKind, PagedAtlas};
use crate::font::{FontStore, rgba};
use std::collections::{HashMap, HashSet};

// CPU rasters are placement metadata, not a second copy of atlas pixels.
pub(super) const RASTER_BYTES: usize = 1024 * 1024;
const RASTER_KEYS: usize = RASTER_BYTES / (4 * (size_of::<(GlyphKey, Raster)>() + 1));

#[derive(Clone, Copy, Debug)]
pub(super) struct Raster {
    pub(super) left: i32,
    pub(super) top: i32,
    pub(super) color: bool,
}

#[derive(Debug)]
pub(super) struct AtlasTexture {
    pub(super) bind: wgpu::BindGroup,
    texture: wgpu::Texture,
    pub(super) pages: u32,
}

#[derive(Debug)]
pub(super) struct GpuAtlas {
    pub(super) atlas: PagedAtlas,
    pub(super) atlas_texture: Option<AtlasTexture>,
    pub(super) rasters: HashMap<GlyphKey, Raster>,
    blanks: HashSet<GlyphKey>,
}
impl GpuAtlas {
    pub(super) fn new() -> Self {
        Self {
            atlas: PagedAtlas::new(ATLAS_SIDE, MAX_PAGES),
            atlas_texture: None,
            rasters: HashMap::new(),
            blanks: HashSet::new(),
        }
    }
    pub(super) fn reset(&mut self, release_texture: bool) {
        self.atlas.reset();
        self.rasters = HashMap::new();
        self.blanks = HashSet::new();
        if release_texture {
            self.atlas_texture = None;
        }
    }
    pub(super) fn allocated_layers(&self) -> u32 {
        self.atlas_texture.as_ref().map_or(0, |atlas| atlas.pages)
    }
    pub(super) fn bytes(&self) -> u64 {
        u64::from(ATLAS_SIDE).pow(2) * 4 * u64::from(self.allocated_layers())
    }
    pub(super) fn grow_texture(&mut self, gpu: &GpuContext, pages: u32) {
        if self
            .atlas_texture
            .as_ref()
            .is_some_and(|old| pages <= old.pages)
        {
            return;
        }
        let texture = make_texture(&gpu.device, pages);
        if let Some(old) = &self.atlas_texture {
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            encoder.copy_texture_to_texture(
                old.texture.as_image_copy(),
                texture.as_image_copy(),
                wgpu::Extent3d {
                    width: ATLAS_SIDE,
                    height: ATLAS_SIDE,
                    depth_or_array_layers: old.pages,
                },
            );
            gpu.queue.submit([encoder.finish()]);
        }
        let bind = make_bind(&gpu.device, &gpu.atlas_layout, &texture, &gpu.sampler);
        self.atlas_texture = Some(AtlasTexture {
            bind,
            texture,
            pages,
        });
    }
    pub(super) fn glyph(
        &mut self,
        gpu: &GpuContext,
        fonts: &mut FontStore,
        face: u16,
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
        let Some(image) = fonts.rasterize(face, key.id, key.px64 as f32 / 64., key.bucket) else {
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
        self.grow_texture(gpu, self.atlas.pages().max(1) as u32);
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
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self
                    .atlas_texture
                    .as_ref()
                    .expect("atlas allocated")
                    .texture,
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
        if self.rasters.len() >= RASTER_KEYS {
            self.rasters.clear();
        }
        self.rasters.insert(key, raster);
        Ok(Some((entry, raster)))
    }
    pub(super) fn image_tile(
        &mut self,
        gpu: &GpuContext,
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
        self.grow_texture(gpu, self.atlas.pages().max(1) as u32);
        let width = tile_width + 2;
        let height = tile_height + 2;
        let mut pixels = vec![0; (width * height * 4) as usize];
        for y in 0..tile_height as usize {
            let src = ((y + tile_y as usize) * image.width as usize + tile_x as usize) * 4;
            let dst = ((y + 1) * width as usize + 1) * 4;
            pixels[dst..dst + tile_width as usize * 4]
                .copy_from_slice(&image.rgba[src..src + tile_width as usize * 4]);
        }
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self
                    .atlas_texture
                    .as_ref()
                    .expect("atlas allocated")
                    .texture,
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
) -> wgpu::BindGroup {
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("shared glyph pages"),
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
        ],
    })
}
