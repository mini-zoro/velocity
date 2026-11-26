//! Finder, Dock and file-drop requests share the existing text/huge/hex tab loaders.
//! Native delegates can queue literal filesystem paths before a window is available.
use crate::{
    cli::{FileLocation, Options},
    instance::{Instance, Request, Start},
    panels::Panels,
    workspace::Workspace,
};
#[cfg(test)]
use std::fs::File;
use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
};

use vel_render::scene::Image;

const PREVIEW_BYTES: usize = 128 * 1024 * 1024;
static PREVIEW_CLOCK: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct ImageKey {
    path: PathBuf,
    disk: Option<(u64, std::time::SystemTime)>,
}

#[derive(Debug)]
struct PreviewPixels {
    image: Mutex<Option<Arc<Image>>>,
    active: AtomicUsize,
    last_used: AtomicU64,
}
impl PreviewPixels {
    fn new(image: Arc<Image>) -> Self {
        Self {
            image: Mutex::new(Some(image)),
            active: AtomicUsize::new(0),
            last_used: AtomicU64::new(PREVIEW_CLOCK.fetch_add(1, Ordering::Relaxed)),
        }
    }
    fn touch(&self) {
        self.last_used.store(
            PREVIEW_CLOCK.fetch_add(1, Ordering::Relaxed),
            Ordering::Relaxed,
        );
    }
    fn resident(&self) -> bool {
        self.image.lock().expect("preview pixels").is_some()
    }
}

#[derive(Default)]
struct PreviewCache {
    entries: HashMap<ImageKey, Weak<PreviewPixels>>,
    // An evicted image can still be borrowed by the current renderer scene.
    // Keep charging its bytes until that scene releases its last reference.
    charged: Vec<(Weak<Image>, usize)>,
}
impl PreviewCache {
    fn lookup(&self, key: &ImageKey) -> Option<Arc<PreviewPixels>> {
        self.entries
            .get(key)?
            .upgrade()
            .filter(|entry| entry.resident())
    }
    fn bytes(&mut self) -> usize {
        self.charged.retain(|(image, _)| image.strong_count() > 0);
        self.charged.iter().map(|(_, bytes)| bytes).sum()
    }
    fn make_room(&mut self, needed: usize, limit: usize) -> bool {
        while self.bytes().saturating_add(needed) > limit {
            let Some(victim) = self
                .entries
                .values()
                .filter_map(Weak::upgrade)
                .filter(|entry| entry.resident())
                .min_by_key(|entry| {
                    (
                        entry.active.load(Ordering::Relaxed) != 0,
                        entry.last_used.load(Ordering::Relaxed),
                    )
                })
            else {
                return false;
            };
            victim.image.lock().expect("preview pixels").take();
        }
        self.entries
            .retain(|_, entry| entry.upgrade().is_some_and(|entry| entry.resident()));
        true
    }
    fn insert(&mut self, key: ImageKey, image: Arc<Image>) -> Arc<PreviewPixels> {
        self.charged
            .push((Arc::downgrade(&image), image.rgba.len()));
        let entry = Arc::new(PreviewPixels::new(image));
        self.entries.insert(key, Arc::downgrade(&entry));
        entry
    }
}

#[cfg(test)]
mod preview_budget_tests {
    use super::*;

    fn image(id: u64) -> Arc<Image> {
        Arc::new(Image {
            logical_size: [2000., 2000.],
            width: 2000,
            height: 2000,
            id,
            rgba: vec![0; 2000 * 2000 * 4].into(),
        })
    }

    fn key(i: u64) -> ImageKey {
        ImageKey {
            path: format!("preview-{i}.png").into(),
            disk: None,
        }
    }

    #[test]
    fn fifty_reopens_release_the_only_live_decode() {
        let mut cache = PreviewCache::default();
        for id in 0..50 {
            assert!(cache.make_room(crate::image_decode::MAX_DECODED_BYTES, PREVIEW_BYTES));
            let entry = cache.insert(key(0), image(id));
            assert!(Arc::ptr_eq(&entry, &cache.lookup(&key(0)).unwrap()));
            assert_eq!(cache.bytes(), 16_000_000);
            drop(entry);
            assert_eq!(cache.bytes(), 0);
        }
    }

    #[test]
    fn process_preview_limit_evicts_inactive_pixels_first() {
        let mut cache = PreviewCache::default();
        let active = cache.insert(key(0), image(0));
        active.active.store(1, Ordering::Relaxed);
        let mut others = Vec::new();
        for id in 1..12 {
            assert!(cache.make_room(crate::image_decode::MAX_DECODED_BYTES, PREVIEW_BYTES));
            others.push(cache.insert(key(id), image(id)));
            assert!(cache.bytes() <= PREVIEW_BYTES);
        }
        assert!(active.resident());
        assert!(!others[0].resident());
        assert!(others.last().unwrap().resident());
        drop((active, others));
        assert_eq!(cache.bytes(), 0);
    }
}

#[derive(Debug)]
enum ImageResult {
    Ready(Arc<PreviewPixels>),
    Failed,
    Evicted,
}
#[derive(Debug)]
struct PendingImage {
    cancelled: Arc<AtomicBool>,
    result: mpsc::Receiver<ImageResult>,
}
impl Drop for PendingImage {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}
struct ImageJob {
    key: ImageKey,
    cancelled: Arc<AtomicBool>,
    result: mpsc::SyncSender<ImageResult>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpenAs {
    Image,
    Text,
}
pub(crate) type OpenAsChoices =
    std::rc::Rc<std::cell::RefCell<std::collections::BTreeMap<PathBuf, OpenAs>>>;
#[derive(Default)]
struct ImageQueue {
    jobs: Mutex<VecDeque<ImageJob>>,
    ready: Condvar,
}
impl ImageQueue {
    fn submit(path: &Path, disk: Option<(u64, std::time::SystemTime)>) -> PendingImage {
        static QUEUE: OnceLock<Arc<ImageQueue>> = OnceLock::new();
        let queue = QUEUE.get_or_init(|| {
            let queue = Arc::new(Self::default());
            let worker = queue.clone();
            std::thread::spawn(move || worker.run());
            queue
        });
        let (tx, result) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut jobs = queue.jobs.lock().expect("image queue");
        jobs.retain(|job| !job.cancelled.load(Ordering::Acquire));
        if jobs.len() == 32 {
            jobs.pop_front();
        }
        jobs.push_back(ImageJob {
            key: ImageKey {
                path: path.to_owned(),
                disk,
            },
            cancelled: cancelled.clone(),
            result: tx,
        });
        queue.ready.notify_one();
        PendingImage { cancelled, result }
    }

