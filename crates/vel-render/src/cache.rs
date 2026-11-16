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
    /// Weight/slant; distinct faces must never share layout entries.
    pub font_style: crate::scene::FontStyle,
    /// Physical font size in 1/64 pixels.
    pub px64: u32,
    /// Changes when the font set/configuration changes.
    pub font_epoch: u64,
}
#[derive(Debug)]
struct Cached {
    key: (u64, LayoutStyle),
    text: String,
    glyphs: Arc<[ShapedGlyph]>,
    older: Option<usize>,
    newer: Option<usize>,
}
/// LRU cache with both an entry cap and a per-line byte cap. Hash collisions are
/// checked against content, so a collision can only cause a miss. Recency uses
/// stable slot indices: lookup, insertion and eviction take expected O(1) cache
/// operations, in addition to hashing/checking the bounded line contents.
#[derive(Debug)]
pub struct LineLayoutCache {
    entries: HashMap<(u64, LayoutStyle), usize>,
    slots: Vec<Cached>,
    oldest: Option<usize>,
    newest: Option<usize>,
    maximum: usize,
}
impl LineLayoutCache {
    /// Creates a cache with a bounded number of visible/recent lines.
    pub fn new(maximum: usize) -> Self {
        Self {
            entries: HashMap::new(),
            slots: Vec::new(),
            oldest: None,
            newest: None,
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
        let slot = *self.entries.get(&Self::key(text, style))?;
        let e = &self.slots[slot];
        if e.text != text {
            return None;
        }
        let glyphs = Arc::clone(&e.glyphs);
        self.touch(slot);
        Some(glyphs)
    }
    /// Inserts a shaped line; very long lines stay uncached to bound memory.
    pub fn insert(&mut self, text: &str, style: LayoutStyle, glyphs: Arc<[ShapedGlyph]>) {
        if text.len() > 16_384 {
            return;
        }
        let key = Self::key(text, style);
        // Updating an existing key (including a hash collision) needs no eviction.
        let slot = if let Some(&slot) = self.entries.get(&key) {
            self.unlink(slot);
            slot
        } else if self.slots.len() == self.maximum {
            let slot = self.oldest.expect("a full cache has an oldest entry");
            self.entries.remove(&self.slots[slot].key);
            self.unlink(slot);
            slot
        } else {
            self.slots.push(Cached {
                key,
                text: String::new(),
                glyphs: Arc::from([]),
                older: None,
                newer: None,
            });
            self.slots.len() - 1
        };
        let entry = &mut self.slots[slot];
        entry.key = key;
        entry.text.clear();
        entry.text.push_str(text);
        entry.glyphs = glyphs;
        self.entries.insert(key, slot);
        self.append(slot);
    }
    fn unlink(&mut self, slot: usize) {
        let Cached { older, newer, .. } = self.slots[slot];
        if let Some(older) = older {
            self.slots[older].newer = newer;
        } else {
            self.oldest = newer;
        }
        if let Some(newer) = newer {
            self.slots[newer].older = older;
        } else {
            self.newest = older;
        }
    }
    fn append(&mut self, slot: usize) {
        self.slots[slot].older = self.newest;
        self.slots[slot].newer = None;
        if let Some(newest) = self.newest {
            self.slots[newest].newer = Some(slot);
        } else {
            self.oldest = Some(slot);
        }
        self.newest = Some(slot);
    }
    fn touch(&mut self, slot: usize) {
        if self.newest != Some(slot) {
            self.unlink(slot);
            self.append(slot);
        }
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
    use proptest::prelude::*;
    #[test]
    fn content_and_style_invalidate_and_lru_is_bounded() {
        let mut c = LineLayoutCache::new(2);
        let s = LayoutStyle {
            font_style: Default::default(),
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

    proptest! {
        #[test]
        fn indexed_lru_matches_a_simple_recency_list(
            capacity in 0usize..65,
            operations in prop::collection::vec((any::<bool>(), 0usize..100, 0u64..3), 0..2000),
        ) {
            let mut cache = LineLayoutCache::new(capacity);
            let mut model = Vec::new();
            for (insert, key, epoch) in operations {
                let text = key.to_string();
                let style = LayoutStyle {
                    font_style: Default::default(), px64: 832, font_epoch: epoch,
                };
                let item = (text.clone(), style);
                let found = model.iter().position(|e| e == &item);
                if insert {
                    cache.insert(&text, style, Arc::from([]));
                } else {
                    prop_assert_eq!(cache.get(&text, style).is_some(), found.is_some());
                }
                if insert || found.is_some() {
                    if let Some(index) = found {
                        model.remove(index);
                    }
                    model.push(item);
                    if model.len() > capacity.max(1) {
                        model.remove(0);
                    }
                }
                prop_assert_eq!(cache.len(), model.len());
                let mut ordered = Vec::new();
                let mut cursor = cache.oldest;
                while let Some(index) = cursor {
                    let entry = &cache.slots[index];
                    ordered.push((entry.text.clone(), entry.key.1));
                    cursor = entry.newer;
                    prop_assert!(ordered.len() <= cache.len());
                }
                prop_assert_eq!(&ordered, &model);
            }
        }
    }

    #[test]
    fn collision_replacement_and_oversized_lines_preserve_links() {
        let style = LayoutStyle {
            font_style: Default::default(),
            px64: 832,
            font_epoch: 0,
        };
        let mut cache = LineLayoutCache::new(1);
        cache.insert("a", style, Arc::from([]));
        // Force a collision in the index; content checking must still reject it.
        let old = LineLayoutCache::key("a", style);
        let collision = LineLayoutCache::key("b", style);
        cache.entries.remove(&old);
        cache.entries.insert(collision, 0);
        cache.slots[0].key = collision;
        assert!(cache.get("b", style).is_none());
        cache.insert("b", style, Arc::from([]));
        assert!(cache.get("b", style).is_some());
        cache.insert(&"x".repeat(16_385), style, Arc::from([]));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.oldest, cache.newest);
    }
}
