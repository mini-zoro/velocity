//! Bounded asynchronous local/resource/data image decoding for minihtml.
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap},
    hash::{Hash, Hasher},
    io::{Cursor, Read},
    rc::{Rc, Weak},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};
use vel_minihtml::ImageSource;
use vel_render::scene::Image;

const CACHE_BYTES: usize = 32 * 1024 * 1024;
const CACHE_KEYS: usize = 256;
type ResultMessage = (Key, u64, u64, Option<Arc<Image>>);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Url(String),
    Data([u8; 32]),
}
impl Key {
    fn local(source: &ImageSource) -> Option<Self> {
        match source {
            ImageSource::File(url) | ImageSource::Resource(url) => Some(Self::Url(url.clone())),
            ImageSource::Data(uri) => Some(Self::Data(uri.digest())),
            ImageSource::Https(_) => None,
        }
    }
    fn bytes(&self) -> usize {
        256 + match self {
            Self::Url(url) => url.capacity(),
            Self::Data(_) => 0,
        }
    }
}

/// A document owns the slot, not its pixels. Eviction clears the pixels in all
/// documents; closing the last document frees them even before the next poll.
#[derive(Debug)]
pub(crate) struct Slot {
    serial: u64,
    cancelled: Arc<AtomicBool>,
    clock: Rc<Cell<u64>>,
    used: Cell<u64>,
    size: Cell<Option<[f32; 2]>>,
    expected_pixels: Cell<usize>,
    loading: Cell<bool>,
    image: RefCell<Option<Arc<Image>>>,
}
impl Slot {
    fn touch(&self) {
        let now = self.clock.get() + 1;
        self.clock.set(now);
        self.used.set(now);
    }
    pub(crate) fn pixels(&self) -> Option<Arc<Image>> {
        let image = self.image.borrow();
        if image.is_some() {
            self.touch();
        }
        image.clone()
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

#[derive(Debug)]
struct Entry {
    slot: Weak<Slot>,
    charge: usize,
    pixels: usize,
}
#[derive(Debug, Default)]
struct Cache {
    entries: HashMap<Key, Entry>,
    bytes: usize,
}
impl Cache {
    fn prune(&mut self) -> bool {
        let before = self.entries.len();
        self.entries.retain(|_, entry| {
            if entry.slot.strong_count() != 0 {
                true
            } else {
                self.bytes -= entry.charge + entry.pixels;
                false
            }
        });
        self.entries.len() != before
    }
    fn make_room(&mut self, needed: usize) -> bool {
        while self.bytes.saturating_add(needed) > CACHE_BYTES {
            let Some(entry) = self
                .entries
                .values_mut()
                .filter(|entry| entry.pixels != 0)
                .min_by_key(|entry| entry.slot.upgrade().map_or(0, |slot| slot.used.get()))
            else {
                return false;
            };
            if let Some(slot) = entry.slot.upgrade() {
                slot.image.borrow_mut().take();
            }
            self.bytes -= std::mem::take(&mut entry.pixels);
        }
        true
    }
}

#[derive(Debug)]
pub(crate) struct Images {
    resources: RefCell<Arc<BTreeMap<String, Arc<[u8]>>>>,
    cache: RefCell<Cache>,
    resource_epoch: Cell<u64>,
    serial: Cell<u64>,
    clock: Rc<Cell<u64>>,
    tx: mpsc::Sender<ResultMessage>,
    rx: RefCell<mpsc::Receiver<ResultMessage>>,
    pending: Cell<usize>,
    pub generation: Cell<u64>,
}
impl Default for Images {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            resources: Default::default(),
            cache: Default::default(),
            resource_epoch: Cell::new(0),
            serial: Cell::new(0),
            clock: Rc::default(),
            tx,
            rx: RefCell::new(rx),
            pending: Cell::new(0),
            generation: Cell::new(0),
        }
    }
}
impl Images {
    pub fn set_resources(&self, resources: BTreeMap<String, Arc<[u8]>>) {
        *self.resources.borrow_mut() = Arc::new(resources);
        self.resource_epoch.set(self.resource_epoch.get() + 1);
        let cache = self.cache.take();
        for entry in cache.entries.into_values() {
            if let Some(slot) = entry.slot.upgrade() {
                slot.cancelled.store(true, Ordering::Release);
                slot.image.borrow_mut().take();
            }
        }
        self.generation.set(self.generation.get() + 1);
    }
    pub fn pending(&self) -> bool {
        self.pending.get() > 0
    }
    pub fn poll(&self) -> bool {
        let mut cache = self.cache.borrow_mut();
        let mut changed = !cache.entries.is_empty() && cache.prune();
        while let Ok((key, epoch, serial, image)) = self.rx.borrow_mut().try_recv() {
            let was_full = self.pending.get() == 8;
            self.pending.set(self.pending.get() - 1);
            changed |= was_full;
            if epoch != self.resource_epoch.get() {
                continue;
            }
            let Some(slot) = cache
                .entries
                .get(&key)
                .and_then(|entry| entry.slot.upgrade())
                .filter(|slot| slot.serial == serial)
            else {
                continue;
            };
            slot.loading.set(false);
            if let Some(image) = image {
                let pixels = image.rgba.len();
                slot.size.set(Some(image.logical_size));
                slot.expected_pixels.set(pixels);
                slot.touch();
                if cache.make_room(pixels) {
                    cache.entries.get_mut(&key).expect("live slot").pixels = pixels;
                    cache.bytes += pixels;
                    *slot.image.borrow_mut() = Some(image);
                }
            } else {
                slot.expected_pixels.set(0);
            }
            changed = true;
        }
        if changed {
            self.generation.set(self.generation.get() + 1);
        }
        changed
    }
    pub fn size(&self, source: &ImageSource) -> Option<[f32; 2]> {
        if let ImageSource::Https(url) = source {
            return remote::resolve(url, false)
                .1
                .map(|image| image.logical_size);
        }
        let key = Key::local(source)?;
        self.cache
            .borrow()
            .entries
            .get(&key)?
            .slot
            .upgrade()?
            .size
            .get()
    }
    pub fn acquire(&self, source: &ImageSource) -> Option<Rc<Slot>> {
        let key = Key::local(source)?;
        let mut cache = self.cache.borrow_mut();
        if let Some(slot) = cache.entries.get(&key).and_then(|e| e.slot.upgrade()) {
            slot.touch();
            // A closed document can free room for a previously evicted slot.
            // Only refill if this won't evict another resident image and loop.
            if !slot.loading.get()
                && slot.expected_pixels.get() != 0
                && slot.image.borrow().is_none()
                && self.pending.get() < 8
            {
                cache.prune();
                if cache.bytes + slot.expected_pixels.get() <= CACHE_BYTES {
                    self.schedule(source, key, &slot);
                }
            }
            return Some(slot);
        }
        if self.pending.get() >= 8 {
            return None;
        }
        cache.prune();
        let charge = key.bytes();
        if cache.entries.len() == CACHE_KEYS || !cache.make_room(charge) {
            return None;
        }
        let serial = self.serial.get() + 1;
        self.serial.set(serial);
        let slot = Rc::new(Slot {
            serial,
            cancelled: Arc::default(),
            clock: Rc::clone(&self.clock),
            used: Cell::default(),
            size: Cell::default(),
            expected_pixels: Cell::default(),
            loading: Cell::default(),
            image: RefCell::default(),
        });
        slot.touch();
        cache.entries.insert(
            key.clone(),
            Entry {
                slot: Rc::downgrade(&slot),
                charge,
                pixels: 0,
            },
        );
        cache.bytes += charge;
        self.schedule(source, key, &slot);
        Some(slot)
    }