    fn run(&self) {
        let mut cache = PreviewCache::default();
        loop {
            let job = {
                let jobs = self.jobs.lock().expect("image queue");
                let mut jobs = self
                    .ready
                    .wait_while(jobs, |jobs| jobs.is_empty())
                    .expect("image queue");
                jobs.pop_front().expect("ready image")
            };
            if !job.cancelled.load(Ordering::Acquire) {
                let result = if let Some(entry) = cache.lookup(&job.key) {
                    ImageResult::Ready(entry)
                } else if !cache.make_room(crate::image_decode::MAX_DECODED_BYTES, PREVIEW_BYTES) {
                    ImageResult::Evicted
                } else if let Some(image) = decode_image(&job.key.path, &job.cancelled) {
                    if job.cancelled.load(Ordering::Acquire) {
                        continue;
                    }
                    ImageResult::Ready(cache.insert(job.key, image))
                } else {
                    ImageResult::Failed
                };
                let _ = job.result.send(result);
            }
        }
    }
}

pub(crate) fn image_matches(tab: &crate::workspace::Tab, root: &Path, path: &Path) -> bool {
    let patterns = tab
        .editor
        .settings
        .get("image_file_patterns")
        .and_then(vel_config::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(vel_config::Value::as_str)
        .map(|pattern| {
            let mut folded = pattern.to_owned();
            if let Some(dot) = folded
                .rfind('.')
                .filter(|&dot| !folded[dot..].contains('/'))
            {
                folded[dot..].make_ascii_lowercase();
            }
            folded
        })
        .collect::<Vec<_>>();
    let folded = path
        .extension()
        .and_then(|ext| ext.to_str())
        .filter(|ext| ext.bytes().any(|b| b.is_ascii_uppercase()))
        .map(|ext| path.with_extension(ext.to_ascii_lowercase()));
    vel_search::project::Patterns::new(&patterns).matches(
        root,
        folded.as_deref().unwrap_or(path),
        false,
    )
}

/// The folder `image_file_patterns` are matched against for `path`: its project folder (the
/// deepest one containing it), else its own folder. Decoding and tab labels share it.
pub(crate) fn image_root<'a>(
    folders: &'a [vel_search::project::Folder],
    path: &'a Path,
) -> &'a Path {
    folders
        .iter()
        .filter(|f| path.starts_with(&f.path))
        .max_by_key(|f| f.path.as_os_str().len())
        .map_or_else(|| path.parent().unwrap_or(path), |f| f.path.as_path())
}

#[derive(Debug, Default)]
pub(crate) struct ImagePreview {
    path: Option<PathBuf>,
    revision: u64,
    epoch: u64,
    /// Identity of the loaded bytes: a reload from disk decodes again.
    snapshot: usize,
    wants_image: bool,
    pending: Option<PendingImage>,
    pixels: Option<Arc<PreviewPixels>>,
    visible: bool,
    evicted: bool,
    /// The file matched the image patterns but could not be decoded; its bytes are shown.
    pub failed: bool,
}
impl ImagePreview {
    pub fn image(&self) -> Option<Arc<Image>> {
        self.pixels
            .as_ref()?
            .image
            .lock()
            .expect("preview pixels")
            .clone()
    }

    fn set_visible(&mut self, visible: bool) {
        if self.visible != visible {
            self.visible = visible;
            if let Some(pixels) = &self.pixels {
                if visible {
                    pixels.active.fetch_add(1, Ordering::Relaxed);
                    pixels.touch();
                } else {
                    pixels.active.fetch_sub(1, Ordering::Relaxed);
                }
            }
        }
    }

    fn replace_pixels(&mut self, pixels: Option<Arc<PreviewPixels>>) {
        let visible = self.visible;
        self.set_visible(false);
        self.pixels = pixels;
        self.set_visible(visible);
    }

    #[cfg(test)]
    pub fn set_test_image(&mut self, image: Arc<Image>) {
        self.replace_pixels(Some(Arc::new(PreviewPixels::new(image))));
    }

    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn suspend(&mut self) {
        self.set_visible(false);
        if self.pending.take().is_some() {
            // Returning to this tab should request a new decode.
            self.path = None;
        }
    }
    /// Drops a parked, unselected tab's pixels and cancels any decode. A tab
    /// selected again requests the image lazily using the same decode path.
    pub fn release_inactive(&mut self) {
        self.suspend();
        self.evicted |= self.pixels.take().is_some();
    }

