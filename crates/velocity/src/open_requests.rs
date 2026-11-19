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
    collections::VecDeque,
    fs,
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
};

/// Read-only image representation of a named binary document.
type ImageResult = Option<Arc<vel_render::scene::Image>>;
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
    path: PathBuf,
    cancelled: Arc<AtomicBool>,
    result: mpsc::SyncSender<ImageResult>,
}
#[derive(Default)]
struct ImageQueue {
    jobs: Mutex<VecDeque<ImageJob>>,
    ready: Condvar,
}
impl ImageQueue {
    fn submit(path: &Path) -> PendingImage {
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
            path: path.to_owned(),
            cancelled: cancelled.clone(),
            result: tx,
        });
        queue.ready.notify_one();
        PendingImage { cancelled, result }
    }

    fn run(&self) {
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
                let image = decode_image(&job.path, &job.cancelled);
                let _ = job.result.send(image);
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

#[derive(Debug, Default)]
pub(crate) struct ImagePreview {
    path: Option<PathBuf>,
    revision: u64,
    epoch: u64,
    pending: Option<PendingImage>,
    pub image: Option<Arc<vel_render::scene::Image>>,
    /// The file matched the image patterns but could not be decoded; its bytes are shown.
    pub failed: bool,
}
impl ImagePreview {
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn suspend(&mut self) {
        if self.pending.take().is_some() {
            // Returning to this tab should request a new decode.
            self.path = None;
        }
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
        let path = tab.path.as_deref().filter(|_| tab.editor.binary.is_some());
        if path.is_none() && self.path.is_none() {
            return false;
        }
        let revision = tab.editor.settings.revision();
        let epoch = tab.editor.revision;
        let mut changed = false;
        if (path, revision, epoch) != (self.path.as_deref(), self.revision, self.epoch) {
            self.path = path.map(Path::to_owned);
            self.revision = revision;
            self.epoch = epoch;
            changed = self.image.take().is_some() || self.failed;
            self.failed = false;
            self.pending = None;
            if let Some(path) = path {
                let root = folders
                    .iter()
                    .filter(|f| path.starts_with(&f.path))
                    .max_by_key(|f| f.path.as_os_str().len())
                    .map_or_else(|| path.parent().unwrap_or(path), |f| f.path.as_path());
                if image_matches(tab, root, path) {
                    self.pending = Some(ImageQueue::submit(path));
                }
            }
        }
        if let Some(pending) = &self.pending {
            match pending.result.try_recv() {
                Ok(image) => {
                    self.image = image;
                    self.failed = self.image.is_none();
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
struct CancelRead<'a, R> {
    input: R,
    cancelled: &'a AtomicBool,
}
impl<R> CancelRead<'_, R> {
    fn check(&self) -> io::Result<()> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(io::Error::other("image no longer visible"))
        } else {
            Ok(())
        }
    }
}
impl<R: Read> Read for CancelRead<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        self.input.read(buf)
    }
}
impl<R: Seek> Seek for CancelRead<'_, R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.check()?;
        self.input.seek(pos)
    }
}
impl<R: BufRead> BufRead for CancelRead<'_, R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.check()?;
        self.input.fill_buf()
    }
    fn consume(&mut self, amount: usize) {
        self.input.consume(amount);
    }
}

fn decode_image(path: &Path, cancelled: &AtomicBool) -> ImageResult {
    let file = vel_fileio::regular::read(path).ok()?;
    if file.metadata().ok()?.len() > 64 * 1024 * 1024 {
        return None;
    }
    let mut reader = image::ImageReader::new(CancelRead {
        input: BufReader::new(file),
        cancelled,
    })
    .with_guessed_format()
    .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    if cancelled.load(Ordering::Acquire) {
        return None;
    }
    let logical_size = [image.width() as f32, image.height() as f32];
    let image = if image.width().max(image.height()) > 2048 {
        image.thumbnail(2048, 2048)
    } else {
        image
    };
    let image = image.into_rgba8();
    if cancelled.load(Ordering::Acquire) {
        return None;
    }
    static NEXT: AtomicU64 = AtomicU64::new(1);
    Some(Arc::new(vel_render::scene::Image {
        logical_size,
        id: NEXT.fetch_add(1, Ordering::Relaxed) | (1 << 63),
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw().into(),
    }))
}

