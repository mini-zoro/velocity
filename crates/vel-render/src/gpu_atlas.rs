//! One device-wide atlas. Queue submissions are serial on the window thread;
//! page reuse never races a render pass that was already submitted.
//!
//! Coverage masks (almost every glyph) live in a single-channel R8 page array and
//! color bitmaps (emoji, theme and preview images) in an RGBA page array, so a mask
//! page costs 1 MiB instead of 4 MiB. Each array grows one page at a time and has its
//! own page LRU; a kind that has never been drawn holds a one-texel placeholder.
use super::{
    ATLAS_SIDE, AtlasEntry, COLOR_PAGES, GlyphKey, GpuContext, Image, MASK_PAGES, PageKind,
    PagedAtlas,
};
use crate::font::FontStore;
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
struct PageTexture {
    texture: wgpu::Texture,
    /// Allocated array layers; zero for the one-texel placeholder.
    pages: u32,
}

/// One page kind: placements, LRU and its GPU page array.
#[derive(Debug)]
struct Pages {
    kind: PageKind,
    atlas: PagedAtlas,
    texture: Option<PageTexture>,
}
impl Pages {
    fn new(kind: PageKind, maximum: usize) -> Self {
        Self {
            kind,
            atlas: PagedAtlas::new(ATLAS_SIDE, maximum),
            texture: None,
        }
    }
    fn format(&self) -> wgpu::TextureFormat {
        match self.kind {
            PageKind::Mask => wgpu::TextureFormat::R8Unorm,
            PageKind::Color => wgpu::TextureFormat::Rgba8Unorm,
        }
    }
    fn texel_bytes(&self) -> u32 {
        match self.kind {
            PageKind::Mask => 1,
            PageKind::Color => 4,
        }
    }
    fn layers(&self) -> u32 {
        self.texture.as_ref().map_or(0, |t| t.pages)
    }
    fn bytes(&self) -> u64 {
        u64::from(ATLAS_SIDE).pow(2) * u64::from(self.texel_bytes()) * u64::from(self.layers())
    }
    /// Grows the page array to `pages` layers (copying existing pages), or creates
    /// the placeholder. Returns true when the texture object changed.
    fn ensure(&mut self, gpu: &GpuContext, pages: u32) -> bool {
        match &self.texture {
            Some(old) if pages <= old.pages => return false,
            None if pages == 0 => {
                self.texture = Some(PageTexture {
                    texture: make_texture(&gpu.device, self.format(), 1, 1),
                    pages: 0,
                });
                return true;
            }
            _ => {}
        }
        let texture = make_texture(&gpu.device, self.format(), ATLAS_SIDE, pages);
        if let Some(old) = self.texture.as_ref().filter(|old| old.pages > 0) {
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
        self.texture = Some(PageTexture { texture, pages });
        true
    }
    /// Uploads `pixels` (tightly packed, this kind's texel size) around a slot,
    /// including its one-texel transparent border.
    fn upload(&self, gpu: &GpuContext, entry: AtlasEntry, width: u32, height: u32, pixels: &[u8]) {
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture.as_ref().expect("atlas allocated").texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: entry.slot.x - 1,
                    y: entry.slot.y - 1,
                    z: entry.page,
                },
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * self.texel_bytes()),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
    }
}