    fn schedule(&self, source: &ImageSource, key: Key, slot: &Slot) {
        slot.loading.set(true);
        self.pending.set(self.pending.get() + 1);
        let source = source.clone();
        let resources = self.resources.borrow().clone();
        let tx = self.tx.clone();
        let epoch = self.resource_epoch.get();
        let serial = slot.serial;
        let cancelled = slot.cancelled.clone();
        std::thread::spawn(move || {
            // Keep transient codec allocations serialized across windows.
            static DECODE: Mutex<()> = Mutex::new(());
            let _permit = DECODE.lock().expect("image decode");
            let decoded = if cancelled.load(Ordering::Acquire) {
                None
            } else {
                decode(&source, &resources)
            };
            let _ = tx.send((key, epoch, serial, decoded));
        });
    }
}
fn decode(source: &ImageSource, resources: &BTreeMap<String, Arc<[u8]>>) -> Option<Arc<Image>> {
    const LIMIT: usize = 32 * 1024 * 1024;
    let bytes = match source {
        ImageSource::Resource(url) => resources
            .get(url.trim_start_matches("res://").trim_start_matches('/'))?
            .to_vec(),
        ImageSource::File(url) => {
            let path = percent_encoding::percent_decode_str(url.strip_prefix("file://")?)
                .decode_utf8()
                .ok()?;
            let file = std::fs::File::open(path.as_ref()).ok()?;
            let mut bytes = Vec::new();
            file.take((LIMIT + 1) as u64).read_to_end(&mut bytes).ok()?;
            bytes
        }
        ImageSource::Data(uri) => {
            let url = uri.url();
            let (meta, data) = url.split_once(',')?;
            if data.len() > LIMIT * 2 {
                return None;
            }
            if meta.ends_with(";base64") {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .ok()?
            } else {
                percent_encoding::percent_decode_str(data).collect()
            }
        }
        ImageSource::Https(_) => return None,
    };
    if bytes.len() > LIMIT {
        return None;
    }
    let mut reader = image::ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode().ok()?;
    let logical_size = [decoded.width() as f32, decoded.height() as f32];
    // Four atlas tiles bound an individual image's upload; layout retains intrinsic dimensions.
    let image = if decoded.width() > 2044 || decoded.height() > 2044 {
        decoded.thumbnail(2044, 2044)
    } else {
        decoded
    }
    .into_rgba8();
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hash);
    let id = hash.finish();
    Some(Arc::new(Image {
        logical_size,
        id,
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw().into(),
    }))
}

