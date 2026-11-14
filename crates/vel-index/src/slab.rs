//! Stable allocation chunks avoid large Vec growth copies and retained allocator pages.
use std::ops::{Index, IndexMut};
const SHIFT: usize = 16;
const BLOCK: usize = 1 << SHIFT;
#[derive(Debug)]
pub(crate) struct Slab<T> {
    chunks: Vec<Vec<T>>,
    len: usize,
}
impl<T> Default for Slab<T> {
    fn default() -> Self {
        Self {
            chunks: Vec::new(),
            len: 0,
        }
    }
}
impl<T> Slab<T> {
    pub(crate) fn len(&self) -> usize {
        self.len
    }
    pub(crate) fn capacity(&self) -> usize {
        self.chunks.len() * BLOCK
    }
    pub(crate) fn push(&mut self, value: T) {
        if self.len.is_multiple_of(BLOCK) {
            self.chunks.push(Vec::with_capacity(BLOCK));
        }
        self.chunks.last_mut().unwrap().push(value);
        self.len += 1;
    }
    pub(crate) fn iter(&self) -> impl Iterator<Item = &T> {
        self.chunks.iter().flat_map(|chunk| chunk.iter())
    }
}
impl<T> Index<usize> for Slab<T> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        &self.chunks[index >> SHIFT][index & (BLOCK - 1)]
    }
}
impl<T> IndexMut<usize> for Slab<T> {
    fn index_mut(&mut self, index: usize) -> &mut T {
        &mut self.chunks[index >> SHIFT][index & (BLOCK - 1)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chunks_match_a_vector_through_boundaries_and_mutation() {
        let mut slab = Slab::default();
        let mut model = Vec::new();
        for n in 0..BLOCK * 3 + 7 {
            slab.push(n as u32);
            model.push(n as u32);
        }
        for n in [0, BLOCK - 1, BLOCK, BLOCK * 2, BLOCK * 3 + 6] {
            slab[n] += 7;
            model[n] += 7;
        }
        assert_eq!(slab.len(), model.len());
        assert!(slab.iter().copied().eq(model));
    }
}
