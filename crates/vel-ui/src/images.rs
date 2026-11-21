//! Bounded asynchronous local/resource/data image decoding for minihtml.
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap},
    hash::{Hash, Hasher},
    io::{Cursor, Read},
    sync::{Arc, mpsc},
};
use vel_minihtml::ImageSource;
use vel_render::scene::Image;
#[derive(Debug)]
pub(crate) struct Images {
    resources: RefCell<Arc<BTreeMap<String, Arc<[u8]>>>>,
    ready: RefCell<HashMap<String, Option<Arc<Image>>>>,
    requested: RefCell<std::collections::HashSet<String>>,
    tx: mpsc::Sender<(String, Option<Arc<Image>>)>,
    rx: RefCell<mpsc::Receiver<(String, Option<Arc<Image>>)>>,
    pending: Cell<usize>,
    pub generation: Cell<u64>,
}
impl Default for Images {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            resources: Default::default(),
            ready: Default::default(),
            requested: Default::default(),
            tx,
            rx: RefCell::new(rx),
            pending: Cell::new(0),
            generation: Cell::new(0),
        }
    }
}
pub(crate) fn key(source: &ImageSource) -> &str {
    match source {
        ImageSource::File(s)
        | ImageSource::Resource(s)
        | ImageSource::Data(s)
        | ImageSource::Https(s) => s,
    }
}
impl Images {
    pub fn set_resources(&self, resources: BTreeMap<String, Arc<[u8]>>) {
        *self.resources.borrow_mut() = Arc::new(resources);
        self.ready.borrow_mut().clear();
        self.requested.borrow_mut().clear();
        self.generation.set(self.generation.get() + 1);
    }
    pub fn pending(&self) -> bool {
        self.pending.get() > 0
    }
    pub fn poll(&self) -> bool {
        let mut changed = false;
        while let Ok((key, image)) = self.rx.borrow_mut().try_recv() {
            self.ready.borrow_mut().insert(key, image);
            self.pending.set(self.pending.get().saturating_sub(1));
            changed = true;
        }
        if changed {
            self.generation.set(self.generation.get() + 1);
        }
        changed
    }
    pub fn get(&self, source: &ImageSource) -> Option<Arc<Image>> {
        if let ImageSource::Https(url) = source {
            return remote::resolve(url, false).1;
        }
        let key = key(source);
        if let Some(image) = self.ready.borrow().get(key) {
            return image.clone();
        }
        if self.requested.borrow().contains(key)
            || self.pending.get() >= 8
            || self.requested.borrow().len() >= 256
        {
            return None;
        }
        self.requested.borrow_mut().insert(key.into());
        self.pending.set(self.pending.get() + 1);
        let source = source.clone();
        let resources = self.resources.borrow().clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let decoded = decode(&source, &resources);
            let _ = tx.send((self::key(&source).into(), decoded));
        });
        None
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
        ImageSource::Data(url) => {
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