/// Chooses an already recovered window for an explicit project or directory launch.
/// An explicit new-window request always creates an additional window.
pub(crate) fn restored_window(windows: &[vel_session::Window], options: &Options) -> Option<usize> {
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
    (!windows.is_empty()).then_some(0)
}
fn same_path(a: &Path, b: &Path) -> bool {
    let normalized = crate::workspace::absolute;
    match (normalized(a), normalized(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
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
            files: paths
                .iter()
                .map(|path| FileLocation {
                    path: path.clone(),
                    line: 1,
                    column: 1,
                })
                .collect(),
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

/// Whether a request needs a new window. Directories use a new window unless --add
/// explicitly targets the current one, including native folders delivered by Finder.
pub fn needs_new_window(request: &Request, open_files_in_new_window: bool) -> bool {
    request.new_window
        || (!request.add
            && (open_files_in_new_window
                || request.files.iter().any(|location| location.path.is_dir())))
}
/// Includes the session policy in window routing so --project cannot overwrite dirty
/// buffers when Hot Exit is disabled. Menu switches instead use normal close prompts.
pub fn request_needs_window(workspace: &Workspace, request: &Request) -> bool {
    needs_new_window(
        request,
        workspace.global_bool("open_files_in_new_window", false),
    ) || (request.project.is_some()
        && !workspace.hot_exit().retain(false)
        && workspace.tabs.iter().any(|tab| tab.dirty()))
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
        .filter(|folder| paths.iter().any(|path| folder.path.starts_with(path)))
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
            return workspace.open(FileLocation {
                path: path.into(),
                line: 1,
                column: 1,
            });
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
        if let Some(index) = workspace
            .tabs
            .iter()
            .position(|tab| tab.path.as_ref() == Some(&path))
        {
            // A Finder/Dock request is explicit navigation even before OS focus
            // returns. Preserve this tab's current location when selecting duplicates.
            let _request = workspace.navigation_request();
            workspace.select(index);
            workspace.tabs[index].transient = false;
            panels.note_sidebar_spelling(workspace, spelling);
            return Ok(());
        }
        workspace
            .open(FileLocation {
                path,
                line: 1,
                column: 1,
            })
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
        assert_eq!((request.files[0].line, request.files[0].column), (1, 1));
    }
    #[test]
    fn before_window_queue_opens_files_folders_and_literal_colon_names_once_ready() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("你好:2:3");
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
        let Start::Primary(instance) = Instance::start(Some(dir.path()), &request).unwrap() else {
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
    fn directories_choose_new_windows_unless_explicitly_added() {
        let dir = tempfile::tempdir().unwrap();
        let mut request = native_request(&[dir.path().into()]).unwrap();
        assert!(needs_new_window(&request, false));
        request.add = true;
        assert!(!needs_new_window(&request, false));
        request.new_window = true;
        assert!(needs_new_window(&request, false));
        request.new_window = false;
        request.files.clear();
        assert!(!needs_new_window(&request, true));
        request.add = false;
        assert!(needs_new_window(&request, true));
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
        let mut options = Options {
            project: Some(project),
            ..Default::default()
        };
        assert_eq!(restored_window(&windows, &options), Some(1));
        options.project = None;
        options.files.push(crate::cli::FileLocation {
            path: dir.path().into(),
            line: 1,
            column: 1,
        });
        assert_eq!(restored_window(&windows, &options), Some(1));
        options.new_window = true;
        assert_eq!(restored_window(&windows, &options), None);
        options.new_window = false;
        options.add_to_window = true;
        assert_eq!(restored_window(&windows, &options), Some(0));
        options.add_to_window = false;
        options.project = Some(dir.path().join("other.sublime-project"));
        assert_eq!(restored_window(&windows, &options), None);
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
        let windows = [vel_session::Window {
            project: Some(root.join("app.sublime-project")),
            folders: vec![root],
            ..Default::default()
        }];
        let mut options = Options {
            project: Some(alias.join("app.sublime-project")),
            ..Default::default()
        };
        assert_eq!(restored_window(&windows, &options), Some(0));
        options.project = None;
        options.files.push(crate::cli::FileLocation {
            path: alias,
            line: 1,
            column: 1,
        });
        assert_eq!(restored_window(&windows, &options), Some(0));
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
