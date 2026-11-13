//! Bounded line/run layout cache, keyed by content hash and shaping style.
use crate::font::ShapedGlyph;
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::Arc,
};
/// Font size and font-set identity affecting layout.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct LayoutStyle {
    /// Physical font size in 1/64 pixels.
    pub px64: u32,
    /// Changes when the font set/configuration changes.
    pub font_epoch: u64,
}
#[derive(Debug)]
struct Cached {
    text: String,
    glyphs: Arc<[ShapedGlyph]>,
    used: u64,
}
/// LRU cache with both an entry cap and a per-line byte cap. Hash collisions are
/// checked against content, so a collision can only cause a miss.
#[derive(Debug)]
pub struct LineLayoutCache {
    entries: HashMap<(u64, LayoutStyle), Cached>,
    tick: u64,
    maximum: usize,
}
impl LineLayoutCache {
    /// Creates a cache with a bounded number of visible/recent lines.
    pub fn new(maximum: usize) -> Self {
        Self {
            entries: HashMap::new(),
            tick: 0,
            maximum: maximum.max(1),
        }
    }
    fn key(text: &str, style: LayoutStyle) -> (u64, LayoutStyle) {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        (h.finish(), style)
    }
    /// Looks up unchanged content and marks it as recently used.
    pub fn get(&mut self, text: &str, style: LayoutStyle) -> Option<Arc<[ShapedGlyph]>> {
        self.tick += 1;
        let e = self.entries.get_mut(&Self::key(text, style))?;
        if e.text != text {
            return None;
        }
        e.used = self.tick;
        Some(Arc::clone(&e.glyphs))
    }
    /// Inserts a shaped line; very long lines stay uncached to bound memory.
    pub fn insert(&mut self, text: &str, style: LayoutStyle, glyphs: Arc<[ShapedGlyph]>) {
        if text.len() > 16_384 {
            return;
        }
        if self.entries.len() >= self.maximum
            && let Some(key) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| *k)
        {
            self.entries.remove(&key);
        }
        self.tick += 1;
        self.entries.insert(
            Self::key(text, style),
            Cached {
                text: text.into(),
                glyphs,
                used: self.tick,
            },
        );
    }
    /// Number of cached lines/runs.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    /// Whether the cache has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn content_and_style_invalidate_and_lru_is_bounded() {
        let mut c = LineLayoutCache::new(2);
        let s = LayoutStyle {
            px64: 832,
            font_epoch: 0,
        };
        c.insert("a", s, Arc::from([]));
        c.insert("b", s, Arc::from([]));
        assert!(c.get("a", s).is_some());
        c.insert("c", s, Arc::from([]));
        assert!(c.get("b", s).is_none());
        assert!(c.get("a", LayoutStyle { px64: 1664, ..s }).is_none());
        assert_eq!(c.len(), 2);
    }
}
