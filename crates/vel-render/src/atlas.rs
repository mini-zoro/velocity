//! Device-free paged shelf packing, page LRU and frame pinning.
use std::collections::HashMap;

/// An inner rectangle, surrounded by one transparent pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    /// Left pixel.
    pub x: u32,
    /// Top pixel.
    pub y: u32,
    /// Inner width.
    pub width: u32,
    /// Inner height.
    pub height: u32,
}
/// Padded shelf allocator. Failed allocations do not change its state.
#[derive(Clone, Debug)]
pub struct Shelf {
    side: u32,
    x: u32,
    y: u32,
    row_height: u32,
}
impl Shelf {
    /// Creates a square page.
    pub fn new(side: u32) -> Self {
        Self {
            side,
            x: 0,
            y: 0,
            row_height: 0,
        }
    }
    /// Allocates an inner bitmap plus a one-pixel border.
    pub fn allocate(&mut self, width: u32, height: u32) -> Option<Slot> {
        let w = width.checked_add(2)?;
        let h = height.checked_add(2)?;
        if w > self.side || h > self.side {
            return None;
        }
        let wrap = self.x.checked_add(w).is_none_or(|end| end > self.side);
        let y = if wrap {
            self.y.checked_add(self.row_height)?
        } else {
            self.y
        };
        if y.checked_add(h)? > self.side {
            return None;
        }
        if wrap {
            self.x = 0;
            self.y = y;
            self.row_height = 0;
        }
        let slot = Slot {
            x: self.x + 1,
            y: self.y + 1,
            width,
            height,
        };
        self.x += w;
        self.row_height = self.row_height.max(h);
        Some(slot)
    }
}
/// Stable font/raster cache identity.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct GlyphKey {
    /// Font face index.
    pub face: u16,
    /// Face-local glyph id.
    pub id: u16,
    /// Physical raster size in 1/64 pixels.
    pub px64: u32,
    /// Quarter-pixel phase (0..4).
    pub bucket: u8,
}
/// Atlas page bitmap class. Color pages always contain RGBA pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageKind {
    /// Alpha mask expanded to white RGBA.
    Mask,
    /// Straight-alpha color glyph.
    Color,
}
/// A bitmap's position; page generations prevent stale UV reuse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtlasEntry {
    /// Texture array layer.
    pub page: u32,
    /// Page generation, incremented on eviction.
    pub generation: u64,
    /// Inner bitmap rectangle.
    pub slot: Slot,
}
#[derive(Debug)]
struct Page {
    packer: Shelf,
    kind: PageKind,
    generation: u64,
    used: u64,
    pinned: bool,
}
/// A bounded, growing atlas. Eviction is page-granular LRU; pages referenced by
/// the current frame cannot be evicted, so no built scene can contain stale UVs.
#[derive(Debug)]
pub struct PagedAtlas {
    side: u32,
    maximum: usize,
    frame: u64,
    pages: Vec<Page>,
    entries: HashMap<GlyphKey, AtlasEntry>,
    evictions: u64,
}
impl PagedAtlas {
    /// Creates an empty atlas (GPU pages are allocated only on first use).
    pub fn new(side: u32, maximum_pages: usize) -> Self {
        Self {
            side,
            maximum: maximum_pages.max(1),
            frame: 0,
            pages: Vec::new(),
            entries: HashMap::new(),
            evictions: 0,
        }
    }
    /// Starts frame pinning. Existing entries remain cached.
    pub fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
        for p in &mut self.pages {
            p.pinned = false;
        }
    }
    /// Looks up and pins a cached bitmap.
    pub fn get(&mut self, key: GlyphKey) -> Option<AtlasEntry> {
        let entry = *self.entries.get(&key)?;
        let page = &mut self.pages[entry.page as usize];
        page.pinned = true;
        page.used = self.frame;
        Some(entry)
    }
    /// Allocates a bitmap, growing pages or recycling the least-recently-used
    /// unpinned page. Returns an error if one frame exceeds the explicit budget.
    pub fn allocate(
        &mut self,
        key: GlyphKey,
        width: u32,
        height: u32,
        kind: PageKind,
    ) -> Result<AtlasEntry, String> {
        if width.checked_add(2).is_none_or(|w| w > self.side)
            || height.checked_add(2).is_none_or(|h| h > self.side)
        {
            return Err("glyph exceeds atlas page size".into());
        }
        if let Some(entry) = self.get(key) {
            return Ok(entry);
        }
        let mut allocated = None;
        for (i, page) in self.pages.iter_mut().enumerate() {
            if page.kind == kind
                && let Some(slot) = page.packer.allocate(width, height)
            {
                allocated = Some((i, slot));
                break;
            }
        }
        let (i, slot) = if let Some(pair) = allocated {
            pair
        } else {
            let i = if self.pages.len() < self.maximum {
                self.pages.push(Page {
                    packer: Shelf::new(self.side),
                    kind,
                    generation: 0,
                    used: 0,
                    pinned: false,
                });
                self.pages.len() - 1
            } else {
                let i = self
                    .pages
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| !p.pinned)
                    .min_by_key(|(_, p)| p.used)
                    .map(|(i, _)| i)
                    .ok_or("frame exceeds atlas capacity; all pages pinned")?;
                self.entries.retain(|_, e| e.page != i as u32);
                let generation = self.pages[i].generation + 1;
                self.pages[i] = Page {
                    packer: Shelf::new(self.side),
                    kind,
                    generation,
                    used: 0,
                    pinned: false,
                };
                self.evictions += 1;
                i
            };
            (
                i,
                self.pages[i]
                    .packer
                    .allocate(width, height)
                    .ok_or("invalid atlas dimensions")?,
            )
        };
        let p = &mut self.pages[i];
        p.used = self.frame;
        p.pinned = true;
        let entry = AtlasEntry {
            page: i as u32,
            generation: p.generation,
            slot,
        };
        self.entries.insert(key, entry);
        Ok(entry)
    }
    /// Number of currently allocated pages.
    pub fn pages(&self) -> usize {
        self.pages.len()
    }
    /// Number of live glyph keys.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    /// Whether no bitmaps are cached.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// Number of LRU page evictions.
    pub fn evictions(&self) -> u64 {
        self.evictions
    }
}
/// Floor origin and quarter-pixel phase, including negative scrolled positions.
pub fn subpixel_x(x: f32) -> (f32, u8) {
    let origin = x.floor();
    (origin, ((x - origin) * 4.).floor().clamp(0., 3.) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(id: u16) -> GlyphKey {
        GlyphKey {
            face: 0,
            id,
            px64: 832,
            bucket: 0,
        }
    }
    #[test]
    fn growth_lru_and_frame_pinning_prevent_stale_uvs() {
        let mut a = PagedAtlas::new(10, 2);
        a.begin_frame();
        let first = a.allocate(key(1), 8, 8, PageKind::Mask).unwrap();
        a.begin_frame();
        let second = a.allocate(key(2), 8, 8, PageKind::Color).unwrap();
        assert_ne!(first.page, second.page);
        assert_eq!(a.pages(), 2);
        a.begin_frame();
        assert!(a.get(key(1)).is_some());
        let third = a.allocate(key(3), 8, 8, PageKind::Color).unwrap();
        assert_eq!(third.page, second.page);
        assert_ne!(third.generation, second.generation);
        assert!(a.get(key(2)).is_none());
        assert!(a.get(key(1)).is_some());
        assert!(a.allocate(key(4), 8, 8, PageKind::Mask).is_err());
        assert_eq!(a.evictions(), 1);
    }
    #[test]
    fn shelf_padding_overflow_and_negative_subpixels() {
        let mut p = Shelf::new(32);
        let mut slots: Vec<Slot> = Vec::new();
        assert!(p.allocate(u32::MAX, 1).is_none());
        for i in 0..150 {
            if let Some(a) = p.allocate(i % 7 + 1, i % 5 + 1) {
                for b in &slots {
                    assert!(
                        a.x + a.width < b.x
                            || b.x + b.width < a.x
                            || a.y + a.height < b.y
                            || b.y + b.height < a.y
                    );
                }
                slots.push(a);
            }
        }
        assert!(slots.len() > 5);
        assert_eq!(subpixel_x(-0.1), (-1., 3));
    }
}