    #[cfg(test)]
    pub fn cancellation_flag(&self) -> Option<Arc<AtomicBool>> {
        self.pending.as_ref().map(|p| p.cancelled.clone())
    }
    pub fn sync(
        &mut self,
        tab: &crate::workspace::Tab,
        folders: &[vel_search::project::Folder],
    ) -> bool {
        let reentering = !self.visible;
        self.set_visible(true);
        let path = tab.path.as_deref().filter(|_| tab.editor.binary.is_some());
        if path.is_none() && self.path.is_none() {
            return false;
        }
        let revision = tab.editor.settings.revision();
        let epoch = tab.editor.revision;
        let wants_image = path.is_some_and(|path| match tab.open_as() {
            Some(OpenAs::Image) => true,
            Some(OpenAs::Text) => false,
            None => image_matches(tab, image_root(folders, path), path),
        });
        let snapshot = tab
            .editor
            .binary
            .as_ref()
            .map_or(0, |b| Arc::as_ptr(b) as usize);
        let mut changed = false;
        let retry =
            reentering && (self.evicted || self.pixels.as_ref().is_some_and(|p| !p.resident()));
        if retry
            || (path, revision, epoch, snapshot, wants_image)
                != (
                    self.path.as_deref(),
                    self.revision,
                    self.epoch,
                    self.snapshot,
                    self.wants_image,
                )
        {
            // The same file decoded again (settings, zoom, a reload from disk) keeps showing
            // its last result until the new one arrives: no frame without the picture or
            // without "[hex]". Another file starts from nothing.
            let same_file = path.is_some() && path == self.path.as_deref();
            self.path = path.map(Path::to_owned);
            self.revision = revision;
            self.epoch = epoch;
            self.snapshot = snapshot;
            self.wants_image = wants_image;
            self.pending = None;
            if !same_file || !wants_image {
                changed = self.pixels.is_some() || self.failed;
                self.replace_pixels(None);
                self.failed = false;
            }
            if let Some(path) = path.filter(|_| wants_image) {
                self.pending = Some(ImageQueue::submit(path, tab.disk_stamp));
            }
            self.evicted = false;
        }
        if let Some(pending) = &self.pending {
            match pending.result.try_recv() {
                Ok(result) => {
                    self.failed = matches!(result, ImageResult::Failed);
                    self.evicted = matches!(result, ImageResult::Evicted);
                    self.replace_pixels(match result {
                        ImageResult::Ready(pixels) => Some(pixels),
                        _ => None,
                    });
                    changed = true;
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.failed = true;
                    changed = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        changed
    }
}
impl Drop for ImagePreview {
    fn drop(&mut self) {
        self.set_visible(false);
    }
}

fn decode_image(path: &Path, cancelled: &AtomicBool) -> Option<Arc<Image>> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let image = crate::image_decode::preview(path, cancelled)?;
    Some(Arc::new(image.into_scene(
        NEXT.fetch_add(1, Ordering::Relaxed) | (1 << 63),
    )))
}

/// Chooses an already recovered window for a launch request.
/// An explicit new-window request always creates an additional window.
pub(crate) fn restored_window(session: &vel_session::Session, options: &Options) -> Option<usize> {
    let windows = &session.windows;
    if options.new_window
        || (options.mode != crate::cli::Mode::Interactive && !options.restore_session)
    {
        return None;
    }
    if let Some(project) = &options.project {
        return windows.iter().position(|window| {
            window
                .project
                .as_deref()
                .is_some_and(|p| same_path(p, project))
        });
    }
    let directories: Vec<_> = options.files.iter().filter(|f| f.path.is_dir()).collect();
    if !options.add_to_window && !directories.is_empty() {
        return windows.iter().position(|window| {
            directories.iter().all(|directory| {
                window
                    .folders
                    .iter()
                    .any(|folder| same_path(folder, &directory.path))
            })
        });
    }
    let files: Vec<_> = options.files.iter().filter(|f| !f.path.is_dir()).collect();
    if !files.is_empty() {
        // The command-line invocation that starts the primary should reuse the
        // same window as an invocation forwarded to an existing instance.
        let documents: std::collections::BTreeMap<_, _> = session
            .documents
            .iter()
            .filter_map(|d| d.path.as_deref().map(|path| (d.id, path)))
            .collect();
        if let Some(index) = windows.iter().position(|w| {
            !w.retained_closed
                && files.iter().all(|file| {
                    w.sheets.iter().any(|sheet| {
                        documents
                            .get(&sheet.document)
                            .is_some_and(|path| same_path(path, &file.path))
                    })
                })
        }) {
            return Some(index);
        }
    }
    (!windows.is_empty()).then_some(0)
}
pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    let normalized = crate::workspace::absolute;
    match (normalized(a), normalized(b)) {
        (Ok(a), Ok(b)) => vel_config::paths::equal(&a, &b),
        _ => vel_config::paths::equal(a, b),
    }
}

/// Result of relaying a secondary LaunchServices process's actual open event.
#[derive(Debug)]
pub enum NativeForward {
    /// The existing process acknowledged the native request; the proxy can quit.
    Forwarded,
    /// The previous process exited; retain this listener and open paths locally.
    Primary(Instance),
}

/// Relay literal native paths to the existing key window, or claim the profile if
/// its previous primary has exited. The returned listener must be attached to App.
pub fn forward_native_paths(
    data_dir: Option<&Path>,
    paths: &[PathBuf],
) -> Result<NativeForward, String> {
    match Instance::start(data_dir, &native_request(paths)?)? {
        Start::Forwarded => Ok(NativeForward::Forwarded),
        Start::Primary(instance) => Ok(NativeForward::Primary(instance)),
    }
}
pub(crate) fn native_request(paths: &[PathBuf]) -> Result<Request, String> {
    let mut request = Request::new(
        &Options {
            files: paths.iter().cloned().map(FileLocation::path_only).collect(),
            add_to_window: false,
            ..Options::default()
        },
        None,
    )?;
    request.native_open = true;
    Ok(request)
}

/// Asynchronous, ordered LaunchServices relay. A quiet period lets Apple events that
/// arrive during an acknowledgement join the same recipient before it exits.
#[derive(Debug)]
pub struct Forwarder {
    data: Option<PathBuf>,
    pending: VecDeque<Vec<PathBuf>>,
    inflight: Option<std::sync::mpsc::Receiver<Result<NativeForward, String>>>,
    current: Vec<PathBuf>,
    last_activity: std::time::Instant,
}
#[derive(Debug)]
pub enum ForwardPoll {
    Waiting,
    Quit,
    Primary {
        instance: Instance,
        paths: Vec<PathBuf>,
    },
    Failed(String),
}
impl Forwarder {
    pub const QUIET: std::time::Duration = std::time::Duration::from_secs(1);
    pub fn new(data: Option<PathBuf>, now: std::time::Instant) -> Self {
        Self {
            data,
            pending: VecDeque::new(),
            inflight: None,
            current: vec![],
            last_activity: now,
        }
    }
    fn failure(&self, error: &str) -> String {
        let paths = self
            .current
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>();
        if paths.is_empty() {
            format!("Native reopen: {error}")
        } else {
            format!("{}: {error}", paths.join(", "))
        }
    }
    pub fn enqueue(&mut self, paths: Vec<PathBuf>, now: std::time::Instant) {
        self.pending.push_back(paths);
        self.last_activity = now;
    }
    pub fn poll(&mut self, now: std::time::Instant) -> ForwardPoll {
        if let Some(rx) = &self.inflight {
            let result = match rx.try_recv() {
                Ok(result) => result,
                Err(std::sync::mpsc::TryRecvError::Empty) => return ForwardPoll::Waiting,
                Err(error) => Err(error.to_string()),
            };
            self.inflight = None;
            self.last_activity = now;
            match result {
                Ok(NativeForward::Forwarded) => self.current.clear(),
                Ok(NativeForward::Primary(instance)) => {
                    let mut paths = std::mem::take(&mut self.current);
                    paths.extend(self.pending.drain(..).flatten());
                    return ForwardPoll::Primary { instance, paths };
                }
                Err(error) => return ForwardPoll::Failed(self.failure(&error)),
            }
        }
        if let Some(paths) = self.pending.pop_front() {
            self.current = paths.clone();
            let data = self.data.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            match std::thread::Builder::new()
                .name("velocity-native-forward".into())
                .spawn(move || {
                    let _ = tx.send(forward_native_paths(data.as_deref(), &paths));
                }) {
                Ok(_) => self.inflight = Some(rx),
                Err(error) => return ForwardPoll::Failed(self.failure(&error.to_string())),
            }
            ForwardPoll::Waiting
        } else if now.saturating_duration_since(self.last_activity) >= Self::QUIET {
            ForwardPoll::Quit
        } else {
            ForwardPoll::Waiting
        }
    }
}

/// Ordered native open requests retained until the target window is ready.
#[derive(Debug, Default)]
pub struct Router {
    pending: VecDeque<PathBuf>,
}
impl Router {
    /// Queue paths without opening or inspecting them before a window exists.
    pub fn enqueue(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.pending.extend(paths);
    }
    /// Apply all queued requests to the chosen window, collecting failures without
    /// discarding later valid requests. Repeated paths focus the existing tab/root.
    pub fn apply(&mut self, workspace: &mut Workspace, panels: &mut Panels) -> Result<(), String> {
        let mut errors = Vec::new();
        for path in self.pending.drain(..) {
            if let Err(error) = open_path(workspace, panels, &path) {
                errors.push(vel_fileio::regular::error(&path, error));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OpenWindow {
    #[default]
    Never,
    FinderOnly,
    Always,
}
impl OpenWindow {
    pub fn setting(value: Option<&serde_json::Value>) -> Self {
        // The public settings reference documents the Finder/Dock distinction:
        // https://docs.sublimetext.io/reference/settings.html
        // Preserve Velocity's previous false default when the setting is absent.
        match value {
            Some(v) if v.as_bool() == Some(true) || v.as_str() == Some("finder_only") => {
                Self::FinderOnly
            }
            Some(v) if v.as_str() == Some("always") => Self::Always,
            _ => Self::Never,
        }
    }
}
#[derive(Debug, Default)]
pub struct WindowRoute {
    pub project: Option<PathBuf>,
    pub(crate) project_spelling: Option<PathBuf>,
    pub folders: Vec<PathBuf>,
    pub(crate) folder_spellings: Vec<PathBuf>,
    pub files: Vec<PathBuf>,
    pub untouched: bool,
}
impl WindowRoute {
    pub fn from_snapshot(
        window: &vel_session::Window,
        documents: &[vel_session::Document],
    ) -> Self {
        let project_spelling = window.project.clone();
        let folder_spellings = window.folders.clone();
        let mut folders: Vec<_> = folder_spellings
            .iter()
            .map(|path| canonical_route_path(path))
            .collect();
        folders.sort_unstable();
        folders.dedup();
        Self {
            project: project_spelling.as_deref().map(canonical_route_path),
            project_spelling,
            folders,
            folder_spellings,
            files: documents
                .iter()
                .filter_map(|doc| doc.path.clone())
                .collect(),
            // A reserved window is not an untouched startup window; it has
            // not been constructed yet and is reserved for its saved contents.
            untouched: false,
        }
    }
    pub fn update(&mut self, workspace: &Workspace, startup: bool) {
        if self.project_spelling.as_deref() != workspace.project_path() {
            self.project_spelling = workspace.project_path().map(Path::to_owned);
            self.project = workspace.project_path().map(canonical_route_path);
        }
        let folders = || workspace.folders.iter().map(|f| &f.path);
        if !self.folder_spellings.iter().eq(folders()) {
            self.folder_spellings = folders().cloned().collect();
            self.folders = self
                .folder_spellings
                .iter()
                .map(|path| canonical_route_path(path))
                .collect();
            self.folders.sort_unstable();
            self.folders.dedup();
        }
        let files = || {
            workspace
                .tabs
                .iter()
                .filter(|t| !t.goto_preview)
                .filter_map(|t| t.path.as_ref())
        };
        if !self.files.iter().eq(files()) {
            self.files = files().cloned().collect();
        }
        self.untouched = startup
            && self.project.is_none()
            && self.folders.is_empty()
            && workspace.is_empty()
            && self.files.is_empty()
            && workspace.editor().text.is_empty()
            && !workspace.tabs[0].dirty()
            && workspace.closed_files.is_empty()
            && workspace.editor().revision == 0;
    }
}
fn canonical_route_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}
pub struct RouteRequest {
    project: Option<PathBuf>,
    dirs: std::collections::BTreeSet<PathBuf>,
    files: Vec<PathBuf>,
    separate: bool,
}
impl RouteRequest {
    pub fn prepare(request: &mut Request, setting: OpenWindow) -> Self {
        if request.project.is_none()
            && let Some(index) = request.files.iter().position(|f| {
                f.path.extension().is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("sublime-project")
                        || ext.eq_ignore_ascii_case("sublime-workspace")
                })
            })
        {
            request.project = Some(request.files.remove(index).path);
        }
        if let Some(project) = &mut request.project
            && project
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("sublime-workspace"))
        {
            *project = vel_session::project::project_path(project);
        }
        let (mut dirs, mut files) = (std::collections::BTreeSet::new(), Vec::new());
        for file in &request.files {
            let path = canonical_route_path(&file.path);
            if path.is_dir() {
                dirs.insert(path);
            } else {
                files.push(path);
            }
        }
        let has_input = request.stdin.is_some() || !request.files.is_empty();
        Self {
            project: request.project.as_deref().map(canonical_route_path),
            separate: request.new_window
                || (!request.add
                    && (!dirs.is_empty()
                        || (has_input
                            && (setting == OpenWindow::Always
                                || (setting == OpenWindow::FinderOnly && request.native_open))))),
            dirs,
            files,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    Window(u64),
    New,
}

/// Build 4126 reuses the empty startup window for CLI -n; build 4121
/// opens unmatched folders separately. <https://www.sublimetext.com/download>
pub fn route(
    request: &RouteRequest,
    windows: &std::collections::BTreeMap<u64, WindowRoute>,
    current: Option<u64>,
) -> Route {
    route_ready(request, windows, current, |_| true)
}

pub(crate) fn route_ready(
    request: &RouteRequest,
    windows: &std::collections::BTreeMap<u64, WindowRoute>,
    current: Option<u64>,
    ready: impl Fn(u64) -> bool + Clone,
) -> Route {
    let mut preferred = current
        .into_iter()
        .filter_map(|id| windows.get_key_value(&id))
        .chain(windows.iter().filter(|(id, _)| Some(**id) != current))
        .filter(|(id, _)| ready(**id));
    let existing = preferred.clone().find(|(_, window)| {
        if let Some(project) = &request.project {
            return window.project.as_ref() == Some(project);
        }
        if !request.dirs.is_empty() {
            return request.dirs.len() == window.folders.len()
                && window.folders.iter().all(|f| request.dirs.contains(f));
        }
        !request.files.is_empty() && request.files.iter().all(|file| window.files.contains(file))
    });
    if let Some((id, _)) = existing {
        return Route::Window(*id);
    }
    if request.project.is_none() && !request.separate {
        return preferred
            .clone()
            .next()
            .map_or(Route::New, |(id, _)| Route::Window(*id));
    }
    preferred
        .find(|(_, window)| window.untouched)
        .map_or(Route::New, |(id, _)| Route::Window(*id))
}

/// Apply every CLI location, collecting all failures in order without aborting the batch.
pub fn open_locations(
    workspace: &mut Workspace,
    panels: &mut Panels,
    locations: impl IntoIterator<Item = FileLocation>,
) -> Vec<String> {
    let mut errors = Vec::new();
    for location in locations {
        let path = location.path.clone();
        let result = if path.is_dir() {
            panels.add_folder(workspace, path.clone())
        } else {
            workspace
                .open(location)
                .map(|()| panels.note_sidebar_spelling(workspace, &path))
        };
        if let Err(error) = result {
            errors.push(vel_fileio::regular::error(&path, error));
        }
    }
    errors
}
/// Apply successful trash paths, detaching any affected project roots through the same
/// persistence/refresh path as Detach. Failed trash entries must not be supplied here.
pub fn apply_trashed(
    workspace: &mut Workspace,
    panels: &mut Panels,
    paths: &[PathBuf],
) -> Result<(), String> {
    crate::context_menu::apply_delete(workspace, paths);
    let roots: Vec<_> = workspace
        .folders
        .iter()
        .filter(|folder| {
            #[cfg(windows)]
            return crate::context_menu::path_was_deleted(&folder.path, paths);
            #[cfg(not(windows))]
            paths.iter().any(|path| folder.path.starts_with(path))
        })
        .map(|folder| folder.path.clone())
        .collect();
    let mut errors = Vec::new();
    for root in roots {
        if let Err(error) = panels.remove_sidebar_folder(workspace, &root) {
            errors.push(error);
        }
    }
    panels.refresh_sidebar(workspace);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}
/// Open literal native paths in an existing window; the caller presents any errors.
/// Numeric colon suffixes are part of the filename here, never CLI line/column syntax.
pub fn open_paths(
    workspace: &mut Workspace,
    panels: &mut Panels,
    paths: &[PathBuf],
) -> Result<(), String> {
    let mut router = Router::default();
    router.enqueue(paths.iter().cloned());
    router.apply(workspace, panels)
}

fn open_path(workspace: &mut Workspace, panels: &mut Panels, path: &Path) -> Result<(), String> {
    // Validate before selecting existing tabs too: missing or unreadable native requests
    // must report their path rather than silently succeeding against an old buffer.
    let spelling = path;
    let path = match fs::canonicalize(path) {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return workspace.open(FileLocation::path_only(path.into()));
        }
        Err(e) => return Err(e.to_string()),
    };
    let metadata = fs::metadata(&path).map_err(|e| e.to_string())?;
    if metadata.is_dir() {
        // Test actual access rather than Unix mode bits (ACLs and user identity matter).
        fs::read_dir(&path)
            .map_err(|e| e.to_string())?
            .next()
            .transpose()
            .map_err(|e| e.to_string())?;
        panels.add_folder(workspace, path)
    } else if metadata.is_file() {
        vel_fileio::regular::read(&path)?;
        workspace
            .open(FileLocation::path_only(path))
            .map(|()| panels.note_sidebar_spelling(workspace, spelling))
    } else {
        Err("only regular files and folders can be opened".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        time::{Duration, Instant},
    };
    use vel_text::Text;

    fn window() -> (Workspace, Panels) {
        let workspace = Workspace::new(Text::new(), None, None, false).unwrap();
        let panels = Panels::new(&workspace, false);
        (workspace, panels)
    }
    #[test]
    fn forwarded_native_requests_keep_literal_paths_and_native_duplicate_semantics() {
        let request =
            native_request(&[PathBuf::from("literal:2:3"), PathBuf::from("folder")]).unwrap();
        assert!(request.native_open);
        assert!(!request.add);
        assert!(!request.new_window);
        assert!(!request.wait);
        assert!(request.stdin.is_none());
        assert!(request.files[0].path.is_absolute());
        assert!(request.files[0].path.ends_with("literal:2:3"));
        assert!(!request.files[0].has_position());
    }
    #[test]
    fn before_window_queue_opens_files_folders_and_literal_colon_names_once_ready() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(if cfg!(windows) {
            "你好.txt"
        } else {
            "你好:2:3"
        });
        let folder = directory.path().join("folder");
        fs::write(&file, "first\nsecond\n").unwrap();
        fs::create_dir(&folder).unwrap();
        let mut router = Router::default();
        router.enqueue([file.clone(), folder.clone()]);
        assert_eq!(router.pending.len(), 2);
        let (mut workspace, mut panels) = window();
        assert!(workspace.tabs[0].path.is_none());
        router.apply(&mut workspace, &mut panels).unwrap();
        assert!(router.pending.is_empty());
        assert_eq!(workspace.tabs.len(), 1);
        assert_eq!(
            workspace.tabs[0].path.as_ref(),
            Some(&file.canonicalize().unwrap())
        );
        assert_eq!(workspace.editor().caret, 0);
        assert_eq!(workspace.editor().text.to_string(), "first\nsecond\n");
        assert_eq!(workspace.folders.len(), 1);
        assert_eq!(workspace.folders[0].path, folder.canonicalize().unwrap());
        assert!(panels.side_visible);
        router.apply(&mut workspace, &mut panels).unwrap();
        assert_eq!(workspace.tabs.len(), 1);
        assert_eq!(workspace.folders.len(), 1);
    }
    #[test]
    fn duplicates_focus_existing_tab_and_preserve_unsaved_edits() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("a.txt");
        let second = directory.path().join("b.txt");
        fs::write(&first, "saved").unwrap();
        fs::write(&second, "second").unwrap();
        let (mut workspace, mut panels) = window();
        open_paths(&mut workspace, &mut panels, std::slice::from_ref(&first)).unwrap();
        workspace
            .editor_mut()
            .run("insert", &serde_json::json!({"characters":"unsaved "}))
            .unwrap();
        workspace
            .editor_mut()
            .set_selection(vel_buffer::Selection::from_regions([
                vel_buffer::Region::new(2, 5),
            ]));
        workspace.tabs[0].scroll = (24., 456.);
        workspace.tabs[0].transient = true;
        workspace.take_pending_center();
        let selection = workspace.editor().regions();
        open_paths(
            &mut workspace,
            &mut panels,
            &[second, directory.path().join("./a.txt")],
        )
        .unwrap();
        assert_eq!(workspace.tabs.len(), 2);
        assert_eq!(workspace.active, 0);
        assert!(workspace.tabs[0].dirty());
        assert!(!workspace.tabs[0].transient);
        assert_eq!(workspace.editor().regions(), selection);
        assert_eq!(workspace.tabs[0].scroll, (24., 456.));
        assert!(!workspace.take_pending_center());
        assert_eq!(workspace.editor().text.to_string(), "unsaved saved");
        #[cfg(unix)]
        {
            let alias = directory.path().join("alias.txt");
            std::os::unix::fs::symlink(&first, &alias).unwrap();
            let folder_alias = directory.path().join("folder-alias");
            let folder = directory.path().join("folder");
            fs::create_dir(&folder).unwrap();
            std::os::unix::fs::symlink(&folder, &folder_alias).unwrap();
            open_paths(&mut workspace, &mut panels, &[alias, folder, folder_alias]).unwrap();
            assert_eq!(workspace.tabs.len(), 2);
            assert_eq!(workspace.active, 0);
            assert_eq!(workspace.editor().regions(), selection);
            assert_eq!(workspace.tabs[0].scroll, (24., 456.));
            assert!(!workspace.take_pending_center());
            assert_eq!(workspace.folders.len(), 1);
        }
    }
    #[test]
    fn missing_native_paths_open_as_new_and_later_valid_paths_still_open() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing");
        let valid = directory.path().join("valid.txt");
        fs::write(&valid, "valid").unwrap();
        let (mut workspace, mut panels) = window();
        open_paths(&mut workspace, &mut panels, &[missing.clone(), valid]).unwrap();
        assert_eq!(workspace.tabs.len(), 2);
        assert_eq!(
            workspace.tabs[0].path.as_ref(),
            Some(&directory.path().canonicalize().unwrap().join("missing"))
        );
        assert!(!workspace.tabs[0].deleted);
        assert_eq!(workspace.editor().text.to_string(), "valid");
    }
    #[cfg(unix)]
    #[test]
    fn unreadable_files_and_folders_report_their_paths_without_opening_tabs() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("unreadable.txt");
        let folder = directory.path().join("unreadable-folder");
        fs::write(&file, "private").unwrap();
        fs::create_dir(&folder).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o0)).unwrap();
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o0)).unwrap();
        // Privileged test runners can legitimately read mode-000 paths; assert that
        // routing follows actual OS access rather than manufacturing a permission error.
        let inaccessible = [file.clone(), folder.clone()]
            .into_iter()
            .filter(|path| {
                if path == &file {
                    File::open(path).is_err()
                } else {
                    fs::read_dir(path).is_err()
                }
            })
            .collect::<Vec<_>>();
        let (mut workspace, mut panels) = window();
        let result = open_paths(&mut workspace, &mut panels, &inaccessible);
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o700)).unwrap();
        if !inaccessible.is_empty() {
            let error = result.unwrap_err();
            for path in inaccessible {
                assert!(error.contains(&path.display().to_string()));
            }
        }
        assert!(workspace.tabs[0].path.is_none());
        assert!(workspace.folders.is_empty());
    }
    #[cfg(unix)]
    #[test]
    fn special_files_fail_without_attempting_a_stream_read() {
        use std::os::unix::net::UnixListener;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("socket");
        let _listener = UnixListener::bind(&path).unwrap();
        let (mut workspace, mut panels) = window();
        let error = open_paths(&mut workspace, &mut panels, &[path]).unwrap_err();
        assert!(error.contains("only regular files and folders"));
        assert!(workspace.tabs[0].path.is_none());
    }
    #[test]
    fn binaries_and_huge_text_delegate_to_existing_snapshot_and_background_loaders() {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("binary.bin");
        let huge = directory.path().join("huge.log");
        fs::write(&binary, b"\0binary\xff").unwrap();
        let mut file = File::create(&huge).unwrap();
        let block = b"a log line\n".repeat(8 * 1024);
        for _ in 0..800 {
            file.write_all(&block).unwrap();
        }
        drop(file);
        let (mut workspace, mut panels) = window();
        open_paths(&mut workspace, &mut panels, std::slice::from_ref(&huge)).unwrap();
        assert!(workspace.tabs[workspace.active].loading.is_some());
        assert!(workspace.editor().read_only);
        assert!(workspace.editor().source_bytes >= 64 * 1024 * 1024);
        let cancel = workspace.tabs[workspace.active]
            .loading
            .as_ref()
            .unwrap()
            .cancellation();
        workspace.close(workspace.active).unwrap();
        assert!(cancel.is_cancelled());
        open_paths(&mut workspace, &mut panels, &[binary]).unwrap();
        assert!(workspace.tabs[workspace.active].binary_loading.is_some());
        assert!(workspace.editor().read_only);
        let deadline = Instant::now() + Duration::from_secs(30);
        while workspace.tabs[workspace.active].binary_loading.is_some() {
            workspace.poll();
            assert!(Instant::now() < deadline, "{:?}", workspace.messages);
            std::thread::park_timeout(Duration::from_millis(1));
        }
        assert_eq!(
            workspace.editor().binary.as_ref().unwrap().view.len_bytes(),
            8
        );
        assert!(workspace.editor().read_only);
    }
}