pub(crate) mod remote;

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_slot(images: &Images, key: Key) -> Rc<Slot> {
        let slot = Rc::new(Slot {
            serial: 1,
            cancelled: Arc::default(),
            clock: Rc::clone(&images.clock),
            used: Cell::default(),
            size: Cell::default(),
            expected_pixels: Cell::default(),
            loading: Cell::default(),
            image: RefCell::default(),
        });
        let mut cache = images.cache.borrow_mut();
        cache.bytes += key.bytes();
        cache.entries.insert(
            key.clone(),
            Entry {
                slot: Rc::downgrade(&slot),
                charge: key.bytes(),
                pixels: 0,
            },
        );
        slot
    }

    fn complete(images: &Images, key: Key, epoch: u64) {
        let image = Arc::new(Image {
            logical_size: [2044., 2044.],
            id: 1,
            width: 2044,
            height: 2044,
            rgba: vec![0; 2044 * 2044 * 4].into(),
        });
        images.pending.set(1);
        images.tx.send((key, epoch, 1, Some(image))).unwrap();
        images.poll();
    }

    #[test]
    fn decoded_image_lru_is_byte_weighted_and_slots_release_on_close() {
        let images = Images::default();
        let key = |i| Key::Data([i; 32]);
        let a = fake_slot(&images, key(1));
        complete(&images, key(1), 0);
        let b = fake_slot(&images, key(2));
        complete(&images, key(2), 0);
        assert!(a.pixels().is_some());
        let c = fake_slot(&images, key(3));
        complete(&images, key(3), 0);
        assert!(b.pixels().is_none());
        assert!(a.pixels().is_some() && c.pixels().is_some());
        assert!(images.cache.borrow().bytes <= CACHE_BYTES);
        let pixels = c.pixels().unwrap();
        let weak = Arc::downgrade(&pixels);
        drop((a, b, c, pixels));
        assert!(weak.upgrade().is_none());
        images.poll();
        assert_eq!(images.cache.borrow().bytes, 0);
    }

    #[test]
    fn a_data_digest_has_constant_weight_and_old_resources_cannot_repopulate() {
        let url =
            ImageSource::parse(&format!("data:image/png,{}", "x".repeat(1024 * 1024))).unwrap();
        let key = Key::local(&url).unwrap();
        assert_eq!(key.bytes(), 256);
        let images = Images::default();
        let old = fake_slot(&images, key.clone());
        images.set_resources(BTreeMap::new());
        let current = fake_slot(&images, key.clone());
        complete(&images, key, 0);
        assert!(old.pixels().is_none() && current.pixels().is_none());
        assert_eq!(images.cache.borrow().bytes, 256);
    }
}