#[derive(Debug)]
pub(super) struct GpuAtlas {
    mask: Pages,
    color: Pages,
    bind: Option<wgpu::BindGroup>,
    pub(super) rasters: HashMap<GlyphKey, Raster>,
    blanks: HashSet<GlyphKey>,
}
impl GpuAtlas {
    pub(super) fn new() -> Self {
        Self {
            mask: Pages::new(PageKind::Mask, MASK_PAGES),
            color: Pages::new(PageKind::Color, COLOR_PAGES),
            bind: None,
            rasters: HashMap::new(),
            blanks: HashSet::new(),
        }
    }
    pub(super) fn reset(&mut self, release_texture: bool) {
        self.mask.atlas.reset();
        self.color.atlas.reset();
        self.rasters = HashMap::new();
        self.blanks = HashSet::new();
        if release_texture {
            self.mask.texture = None;
            self.color.texture = None;
            self.bind = None;
        }
    }
    /// Changes whenever any previously issued placement can no longer be used.
    pub(super) fn generation(&self) -> u64 {
        // Both counters only grow, so their sum changes whenever either does.
        self.mask.atlas.generation() + self.color.atlas.generation()
    }
    pub(super) fn begin_frame(&mut self) {
        self.mask.atlas.begin_frame();
        self.color.atlas.begin_frame();
    }
    pub(super) fn is_empty(&self) -> bool {
        self.mask.atlas.is_empty() && self.color.atlas.is_empty()
    }
    pub(super) fn len(&self) -> usize {
        self.mask.atlas.len() + self.color.atlas.len()
    }
    pub(super) fn evictions(&self) -> u64 {
        self.mask.atlas.evictions() + self.color.atlas.evictions()
    }
    /// Placement pages and allocated texture layers, as (mask, color).
    pub(super) fn pages(&self) -> ([usize; 2], [u32; 2]) {
        (
            [self.mask.atlas.pages(), self.color.atlas.pages()],
            [self.mask.layers(), self.color.layers()],
        )
    }
    pub(super) fn allocated_layers(&self) -> u32 {
        self.mask.layers() + self.color.layers()
    }
    pub(super) fn bytes(&self) -> u64 {
        self.mask.bytes() + self.color.bytes()
    }
    /// The bind group of the current page arrays; `prepare` creates it.
    pub(super) fn bind(&self) -> Option<&wgpu::BindGroup> {
        self.bind.as_ref()
    }
    /// Makes both page arrays bindable before a frame is built: an unused kind gets a
    /// placeholder, a kind with placements at least one page.
    pub(super) fn prepare_textures(&mut self, gpu: &GpuContext) {
        let mask = self.mask.atlas.pages() as u32;
        let color = self.color.atlas.pages() as u32;
        let changed = self.mask.ensure(gpu, mask) | self.color.ensure(gpu, color);
        if changed || self.bind.is_none() {
            self.rebind(gpu);
        }
    }
    fn rebind(&mut self, gpu: &GpuContext) {
        let view = |pages: &Pages| {
            pages
                .texture
                .as_ref()
                .expect("page array allocated")
                .texture
                .create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2Array),
                    ..Default::default()
                })
        };
        let (mask, color) = (view(&self.mask), view(&self.color));
        self.bind = Some(gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shared glyph pages"),
            layout: &gpu.atlas_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&mask),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&gpu.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&color),
                },
            ],
        }));
    }
    fn grow(&mut self, gpu: &GpuContext, kind: PageKind) {
        let pages = match kind {
            PageKind::Mask => &mut self.mask,
            PageKind::Color => &mut self.color,
        };
        let wanted = pages.atlas.pages().max(1) as u32;
        if pages.ensure(gpu, wanted) {
            self.rebind(gpu);
        }
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
        if let Some(raster) = self.rasters.get(&key).copied() {
            let pages = if raster.color {
                &mut self.color
            } else {
                &mut self.mask
            };
            if let Some(entry) = pages.atlas.get(key) {
                return Ok(Some((entry, raster)));
            }
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
        let kind = if raster.color {
            PageKind::Color
        } else {
            PageKind::Mask
        };
        let pages = match kind {
            PageKind::Mask => &mut self.mask,
            PageKind::Color => &mut self.color,
        };
        let entry =
            pages
                .atlas
                .allocate(key, image.placement.width, image.placement.height, kind)?;
        self.grow(gpu, kind);
        // Upload a transparent border even when recycling a page with old pixels.
        let slot = entry.slot;
        let width = slot.width + 2;
        let height = slot.height + 2;
        let pixels = bordered(&image, kind, slot.width, slot.height);
        let pages = match kind {
            PageKind::Mask => &self.mask,
            PageKind::Color => &self.color,
        };
        pages.upload(gpu, entry, width, height, &pixels);
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
        if let Some(entry) = self.color.atlas.get_image_tile(image.id, tile_x, tile_y) {
            return Ok(entry);
        }
        if image.rgba.len() != image.width as usize * image.height as usize * 4 {
            return Err("invalid image buffer".into());
        }
        let tile_width = (image.width - tile_x).min(ATLAS_SIDE - 2);
        let tile_height = (image.height - tile_y).min(ATLAS_SIDE - 2);
        let entry = self.color.atlas.allocate_image_tile(
            image.id,
            tile_x,
            tile_y,
            tile_width,
            tile_height,
        )?;
        self.grow(gpu, PageKind::Color);
        let width = tile_width + 2;
        let height = tile_height + 2;
        let mut pixels = vec![0; (width * height * 4) as usize];
        for y in 0..tile_height as usize {
            let src = ((y + tile_y as usize) * image.width as usize + tile_x as usize) * 4;
            let dst = ((y + 1) * width as usize + 1) * 4;
            pixels[dst..dst + tile_width as usize * 4]
                .copy_from_slice(&image.rgba[src..src + tile_width as usize * 4]);
        }
        self.color.upload(gpu, entry, width, height, &pixels);
        Ok(entry)
    }
}

/// The glyph bitmap inside a one-texel transparent border: one coverage byte per
/// texel for a mask page (a subpixel mask keeps its strongest channel, as the RGBA
/// pages did), straight-alpha RGBA for a color page.
fn bordered(
    image: &swash::scale::image::Image,
    kind: PageKind,
    width: u32,
    height: u32,
) -> Vec<u8> {
    use swash::scale::image::Content;
    let (w, h) = (width as usize, height as usize);
    let texel = match kind {
        PageKind::Mask => 1,
        PageKind::Color => 4,
    };
    let row = (w + 2) * texel;
    let mut pixels = vec![0; row * (h + 2)];
    for y in 0..h {
        let dst = &mut pixels[(y + 1) * row + texel..(y + 1) * row + texel + w * texel];
        match (kind, image.content) {
            (PageKind::Mask, Content::Mask) => dst.copy_from_slice(&image.data[y * w..(y + 1) * w]),
            (PageKind::Mask, Content::SubpixelMask) => {
                for (out, p) in dst
                    .iter_mut()
                    .zip(image.data[y * w * 4..(y + 1) * w * 4].as_chunks::<4>().0)
                {
                    *out = p[0].max(p[1]).max(p[2]);
                }
            }
            (PageKind::Color, Content::Color) => {
                dst.copy_from_slice(&image.data[y * w * 4..(y + 1) * w * 4]);
            }
            // A color page only receives color content and vice versa.
            _ => unreachable!("glyph content does not match its page kind"),
        }
    }
    pixels
}

fn make_texture(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    side: u32,
    pages: u32,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(match format {
            wgpu::TextureFormat::R8Unorm => "growing R8 glyph mask array",
            _ => "growing RGBA color glyph and image array",
        }),
        size: wgpu::Extent3d {
            width: side,
            height: side,
            depth_or_array_layers: pages.max(1),
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}