#[cfg(test)]
mod round15_tests {
    use super::*;
    use std::time::{Duration, Instant};
    use vel_text::Text;
    #[test]
    fn primary_takeover_keeps_inflight_and_all_later_native_batches_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let request = Request::new(&Options::default(), None).unwrap();
        #[cfg(unix)]
        let start = Instance::start_fixture(dir.path(), &request);
        #[cfg(windows)]
        let start = Instance::start(Some(dir.path()), &request);
        let Start::Primary(instance) = start.unwrap() else {
            panic!("primary")
        };
        let now = Instant::now();
        let mut relay = Forwarder::new(Some(dir.path().into()), now);
        relay.current = vec!["first".into(), "second".into()];
        relay.enqueue(vec!["third".into()], now);
        relay.enqueue(vec!["fourth".into(), "fifth".into()], now);
        let (tx, rx) = std::sync::mpsc::channel();
        relay.inflight = Some(rx);
        tx.send(Ok(NativeForward::Primary(instance))).unwrap();
        let ForwardPoll::Primary {
            instance: _instance,
            paths,
        } = relay.poll(now)
        else {
            panic!("takeover")
        };
        assert_eq!(
            paths,
            ["first", "second", "third", "fourth", "fifth"].map(PathBuf::from)
        );
        assert!(relay.pending.is_empty());
    }
    #[test]
    fn route_matches_exact_roots_then_reuses_startup_and_respects_origin() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let mut windows = std::collections::BTreeMap::from([
            (
                1,
                WindowRoute {
                    folders: vec![root.join("another")],
                    ..WindowRoute::default()
                },
            ),
            (
                2,
                WindowRoute {
                    folders: vec![root.clone(), root.join("extra")],
                    ..WindowRoute::default()
                },
            ),
            (
                3,
                WindowRoute {
                    untouched: true,
                    ..WindowRoute::default()
                },
            ),
        ]);
        let mut request = native_request(&[dir.path().into()]).unwrap();
        let decision = |request: &Request, windows: &_, setting| {
            route(
                &RouteRequest::prepare(&mut request.clone(), setting),
                windows,
                Some(1),
            )
        };
        assert_eq!(
            decision(&request, &windows, OpenWindow::Never),
            Route::Window(3)
        );
        windows.get_mut(&2).unwrap().folders.pop();
        assert_eq!(
            decision(&request, &windows, OpenWindow::Never),
            Route::Window(2)
        );
        windows.get_mut(&2).unwrap().folders.clear();
        windows.get_mut(&3).unwrap().untouched = false;
        assert_eq!(decision(&request, &windows, OpenWindow::Never), Route::New);
        request.files[0].path = root.join("file.rs");
        assert_eq!(
            decision(&request, &windows, OpenWindow::FinderOnly),
            Route::New
        );
        request.native_open = false;
        assert_eq!(
            decision(&request, &windows, OpenWindow::FinderOnly),
            Route::Window(1)
        );
        assert_eq!(decision(&request, &windows, OpenWindow::Always), Route::New);
        request.add = true;
        assert_eq!(
            decision(&request, &windows, OpenWindow::Always),
            Route::Window(1)
        );
        request.files[0].path = root.join("p.sublime-project");
        windows.get_mut(&2).unwrap().project = Some(request.files[0].path.clone());
        assert_eq!(
            decision(&request, &windows, OpenWindow::Never),
            Route::Window(2)
        );
    }
    #[test]
    fn forwarder_without_events_quits_and_waits_for_inflight_and_later_batches() {
        let now = Instant::now();
        let mut relay = Forwarder::new(None, now);
        assert!(matches!(relay.poll(now), ForwardPoll::Waiting));
        assert!(matches!(
            relay.poll(now + Forwarder::QUIET),
            ForwardPoll::Quit
        ));
        let mut relay = Forwarder::new(None, now);
        let (tx, rx) = std::sync::mpsc::channel();
        relay.current = vec!["first".into()];
        relay.inflight = Some(rx);
        relay.enqueue(vec!["second".into()], now + Duration::from_secs(3));
        assert!(matches!(
            relay.poll(now + Duration::from_secs(30)),
            ForwardPoll::Waiting
        ));
        assert_eq!(relay.pending.len(), 1);
        tx.send(Ok(NativeForward::Forwarded)).unwrap();
        // Replace the next worker by a controlled acknowledgement to test quiet timing.
        relay.pending.clear();
        let received = now + Duration::from_secs(31);
        assert!(matches!(relay.poll(received), ForwardPoll::Waiting));
        relay.enqueue(vec!["late".into()], received + Duration::from_millis(500));
        assert_eq!(
            relay.pending.pop_front().unwrap(),
            vec![PathBuf::from("late")]
        );
        assert!(matches!(
            relay.poll(received + Duration::from_secs(1)),
            ForwardPoll::Waiting
        ));
        assert!(matches!(
            relay.poll(received + Duration::from_secs(2)),
            ForwardPoll::Quit
        ));
    }
    #[cfg(unix)]
    #[test]
    fn cli_batch_reports_all_errors_and_opens_later_files_and_new_paths() {
        use std::os::unix::net::UnixListener;
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.socket");
        let second = dir.path().join("second.socket");
        let _a = UnixListener::bind(&first).unwrap();
        let _b = UnixListener::bind(&second).unwrap();
        let good = dir.path().join("good.txt");
        let new = dir.path().join("new.txt");
        fs::write(&good, "good").unwrap();
        let mut w = Workspace::new(Text::new(), None, None, false).unwrap();
        let mut p = Panels::new(&w, false);
        let errors = open_locations(
            &mut w,
            &mut p,
            [first.clone(), good, second.clone(), new.clone()]
                .into_iter()
                .map(|path| FileLocation {
                    path,
                    line: 1,
                    column: 1,
                }),
        );
        assert_eq!(errors.len(), 2);
        assert!(errors[0].contains(&first.display().to_string()));
        assert!(errors[1].contains(&second.display().to_string()));
        assert_eq!(w.tabs.len(), 2);
        assert_eq!(
            w.tabs[w.active].path.as_ref(),
            Some(&dir.path().canonicalize().unwrap().join("new.txt"))
        );
    }
    #[test]
    fn successful_root_trash_detaches_and_persists_while_failed_paths_remain() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let keep = dir.path().join("keep");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&keep).unwrap();
        let project = dir.path().join("app.sublime-project");
        fs::write(
            &project,
            r#"{"folders":[{"path":"root"},{"path":"keep","custom":7}],"custom":42}"#,
        )
        .unwrap();
        let mut w = Workspace::new(Text::new(), None, Some(project.clone()), false).unwrap();
        let mut p = Panels::new(&w, false);
        let root = root.canonicalize().unwrap();
        fs::rename(&root, dir.path().join("trashed-root")).unwrap();
        apply_trashed(&mut w, &mut p, &[root]).unwrap();
        assert_eq!(w.folders.len(), 1);
        assert!(w.folders[0].path.ends_with("keep"));
        let saved = vel_session::project::Project::load(&project).unwrap();
        assert_eq!(saved.folders.len(), 1);
        assert_eq!(saved.folders[0].path, PathBuf::from("keep"));
        assert_eq!(saved.folders[0].extra["custom"], 7);
        assert_eq!(saved.extra["custom"], 42);
        assert_eq!(saved.folders[0].follow_symlinks, None);
    }
}

