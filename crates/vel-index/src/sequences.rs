//! Compact n-gram counters with context-local posting chains.
use crate::names::NONE;
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
};
#[derive(Debug)]
struct Entry {
    words: [u32; 3],
    count: u32,
    next: u32,
}
#[derive(Debug, Default)]
pub(crate) struct Sequences {
    entries: crate::slab::Slab<Entry>,
    table: Vec<u32>,
    heads: HashMap<[u32; 2], u32>,
}
fn hash(key: &[u32; 3]) -> usize {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hash);
    hash.finish() as usize
}
fn context(words: [u32; 3]) -> [u32; 2] {
    if words[2] == NONE {
        [words[0], NONE]
    } else {
        [words[0], words[1]]
    }
}
impl Sequences {
    fn find(&self, key: [u32; 3]) -> Option<u32> {
        if self.table.is_empty() {
            return None;
        }
        let mut slot = hash(&key) & (self.table.len() - 1);
        loop {
            let id = self.table[slot];
            if id == NONE {
                return None;
            }
            if self.entries[id as usize].words == key {
                return Some(id);
            }
            slot = (slot + 1) & (self.table.len() - 1);
        }
    }
    pub(crate) fn add(&mut self, key: [u32; 3], count: u32) {
        if let Some(id) = self.find(key) {
            self.entries[id as usize].count = self.entries[id as usize].count.saturating_add(count);
            return;
        }
        if (self.entries.len() + 1) * 10 > self.table.len() * 7 {
            self.table = vec![NONE; (self.table.len() * 2).max(32)];
            for (id, entry) in self.entries.iter().enumerate() {
                let mut slot = hash(&entry.words) & (self.table.len() - 1);
                while self.table[slot] != NONE {
                    slot = (slot + 1) & (self.table.len() - 1);
                }
                self.table[slot] = id as u32;
            }
        }
        let id = self.entries.len() as u32;
        let prefix = context(key);
        let next = self.heads.insert(prefix, id).unwrap_or(NONE);
        self.entries.push(Entry {
            words: key,
            count,
            next,
        });
        let mut slot = hash(&key) & (self.table.len() - 1);
        while self.table[slot] != NONE {
            slot = (slot + 1) & (self.table.len() - 1);
        }
        self.table[slot] = id;
    }
    pub(crate) fn subtract(&mut self, key: [u32; 3], count: u32) {
        if let Some(id) = self.find(key) {
            self.entries[id as usize].count = self.entries[id as usize].count.saturating_sub(count);
        }
    }
    pub(crate) fn count(&self, key: [u32; 3]) -> u32 {
        self.find(key)
            .map_or(0, |id| self.entries[id as usize].count)
    }
    pub(crate) fn context_counts(&self, context: &[Option<u32>], count: usize) -> Vec<u32> {
        let mut counts = vec![0u32; count];
        for size in 1..=context.len() {
            let mut prefix = [NONE; 2];
            let mut known = true;
            for (i, value) in context[context.len() - size..].iter().enumerate() {
                if let Some(value) = value {
                    prefix[i] = *value;
                } else {
                    known = false;
                }
            }
            if !known {
                continue;
            }
            let mut id = self.heads.get(&prefix).copied().unwrap_or(NONE);
            while id != NONE {
                let entry = &self.entries[id as usize];
                let word = if entry.words[2] == NONE {
                    entry.words[1]
                } else {
                    entry.words[2]
                };
                counts[word as usize] = counts[word as usize].saturating_add(entry.count);
                id = entry.next;
            }
        }
        counts
    }
    pub(crate) fn storage_bytes(&self) -> usize {
        self.entries.capacity() * std::mem::size_of::<Entry>()
            + self.table.capacity() * 4
            + self.heads.capacity() * 16
    }
}