#[cfg(test)]
mod review15_tests {
    use super::*;
    use std::time::{Duration, Instant};
    #[test]
    fn explicit_project_and_directory_launches_reuse_the_matching_recovered_window() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("app.sublime-project");
        fs::write(&project, "{}").unwrap();
        let windows = vec![
            vel_session::Window::default(),
            vel_session::Window {
                project: Some(project.clone()),
                folders: vec![dir.path().into()],
                ..Default::default()
            },
        ];
        let session = vel_session::Session {
            windows,
            ..Default::default()
        };
        let mut options = Options {
            project: Some(project),
            ..Default::default()
        };
        assert_eq!(restored_window(&session, &options), Some(1));
        options.project = None;
        options.files.push(crate::cli::FileLocation {
            path: dir.path().into(),
            line: 1,
            column: 1,
        });
        assert_eq!(restored_window(&session, &options), Some(1));
        options.new_window = true;
        assert_eq!(restored_window(&session, &options), None);
        options.new_window = false;
        options.add_to_window = true;
        assert_eq!(restored_window(&session, &options), Some(0));
        options.add_to_window = false;
        options.project = Some(dir.path().join("other.sublime-project"));
        assert_eq!(restored_window(&session, &options), None);
    }
    #[cfg(unix)]
    #[test]
    fn recovery_matching_accepts_symlink_spellings() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("app.sublime-project"), "{}").unwrap();
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        let windows = vec![vel_session::Window {
            project: Some(root.join("app.sublime-project")),
            folders: vec![root],
            ..Default::default()
        }];
        let session = vel_session::Session {
            windows,
            ..Default::default()
        };
        let mut options = Options {
            project: Some(alias.join("app.sublime-project")),
            ..Default::default()
        };
        assert_eq!(restored_window(&session, &options), Some(0));
        options.project = None;
        options.files.push(crate::cli::FileLocation {
            path: alias,
            line: 1,
            column: 1,
        });
        assert_eq!(restored_window(&session, &options), Some(0));
    }
    #[test]
    fn relay_quiet_period_starts_after_the_last_request_and_ack_and_errors_name_paths() {
        let start = Instant::now();
        let mut relay = Forwarder::new(None, start);
        let late = start + Forwarder::QUIET - Duration::from_millis(1);
        relay.enqueue(vec!["first.txt".into(), "second.txt".into()], late);
        relay.current = relay.pending.pop_front().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        relay.inflight = Some(rx);
        assert!(matches!(
            relay.poll(start + Forwarder::QUIET),
            ForwardPoll::Waiting
        ));
        let ack = start + Duration::from_secs(3);
        tx.send(Ok(NativeForward::Forwarded)).unwrap();
        assert!(matches!(relay.poll(ack), ForwardPoll::Waiting));
        assert!(matches!(
            relay.poll(ack + Forwarder::QUIET - Duration::from_millis(1)),
            ForwardPoll::Waiting
        ));
        assert!(matches!(
            relay.poll(ack + Forwarder::QUIET),
            ForwardPoll::Quit
        ));
        relay.current = vec!["failed one.txt".into(), "failed two.txt".into()];
        let (tx, rx) = std::sync::mpsc::channel();
        relay.inflight = Some(rx);
        tx.send(Err("socket disconnected".into())).unwrap();
        let ForwardPoll::Failed(message) = relay.poll(ack) else {
            panic!("failure expected")
        };
        assert!(
            message.contains("failed one.txt")
                && message.contains("failed two.txt")
                && message.contains("socket disconnected")
        );
    }
}
