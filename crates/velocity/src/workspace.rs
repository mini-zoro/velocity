//! Headless tab/document lifecycle, settings resources and asynchronous file tasks.
use crate::{cli::FileLocation, editor::Editor};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    sync::atomic::Ordering,
    time::{Duration, Instant, SystemTime},
};
use vel_buffer::{Region, Selection};
use vel_config::{
    Value,
    json::Document,
    keymap::Keymap,
    resources::Resources,
    settings::{Layer, Settings},
    ui::{self, MenuItem, MouseBinding, PaletteItem},
};
use vel_fileio::{self as fileio, ChangeTracker, Format, LoadTask, Loaded, Watcher};
use vel_text::Text;
#[path = "appearance.rs"]
pub(crate) mod appearance;

/// The authored name of a macro; the default package needs no qualifier.
pub(crate) fn macro_name(caption: &str) -> &str {
    let name = caption.strip_prefix("Macro: ").unwrap_or(caption);
    name.strip_prefix("Default/").unwrap_or(name)
}

#[derive(Debug)]
pub struct Tab {
    pub shared_dirty: bool,
    /// The file was removed while its in-memory document remains open.
    pub deleted: bool,
    pub restored_format_dirty: bool,
    pub previous_document: Option<vel_buffer::BufferId>,
    pub restored_document: Option<u64>,
    pub disk_saved: Option<vel_session::DiskStamp>,
    pub(crate) disk_content: Option<fileio::ContentIdentity>,
    pub(crate) saved_in_effective_encoding: bool,
    pub(crate) legacy_stamp_matches: bool,
    pub(crate) legacy_saved_text: Option<Text>,
    pub(crate) legacy_text_fingerprint: Option<(usize, String)>,
    pub(crate) overwrite_allowed: Option<fileio::ContentIdentity>,
    pub sheet: u64,
    pub session_document: u64,
    pub document: vel_buffer::BufferId,
    pub shared_view: vel_buffer::ViewId,
    pub lazy: Option<(vel_session::Document, vel_session::ViewState)>,
    language_settings_revision: u64,
    appearance_revision: u64,
    pub(crate) needs_detection: bool,
    pub(crate) indentation_initialized: bool,
    pub transient: bool,
    /// A Goto preview is an editor adapter, not a member of any workbench group.
    pub(crate) goto_preview: bool,
    pub(crate) preview_recent_pending: bool,
    pub editor: Editor,
    pub path: Option<PathBuf>,
    pub(crate) find_results: bool,
    pub(crate) suggested_name: Option<String>,
    /// The display name stored with a lazily restored untitled sheet: shown until its
    /// text is materialized, then dropped so the stored text supplies the name.
    pub(crate) restored_name: Option<String>,
    pub format: Format,
    /// Explicit encoding commands set this; automatic detection never does.
    pub encoding_chosen: bool,
    /// Reload preference, retained when one disk revision needs automatic decoding.
    pub(crate) preferred_encoding: Option<String>,
    pub(crate) load_warning: Option<String>,
    pub(crate) restore_history: Option<(vel_session::History, bool)>,
    pub(crate) restored_history_views:
        std::collections::BTreeMap<vel_buffer::ViewId, vel_buffer::ViewId>,
    pub(crate) saved_format: Format,
    pub tracker: ChangeTracker,
    pub scroll: (f64, f64),
    pub scroll_line_height: f64,
    pub loading: Option<LoadTask>,
    pub binary_loading: Option<crate::binary::Task>,
    /// Binary identity survives worker completion/failure independently of its snapshot.
    pub(crate) binary_document: bool,
    /// An image file whose decode failed: its bytes are shown, so its label ends in `[hex]`.
    pub(crate) image_failed: bool,
    pub(crate) open_as_choices: Option<crate::open_requests::OpenAsChoices>,
    /// Whether the path matches `image_file_patterns`, with what that decision depends on.
    image_file: std::cell::RefCell<Option<ImageFile>>,
    /// The project folder the patterns match against; the file's own folder until a frame
    /// has supplied it.
    pub(crate) image_root: std::cell::RefCell<Option<PathBuf>>,
    /// The source could not load. Never persist an empty error placeholder as
    /// if it were recovered user text; retained dirty contents still survive.
    pub(crate) load_failed: bool,
    pub pending_restore: Option<vel_session::ViewState>,
    /// Center this location once the active view has its final viewport geometry.
    pub pending_center: bool,
    location: Option<FileLocation>,
    pub(crate) external_load: bool,
    load_started: Option<Instant>,
    pub(crate) disk_stamp: Option<(u64, SystemTime)>,
    /// Disk version sampled before the pending background load started reading.
    read_from: Option<DiskVersion>,
    /// Pending disk change owned by the real buffer, not by its shadow views.
    reload_deferred: bool,
    pub external: Option<Loaded>,
}
/// The disk version a load samples before it reads: the change-detection stamp and the
/// revision recorded for focus-save and sessions.
#[derive(Clone, Debug, PartialEq)]
struct DiskVersion {
    stamp: Option<(u64, SystemTime)>,
    saved: Option<vel_session::DiskStamp>,
}
impl DiskVersion {
    fn read(path: &Path) -> Self {
        Self {
            stamp: stamp(path),
            saved: vel_session::DiskStamp::read(path),
        }
    }
}
/// A remembered `image_file_patterns` decision and what it depends on.
#[derive(Debug, Clone)]
pub(crate) struct ImageFile {
    revision: u64,
    path: PathBuf,
    root: PathBuf,
    image: bool,
}
/// Only owned IO results cross the restore watchdog; editor state stays on its thread.
pub(crate) enum PreparedLoad {
    Text(Loaded),
    Loading(LoadTask),
    Binary(crate::binary::Task, String),
}
pub(crate) fn prepare_disk_load(
    path: &Path,
    options: fileio::DecodeOptions,
    background: bool,
    comparing: bool,
) -> Result<PreparedLoad, String> {
    prepare_disk_load_on(path, options, background, comparing, None)
}

fn prepare_disk_load_on(
    path: &Path,
    options: fileio::DecodeOptions,
    background: bool,
    comparing: bool,
    workers: Option<(&fileio::IoWorkers, usize)>,
) -> Result<PreparedLoad, String> {
    let (_, routing) = fileio::decoding_for_path(path, &options)?;
    if routing.attempts.is_empty()
        && let Some(notice) = &routing.binary
    {
        return Ok(PreparedLoad::Binary(
            crate::binary::Task::open(path.to_owned())?,
            notice.message(path),
        ));
    }
    if background {
        return Ok(PreparedLoad::Loading(fileio::load_on_workers(
            path.to_owned(),
            options,
            comparing,
            workers,
        )?));
    }
    match fileio::load_with_options(path, &options, |_| {}) {
        Ok(loaded) => Ok(PreparedLoad::Text(loaded)),
        Err(notice) if routing.binary.is_some() => Ok(PreparedLoad::Binary(
            crate::binary::Task::open(path.to_owned())?,
            notice,
        )),
        Err(error) => Err(error),
    }
}
impl Tab {
    pub(crate) fn restore_scroll(&mut self, state: &vel_session::ViewState) {
        // Before font metrics, main used this scaled 20 px grid plus logical padding.
        let settings = &self.editor.settings;
        let legacy_height = 20. * settings.number("font_size", 13.).clamp(6., 96.) / 13.
            + settings.number("line_padding_top", 0.).max(0.)
            + settings.number("line_padding_bottom", 0.).max(0.);
        let row = state.top_visual_line(legacy_height);
        self.scroll = (state.scroll.0, row * 20.);
        self.scroll_line_height = 20.;
        if let Some((_, saved)) = &mut self.lazy {
            // A lazy sheet can be saved again after zoom, before materialization.
            // Finish the wire migration now so its old pixels are never reinterpreted
            // using that later font size on the next startup.
            saved.scroll = self.scroll;
            saved.scroll_line = Some(row);
        }
    }
    pub(crate) fn new(text: Text, settings: &Settings, keymap: &Keymap) -> Result<Self, String> {
        Self::new_with_sheet(text, settings, keymap, crate::workbench::next_sheet())
    }
    fn new_with_sheet(
        text: Text,
        settings: &Settings,
        keymap: &Keymap,
        sheet: u64,
    ) -> Result<Self, String> {
        let mut editor = Editor::new(text);
        let tracker = ChangeTracker::new(&editor.buffer);
        editor.settings = settings.clone();
        editor.sync();
        editor.keymap = keymap.clone();
        let format = fileio::default_format(&settings.editor()).unwrap_or_default();
        let mut tab = Self {
            shared_dirty: false,
            deleted: false,
            restored_format_dirty: false,
            previous_document: None,
            restored_document: None,
            disk_saved: None,
            disk_content: None,
            saved_in_effective_encoding: false,
            legacy_stamp_matches: false,
            legacy_saved_text: None,
            legacy_text_fingerprint: None,
            overwrite_allowed: None,
            sheet,
            session_document: crate::workbench::next_sheet(),
            document: editor.buffer.id(),
            shared_view: editor.id,
            lazy: None,
            language_settings_revision: u64::MAX,
            appearance_revision: 0,
            needs_detection: true,
            indentation_initialized: false,
            transient: false,
            goto_preview: false,
            preview_recent_pending: false,
            editor,
            path: None,
            find_results: false,
            suggested_name: None,
            restored_name: None,
            encoding_chosen: false,
            preferred_encoding: None,
            load_warning: None,
            restore_history: None,
            restored_history_views: Default::default(),
            saved_format: format.clone(),
            format,
            tracker,
            scroll: (0., 0.),
            scroll_line_height: 20.,
            loading: None,
            binary_loading: None,
            binary_document: false,
            image_failed: false,
            open_as_choices: None,
            image_file: std::cell::RefCell::new(None),
            image_root: std::cell::RefCell::new(None),
            load_failed: false,
            pending_restore: None,
            pending_center: false,
            location: None,
            external_load: false,
            load_started: None,
            disk_stamp: None,
            read_from: None,
            reload_deferred: false,
            external: None,
        };
        crate::workbench::apply_window_layers(&mut tab, settings)?;
        Ok(tab)
    }
    pub fn install_restored(&mut self, loaded: Loaded) {
        // Restore already probed the path on its bounded reader. Installing editor
        // state must not repeat document IO on the startup thread.
        let path = self.path.take();
        self.install(loaded);
        self.path = path;
    }
    /// Move the document's worker with its actual buffer. Pending view restoration
    /// remains on each sheet so completion restores that sheet's own selection.
    pub(crate) fn transfer_document_io(&mut self, target: &mut Self) {
        target.loading = self.loading.take();
        target.read_from = self.read_from.take();
        target.binary_loading = self.binary_loading.take();
        target.previous_document = self.previous_document.take();
        target.external = self.external.take();
        target.external_load = std::mem::take(&mut self.external_load);
        target.reload_deferred = std::mem::take(&mut self.reload_deferred);
        target.load_started = self.load_started.take();
        target.location = self.location.take();
        target.load_warning = self.load_warning.take();
        target.restore_history = self.restore_history.take();
        target.restored_history_views = std::mem::take(&mut self.restored_history_views);
    }
    /// Reload/restore policy belongs to the document, rather than to detection.
    pub(crate) fn decode_options(&self) -> fileio::DecodeOptions {
        fileio::DecodeOptions {
            encoding: self.preferred_encoding.clone().or_else(|| {
                self.encoding_chosen.then(|| {
                    self.format
                        .display_name
                        .clone()
                        .unwrap_or_else(|| self.format.encoding.clone())
                })
            }),
            effective_encoding: Some(self.saved_format.encoding.clone()),
            saved_in_effective_encoding: self.saved_in_effective_encoding,
            saved_content: self.disk_content,
            revision_stamp: self
                .disk_saved
                .as_ref()
                .map(|stamp| (stamp.bytes, stamp.modified_ns)),
            legacy_format: Some(self.saved_format.clone()),
            legacy_stamp_matches: self.legacy_stamp_matches
                && self.path.as_deref().is_some_and(|path| {
                    self.disk_saved.is_some()
                        && vel_session::DiskStamp::read(path) == self.disk_saved
                }),
            legacy_saved_text: self.legacy_saved_text.clone(),
            legacy_text_fingerprint: self.legacy_text_fingerprint.clone(),
            default_line_ending: self.format.line_ending,
            ..fileio::DecodeOptions::from_settings(&self.editor.preferences)
        }
    }
    /// Includes removed files whose retained contents have no saved disk copy.
    pub fn dirty(&self) -> bool {
        if self.load_failed || self.find_results {
            return false;
        }
        self.deleted
            || self.shared_dirty
            || self.restored_format_dirty
            || self.lazy.as_ref().is_some_and(|(d, _)| d.dirty)
            || self.editor.buffer.is_dirty()
            || self.format != self.saved_format
    }
    /// Whether this document's path matches `image_file_patterns` (it is shown as a picture
    /// unless its decode fails). Remembered per settings revision and path (labels are painted
    /// often), matched against the project folder that `Workspace::sync_image_files` supplies,
    /// the same root the decoder uses; until then against the file's own folder.
    pub(crate) fn is_image_file(&self) -> bool {
        use crate::open_requests::OpenAs;
        if let Some(choice) = self.open_as() {
            return choice == OpenAs::Image;
        }
        let Some(path) = self.path.as_deref() else {
            return false;
        };
        let revision = self.editor.settings.revision();
        let owned = self.image_root.borrow().clone();
        // Project-relative `//` rules need the project folder, which a frame supplies; until
        // then the file's own folder cannot answer them, and a picture is shown as one.
        if owned.is_none() && self.has_project_image_patterns() {
            return true;
        }
        let root = owned
            .as_deref()
            .unwrap_or_else(|| path.parent().unwrap_or(path));
        if let Some(known) = self.image_file.borrow().as_ref()
            && known.revision == revision
            && known.path == path
            && known.root == root
        {
            return known.image;
        }
        self.classify_image_file(revision, path, root)
    }
    pub(crate) fn open_as(&self) -> Option<crate::open_requests::OpenAs> {
        self.open_as_choices
            .as_ref()?
            .borrow()
            .get(self.path.as_ref()?)
            .copied()
    }
    fn classify_image_file(&self, revision: u64, path: &Path, root: &Path) -> bool {
        let image = crate::open_requests::image_matches(self, root, path);
        *self.image_file.borrow_mut() = Some(ImageFile {
            revision,
            path: path.to_owned(),
            root: root.to_owned(),
            image,
        });
        image
    }
    /// Whether any `image_file_patterns` rule is relative to the project folder.
    fn has_project_image_patterns(&self) -> bool {
        self.editor
            .settings
            .get("image_file_patterns")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(|pattern| pattern.starts_with("//"))
    }
    /// Matches a byte document's path against `image_file_patterns` relative to its project
    /// folder (project-relative `//` rules need it), when anything it depends on changed.
    pub(crate) fn sync_image_file(&self, folders: &[vel_search::project::Folder]) {
        let Some(path) = self
            .path
            .as_deref()
            .filter(|_| self.editor.binary.is_some())
        else {
            return;
        };
        let revision = self.editor.settings.revision();
        let root = crate::open_requests::image_root(folders, path);
        *self.image_root.borrow_mut() = Some(root.to_owned());
        if self.image_file.borrow().as_ref().is_some_and(|known| {
            known.revision == revision && known.path == path && known.root == root
        }) {
            return;
        }
        self.classify_image_file(revision, path, root);
    }
    #[cfg(test)]
    pub fn label(&self) -> String {
        self.ui_label(&crate::ui_text::Chrome::new(
            &vel_i18n::Translator::default(),
        ))
    }
    pub fn ui_label(&self, ui: &crate::ui_text::Chrome) -> String {
        if let Some(title) = self
            .editor
            .settings
            .get("velocity_html_sheet_title")
            .and_then(Value::as_str)
        {
            return self.settings_title(ui, title);
        }
        let name = self.ui_name(ui);
        format!(
            "{name}{}{}{}{}",
            if self.editor.binary.is_some() && (self.image_failed || !self.is_image_file()) {
                &ui.hex_tab
            } else {
                ""
            },
            if self.path.is_some()
                && self.disk_saved.is_none()
                && self.lazy.is_none()
                && !self.deleted
                && !self.editor.read_only
            {
                &ui.new_file
            } else {
                ""
            },
            if self.deleted {
                ui.deleted.as_ref()
            } else {
                ""
            },
            if self.dirty() { " •" } else { "" }
        )
    }
    pub fn name(&self) -> String {
        self.name_with_untitled("Untitled")
    }
    pub fn ui_name(&self, ui: &crate::ui_text::Chrome) -> String {
        if self.find_results {
            return ui.find_results.to_string();
        }
        if let Some(title) = self
            .editor
            .settings
            .get("velocity_settings_label")
            .and_then(Value::as_str)
        {
            return self.settings_title(ui, title);
        }
        self.name_with_untitled(&ui.untitled)
    }
    fn settings_title(&self, ui: &crate::ui_text::Chrome, title: &str) -> String {
        if let Some(label) = self
            .editor
            .settings
            .get("velocity_settings_label")
            .and_then(Value::as_str)
        {
            let default = self
                .editor
                .settings
                .get("velocity_settings_resource")
                .is_some();
            let (suffix, template) = if default {
                (" (Default)", &ui.default_settings)
            } else {
                (" (User)", &ui.user_settings)
            };
            if let Some(name) = label.strip_suffix(suffix) {
                return vel_i18n::format_named(template, &[("name", name)]);
            }
        }
        title.into()
    }
    fn name_with_untitled(&self, untitled: &str) -> String {
        if self.path.is_none()
            && let Some(name) = &self.suggested_name
        {
            return name.clone();
        }
        if let Some(title) = self
            .editor
            .settings
            .get("velocity_html_sheet_title")
            .and_then(Value::as_str)
        {
            return title.into();
        }
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                if !self.editor.settings.bool("set_unsaved_view_name", true) {
                    return untitled.into();
                }
                if !self
                    .editor
                    .settings
                    .bool("set_unsaved_view_name_for_syntax", true)
                    && self
                        .editor
                        .language
                        .borrow()
                        .as_ref()
                        .is_some_and(|l| l.name != "Plain Text")
                {
                    return untitled.into();
                }
                // A sheet restored lazily still has no text: show the name it had when the
                // session was written instead of a placeholder until the tab is opened.
                if let Some(name) = &self.restored_name {
                    return name.clone();
                }
                let name: String = self
                    .editor
                    .text
                    .line(0)
                    .chars()
                    .skip_while(|c| c.is_whitespace())
                    .take(50)
                    .collect();
                let name = name.trim_end().to_owned();
                if name.is_empty() {
                    untitled.into()
                } else {
                    name
                }
            })
    }
    pub fn progress(&self) -> Option<(u64, u64)> {
        self.loading
            .as_ref()
            .map(|t| (t.read.load(Ordering::Relaxed), t.total))
            .or_else(|| self.binary_loading.as_ref().map(|t| (0, t.total)))
    }
    pub(crate) fn load_disk(
        &mut self,
        options: fileio::DecodeOptions,
        background: bool,
        external: bool,
        workers: Option<(&fileio::IoWorkers, usize)>,
    ) -> Result<(), String> {
        let path = self.path.clone().ok_or("load requires a saved file")?;
        // Sample the version before reading: a change made while the load reads must not
        // be acknowledged as the version that was read.
        let before = DiskVersion::read(&path);
        let prepared = prepare_disk_load_on(
            &path,
            options,
            background,
            external && !self.load_failed,
            workers,
        )?;
        let read = matches!(prepared, PreparedLoad::Text(_));
        self.install_disk_load(prepared, external);
        self.reload_deferred = false;
        if read {
            self.verify_read(before);
        } else if self.loading.is_some() {
            self.read_from = Some(before);
        }
        Ok(())
    }
    /// The file changed after `before` was sampled, so the completed read may predate
    /// the change or mix both versions, and the change notification arrived while the
    /// load was running. Keep the sampled version instead of acknowledging the current
    /// one (a pending conflict keeps the revision it was compared with), and check the
    /// file again on the next poll.
    fn verify_read(&mut self, before: DiskVersion) {
        let Some(path) = self.path.as_deref() else {
            return;
        };
        if DiskVersion::read(path) != before {
            if self.external.is_none() {
                self.disk_stamp = before.stamp;
                self.disk_saved = before.saved;
            }
            self.reload_deferred = true;
        }
    }

    fn defer_external_reload(&self, active_document: u64) -> bool {
        self.session_document != active_document
            && !self.editor.settings.bool("reload_file_in_background", true)
            && !self.dirty()
            && self.editor.marked.is_none()
    }
    pub(crate) fn install_disk_load(&mut self, prepared: PreparedLoad, external: bool) {
        self.read_from = None;
        match prepared {
            PreparedLoad::Text(loaded) => {
                if external {
                    self.accept_reload(loaded);
                } else {
                    self.install_restored(loaded);
                }
            }
            PreparedLoad::Loading(task) => {
                self.external_load = external;
                self.editor.source_bytes = task.total;
                self.loading = Some(task);
                self.editor.refresh_policy();
                self.load_started = Some(Instant::now());
                self.editor.read_only = !external || self.load_failed;
            }
            PreparedLoad::Binary(task, notice) => {
                self.load_warning = Some(notice);
                if external && (self.dirty() || self.editor.marked.is_some()) {
                    return;
                }
                self.loading = None;
                self.restore_history = None;
                self.binary_loading = Some(task);
                self.binary_document = true;
                self.editor.source_bytes = self.binary_loading.as_ref().map_or(0, |t| t.total);
                self.editor.refresh_policy();
                self.editor.read_only = true;
                self.external = None;
                self.external_load = false;
                self.load_failed = false;
            }
        }
    }
    fn load_hex(&mut self, notice: String, external: bool) -> Result<(), String> {
        self.load_warning = Some(notice);
        if external && (self.dirty() || self.editor.marked.is_some()) {
            // A binary rewrite cannot discard unsaved text. Keep edits visible;
            // explicit Reopen, or a clean restore, enters the streaming hex view.
            return Ok(());
        }
        self.loading = None;
        self.restore_history = None;
        self.binary_document = true;
        self.binary_loading = Some(crate::binary::Task::open(
            self.path.clone().ok_or("hex load requires a saved file")?,
        )?);
        self.editor.source_bytes = self.binary_loading.as_ref().map_or(0, |t| t.total);
        self.editor.refresh_policy();
        self.editor.read_only = true;
        self.external = None;
        self.external_load = false;
        self.load_failed = false;
        Ok(())
    }
    fn accept_reload(&mut self, loaded: Loaded) {
        if loaded.unchanged && !self.load_failed {
            self.unchanged_reload(loaded);
        } else if !self.load_failed
            && (self.dirty()
                || self.editor.marked.is_some()
                || !self.editor.settings.bool("reload_file_on_change", true))
        {
            self.external = Some(loaded);
        } else {
            self.install(loaded);
        }
    }
    pub(crate) fn failed_load(&mut self, error: String) {
        // Keep the revision/choice/history for retry. A failed read is neither a
        // decode result nor a new revision, and must not poison its identity.
        self.load_warning = Some(error);
        self.load_failed = true;
        self.editor.read_only = true;
        self.loading = None;
    }
    fn record_decode(&mut self, loaded: &mut Loaded) {
        if let Some(warning) = loaded.warning.take() {
            self.load_warning = Some(warning);
        }
    }
    fn unchanged_reload(&mut self, mut loaded: Loaded) {
        self.record_decode(&mut loaded);
        self.disk_content = loaded.disk_content;
        self.legacy_stamp_matches = false;
        self.legacy_saved_text = None;
        self.legacy_text_fingerprint = None;
        // A disk touch only acknowledges stamps. It does not replace live text,
        // selections, native history, pending format edits or decode provenance.
        self.disk_stamp = self.path.as_deref().and_then(stamp);
        self.disk_saved = self.path.as_deref().and_then(vel_session::DiskStamp::read);
    }
    fn install(&mut self, mut loaded: Loaded) {
        self.record_decode(&mut loaded);
        let navigation = self.editor.navigation.take();
        self.editor.binary = None;
        self.binary_document = false;
        self.load_failed = false;
        self.editor.source_bytes = self.loading.as_ref().map_or(0, |t| t.total);
        self.editor.cancel_composition();
        *self.editor.html.borrow_mut() = Default::default();
        let old = self.editor.buffer.view(self.editor.id).sel().clone();
        let positions: Vec<_> = old
            .iter()
            .map(|r| {
                let p = self.editor.text.byte_to_point(r.b);
                (p.row, p.column)
            })
            .collect();
        self.editor.annotations = std::rc::Rc::new(std::cell::RefCell::new(
            crate::annotations::Annotations::new(loaded.text.clone()),
        ));
        self.editor.buffer = vel_buffer::Buffer::new(loaded.text.clone());
        self.editor.id = self.editor.buffer.new_view();
        let mut imported_history = false;
        if self.loading.is_none()
            && let Some((history, keep)) = self.restore_history.take()
        {
            let history = history.with_current_text(loaded.text.clone());
            match history
                .restore_views(keep || loaded.unchanged, false)
                .or_else(|_| history.restore_views(false, false))
            {
                Ok((mut buffer, views)) => {
                    self.editor.id = self
                        .pending_restore
                        .as_ref()
                        .and_then(|state| state.history_view)
                        .and_then(|old| views.get(&old))
                        .copied()
                        // A clone created from a failed-load placeholder owns no
                        // stored history view; it must not steal a sibling's view.
                        .unwrap_or_else(|| buffer.new_view());
                    self.editor.buffer = buffer;
                    self.restored_history_views = views;
                    imported_history = true;
                }
                Err(error) => self.load_warning = Some(error),
            }
        }
        self.editor.read_only = false;
        self.editor.sync();
        self.previous_document = Some(self.document);
        self.document = self.editor.buffer.id();
        self.shared_view = self.editor.id;
        self.needs_detection = !self.indentation_initialized;
        self.saved_format = loaded.format.clone();
        // Prefix snapshots retain the baseline. A terminal failed/synthetic
        // install has no disk identity: keeping the previous digest would make
        // recovery of those bytes a false touch and preserve the blank buffer.
        if loaded.disk_content.is_some() || self.loading.is_none() {
            self.disk_content = loaded.disk_content;
            self.saved_in_effective_encoding = false;
            self.legacy_stamp_matches = false;
            self.legacy_saved_text = None;
            self.legacy_text_fingerprint = None;
            self.load_failed = false;
        }
        self.format = loaded.format;
        self.tracker.mark_saved(&self.editor.buffer);
        let regions = positions.into_iter().map(|(row, column)| {
            Region::caret(
                loaded
                    .text
                    .point_to_byte(loaded.text.clip_point(vel_text::Point { row, column })),
            )
        });
        if !imported_history {
            self.editor.set_selection(Selection::from_regions(regions));
        }
        if self.loading.is_none() {
            if let Some(state) = self.pending_restore.take() {
                state.apply(&mut self.editor.buffer, self.editor.id);
                self.editor
                    .restore_folded_view_state(&state, self.editor.id);
                self.editor.sync();
            }
            self.editor.measure_document();
        }
        self.disk_stamp = self.path.as_deref().and_then(stamp);
        self.disk_saved = self.path.as_deref().and_then(vel_session::DiskStamp::read);
        self.overwrite_allowed = None;
        if let Some(location) = self.location.take() {
            self.goto(&location);
        }
        self.editor.navigation = navigation;
        if let Some(observer) = &self.editor.navigation
            && let Some(jump) = self.editor.navigation_snapshot()
        {
            observer.history.borrow_mut().installed(jump);
        }
    }
    pub fn goto(&mut self, location: &FileLocation) {
        let text = self.editor.buffer.text();
        let row = location.line.saturating_sub(1).min(text.len_lines() - 1);
        let start = text.line_to_byte(row);
        let end = start + crate::layout::body_len(text, row);
        let chars = text
            .byte_to_char(start)
            .saturating_add(location.column.saturating_sub(1));
        let offset = text.char_to_byte(chars.min(text.len_chars())).min(end) - start;
        self.editor.set_caret(start + offset, false);
        self.pending_center = true;
    }
}
#[derive(Debug)]
pub struct Workspace {
    io_workers: fileio::IoWorkers,
    /// A workspace sheet with its own input and drawing, independent of document tabs.
    pub(crate) settings_page: Option<Box<crate::settings_ui::Page>>,
    pending_revert: Option<PendingRevert>,
    pub(crate) jump_history: crate::editor::navigation::Shared,
    pub(crate) appearance: appearance::Catalog,
    pub(crate) appearance_preview: Option<appearance::Preview>,
    appearance_commits: Vec<appearance::PendingCommit>,
    pub(crate) appearance_notice: Option<String>,
    appearance_revision: u64,
    #[cfg(test)]
    pub(crate) appearance_writes: Arc<std::sync::atomic::AtomicUsize>,
    pub themes: crate::theme_ui::Themes,
    pub git: crate::git_ui::GitUi,
    pub catalog: Arc<crate::completion::Catalog>,
    pub assets: Arc<crate::language::Assets>,
    pub(crate) syntax_settings: crate::shipped::SyntaxSettings,
    schemes: BTreeMap<String, Arc<vel_color_scheme::ColorScheme>>,
    pub dark: bool,
    pub workbench: vel_ui::workbench::Workbench,
    pub distraction_free: bool,
    pub(crate) window_layers: crate::workbench::WindowLayers,
    pub fullscreen: bool,
    pub build_systems: crate::shipped::Builds,
    pub build_system: Option<String>,
    pub project_model: vel_session::project::Project,
    pub recent_files: Vec<PathBuf>,
    pub(crate) goto_recent_files: Vec<PathBuf>,
    pub recent_folders: Vec<PathBuf>,
    pub recent_projects: Vec<PathBuf>,
    recent_menu_state: Option<([Vec<PathBuf>; 3], u64)>,
    pub(crate) distraction_settings: serde_json::Map<String, Value>,
    pub folders: Vec<vel_search::project::Folder>,
    pub(crate) transient_folders: std::collections::BTreeSet<PathBuf>,
    pub tabs: Vec<Tab>,
    pub(crate) closed_sheet_generation: u64,
    pub active: usize,
    /// An empty window retains an input adapter but has no workbench sheet.
    pub(crate) empty: bool,
    pub(crate) closed_files: Vec<(PathBuf, vel_session::ViewState)>,
    pub(crate) open_as_choices: crate::open_requests::OpenAsChoices,
    /// Final stdin text is retained until its CLI waiter consumes it.
    pub(crate) closed_text: BTreeMap<u64, Text>,
    pub(crate) stdout_sheets: std::collections::BTreeSet<u64>,
    pub menus: Vec<MenuItem>,
    /// Resource-layered native context menus, keyed by their complete resource filename.
    pub context_menus: BTreeMap<String, Vec<MenuItem>>,
    pub(crate) widget_context_menu: String,
    pub mousemap: Vec<MouseBinding>,
    pub scrollbar_mousemap: Vec<MouseBinding>,
    pub palette: Vec<PaletteItem>,
    startup_palette: Option<PaletteSeed>,
    pub(crate) translations: vel_i18n::Translator,
    preferred_languages: crate::ui_language::PreferredLanguages,
    unsupported_ui_language: Option<String>,
    pub(crate) ui_language_notice: Option<String>,
    pub(crate) ui: crate::ui_text::Chrome,
    pub(crate) menu_revision: u64,
    pub(crate) palette_revision: u64,
    /// Macro filenames from the background resource overlay; contents are loaded on demand.
    pub(crate) macro_paths: std::collections::BTreeSet<String>,
    pub(crate) resources: Resources,
    pub messages: Vec<String>,
    /// File load/reload diagnostics, consumed by status text and never modal alerts.
    pub(crate) load_notices: Vec<String>,
    /// Consolidated authoring diagnostics; consumed by the status bar, never alerts.
    pub(crate) resource_reports: Vec<String>,
    ignored_user_keymaps: IgnoredKeymaps,
    pub(crate) settings_reports: crate::settings_reports::Scope,
    pub(crate) restore_in_progress: bool,
    pub(crate) restore_source: Option<PathBuf>,
    pub(crate) restore_deadline: Option<Instant>,
    pub(crate) pane_extent: (f64, f64),
    pub(crate) pane_minimum: (f64, f64),
    pub(crate) restore_original: Option<Arc<crate::session_restore::Original>>,
    pub telemetry: Vec<Value>,
    /// Whether measurement records are wanted: unknown (`None`) until the owning App
    /// applies its options, so startup opens are still recorded for automatic modes.
    pub telemetry_enabled: Option<bool>,
    pub(crate) settings: Settings,
    pub(crate) keymap: Keymap,
    pub(crate) watcher: Option<Watcher>,
    /// Only set for the original interactive window, not benchmarks or later windows.
    startup_watcher: bool,
    packed_syntaxes: bool,
    pub(crate) data_dir: Option<PathBuf>,
    pub(crate) project: Option<PathBuf>,
    config_stamp: ConfigStamps,
    config_rx: Option<std::sync::mpsc::Receiver<ConfigLoad>>,
    last_config_check: Option<Instant>,
    reload_requested: bool,
    settings_writes: Vec<(std::sync::mpsc::Receiver<Result<(), String>>, u64)>,
    settings_write_generation: u64,
    config_write_generation: (u64, bool),
    settings_write_overrides: BTreeMap<String, (u64, Option<Value>)>,
    settings_reload_deferred: bool,
    pub resource_revision: u64,
}
#[derive(Debug)]
struct PendingRevert {
    sheet: u64,
    path: PathBuf,
    revision: u64,
    format: fileio::Format,
    task: LoadTask,
}
pub(crate) fn stamp(path: &Path) -> Option<(u64, SystemTime)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()?))
}
fn count_unknown_menu(items: &[MenuItem], known: &impl Fn(&str) -> bool) -> usize {
    items
        .iter()
        .map(|item| {
            usize::from(item.command.as_deref().is_some_and(|name| !known(name)))
                + count_unknown_menu(&item.children, known)
        })
        .sum()
}

fn filter_shipped_menu(items: &mut Vec<MenuItem>, known: &impl Fn(&str) -> bool) {
    items.retain_mut(|item| {
        if item.command.as_deref().is_some_and(|name| !known(name)) {
            return false;
        }
        let submenu = !item.children.is_empty();
        filter_shipped_menu(&mut item.children, known);
        !submenu || !item.children.is_empty()
    });
}

pub(crate) fn syntax_setting_key(path: &str) -> &str {
    let file = path.rsplit('/').next().unwrap_or(path);
    [".sublime-syntax", ".tmLanguage.json", ".tmLanguage"]
        .into_iter()
        .find_map(|suffix| file.strip_suffix(suffix))
        .unwrap_or(file)
}

fn extension_syntax<'a>(
    syntaxes: &'a vel_syntax::SyntaxSet,
    settings: &crate::shipped::SyntaxSettings,
    path: Option<&Path>,
) -> Option<&'a str> {
    let name = path?.file_name()?.to_str()?;
    let (_, suffixes) = name.split_once('.')?;
    syntaxes
        .syntaxes()
        .iter()
        .filter_map(|syntax| {
            settings
                .extensions(&syntax.path, &syntax.name)
                .iter()
                .filter(|ext| {
                    suffixes.eq_ignore_ascii_case(ext)
                        || suffixes
                            .match_indices('.')
                            .any(|(offset, _)| suffixes[offset + 1..].eq_ignore_ascii_case(ext))
                })
                .map(|ext| (ext.len(), syntax.path.as_str()))
                .max_by_key(|(len, _)| *len)
        })
        .max_by_key(|(len, _)| *len)
        .map(|(_, syntax)| syntax)
}

/// One document identity for opens, Save As and restored paths. New filenames
/// also resolve their existing parent, including directory symlinks.
pub(crate) fn absolute(path: &Path) -> Result<PathBuf, String> {
    if std::fs::metadata(path).is_ok_and(|m| m.is_file() || m.is_dir()) {
        return std::fs::canonicalize(path).map_err(|e| e.to_string());
    }
    #[cfg(windows)]
    let absolute = std::path::absolute(path).map_err(|e| e.to_string())?;
    #[cfg(not(windows))]
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map(|p| p.join(path))
            .map_err(|e| e.to_string())?
    };
    if let Some(parent) = absolute.parent()
        && let Some(name) = absolute.file_name()
        && parent.exists()
    {
        Ok(parent.canonicalize().map_err(|e| e.to_string())?.join(name))
    } else {
        Ok(absolute)
    }
}

pub(crate) fn absolute_folder(path: &Path) -> Result<PathBuf, String> {
    let path = std::path::absolute(path).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    for ancestor in path.ancestors() {
        // Keep the folder, walker and tab keys in the same namespace. A root
        // may already be gone when persisting a successful Trash operation.
        if let Ok(canonical) = std::fs::canonicalize(ancestor) {
            return Ok(canonical.join(path.strip_prefix(ancestor).expect("path ancestor")));
        }
    }
    // Normalize system aliases to agree with document paths and FSEvents,
    // but keep user-selected components (including symlinks) below them.
    #[cfg(target_os = "macos")]
    for alias in ["/tmp", "/var", "/etc"] {
        if let Ok(suffix) = path.strip_prefix(alias)
            && let Ok(base) = std::fs::canonicalize(alias)
        {
            return Ok(base.join(suffix));
        }
    }
    Ok(path)
}
#[derive(Debug)]
struct Configuration {
    appearance: appearance::Catalog,
    themes: crate::theme_ui::Prepared,
    resources: Resources,
    settings: Settings,
    keymap: Keymap,
    syntax_settings: crate::shipped::SyntaxSettings,
    schemes: BTreeMap<String, Arc<vel_color_scheme::ColorScheme>>,
    assets: Arc<crate::language::Assets>,
    catalog: Arc<crate::completion::Catalog>,
    menus: Vec<MenuItem>,
    context_menus: BTreeMap<String, Vec<MenuItem>>,
    widget_context_menu: String,
    mousemap: Vec<MouseBinding>,
    scrollbar_mousemap: Vec<MouseBinding>,
    palette: Vec<PaletteItem>,
    startup_palette: Option<PaletteSeed>,
    translations: vel_i18n::Catalogs,
    macro_paths: std::collections::BTreeSet<String>,
    folders: Vec<vel_search::project::Folder>,
    distraction_settings: serde_json::Map<String, Value>,
    build_systems: crate::shipped::Builds,
    project_model: vel_session::project::Project,
    messages: Vec<String>,
    resource_reports: Vec<String>,
    ignored_user_keymaps: IgnoredKeymaps,
}

#[derive(Debug)]
struct PaletteSeed {
    // Snapshot the authored menu, before native recent-file rows are installed.
    menus: Vec<MenuItem>,
    extra: Vec<PaletteItem>,
    translations: vel_i18n::Catalogs,
}
fn load_palette(
    resources: &Resources,
    menus: &[MenuItem],
    extra: Vec<PaletteItem>,
    translations: &vel_i18n::Catalogs,
    assets: &crate::language::Assets,
    catalog: &crate::completion::Catalog,
    messages: &mut Vec<String>,
) -> Vec<PaletteItem> {
    let mut palette = Vec::new();
    let registry = Editor::command_registry();
    for resource in resources.matching(".sublime-commands") {
        match resource.text().and_then(ui::parse::<PaletteItem>) {
            Ok(mut items) => {
                #[cfg(windows)]
                if resource.path == "Packages/Default/Velocity.sublime-commands" {
                    items.retain(|item| {
                        !matches!(
                            item.invocation.command.as_str(),
                            "enable_finder_new_file" | "disable_finder_new_file"
                        )
                    });
                    for item in &mut items {
                        if item.invocation.command == "edit_settings" {
                            use_windows_keymap_paths(&mut item.invocation.args);
                        }
                    }
                }
                if resource.shipped() && !resource.path.starts_with("Packages/Default/") {
                    items.retain(|item| {
                        let name = &item.invocation.command;
                        name == "noop" || registry.kind(name).is_some()
                    });
                }
                palette.extend(items);
            }
            Err(e) => messages.push(format!("{}: {e}", resource.path)),
        }
    }
    palette.extend(extra);
    crate::ui_language::palette_rows(&mut palette, translations);
    ui::extend_palette(&mut palette, menus);
    for syntax in assets.syntaxes.syntaxes().iter().filter(|s| !s.hidden) {
        palette.push(PaletteItem {
            caption: format!("Set Syntax: {}", syntax.name),
            invocation: vel_config::keymap::Invocation {
                command: "set_file_type".into(),
                args: serde_json::json!({"syntax": syntax.path}),
            },
        });
    }
    for (name, snippet) in catalog.snippets.iter() {
        palette.push(PaletteItem {
            caption: format!(
                "Snippet: {}",
                if snippet.description().is_empty() {
                    snippet_title(name)
                } else {
                    snippet.description()
                }
            ),
            invocation: vel_config::keymap::Invocation {
                command: "insert_snippet".into(),
                args: serde_json::json!({"name":name}),
            },
        });
    }
    palette
}

#[derive(Clone, Copy, Default)]
enum ConfigKind {
    #[default]
    Eager,
    FirstFrame,
    InteractiveReload,
}

type ConfigStamps = Vec<(PathBuf, Option<(u64, SystemTime)>)>;

#[derive(Debug)]
struct ConfigLoad {
    result: Result<Configuration, String>,
    stamps: ConfigStamps,
}

#[derive(Debug, PartialEq, Eq)]
struct IgnoredKeymap {
    origin: PathBuf,
    stamp: Option<(u64, SystemTime)>,
}

type IgnoredKeymaps = BTreeMap<String, IgnoredKeymap>;

fn report_ignored_keymaps(old: &IgnoredKeymaps, new: &IgnoredKeymaps, reports: &mut Vec<String>) {
    reports.extend(
        new.iter()
            .filter(|(name, file)| old.get(*name) != Some(*file))
            .map(|(name, _)| format!("Ignored {name}; use Packages/User/{PLATFORM_KEYMAP}")),
    );
}

const PLATFORM_KEYMAP: &str = if cfg!(windows) {
    "Default (Windows).sublime-keymap"
} else {
    "Default (OSX).sublime-keymap"
};

#[cfg(windows)]
pub(crate) fn use_windows_keymap_paths(args: &mut Value) {
    if args["base_file"] == "${packages}/Default/Velocity.sublime-keymap"
        && args["user_file"] == "${packages}/User/Default (OSX).sublime-keymap"
    {
        args["base_file"] = format!("${{packages}}/Default/{PLATFORM_KEYMAP}").into();
        args["user_file"] = format!("${{packages}}/User/{PLATFORM_KEYMAP}").into();
    }
}

#[cfg(windows)]
fn adapt_windows_menu(items: &mut [MenuItem]) {
    for item in items {
        if item.command.as_deref() == Some("edit_settings") {
            use_windows_keymap_paths(&mut item.args);
        }
        adapt_windows_menu(&mut item.children);
    }
}
// Diagnostics retain their resource paths; display names never establish provenance.
fn user_resource(resource: &vel_config::resources::Resource, data: Option<&Path>) -> bool {
    data.is_some_and(|data| {
        resource.origin.starts_with(data.join("Packages"))
            || resource.origin.starts_with(data.join("Installed Packages"))
    })
}
fn warning_from_path(warning: &str, path: &str) -> bool {
    warning
        .strip_prefix(path)
        .is_some_and(|rest| rest.starts_with(':'))
        || warning.starts_with(&format!("skipping {path:?}:"))
}
fn collect_resource_warnings<'a, S: AsRef<str>>(
    kind: &str,
    warnings: impl IntoIterator<Item = (Option<&'a str>, S)>,
    reports: &mut Vec<String>,
    log: &mut Vec<String>,
) {
    let mut count = 0;
    let mut first = None;
    for (path, warning) in warnings {
        eprintln!("{}", warning.as_ref());
        log.push(warning.as_ref().to_owned());
        if let Some(path) = path {
            first.get_or_insert(path);
            count += 1;
        }
    }
    if let Some(first) = first {
        reports.push(format!(
            "{count} {kind} warnings ({first}), see Velocity.log"
        ));
    }
}
/// Size after which `Velocity.log` is rotated to `Velocity.log.1`.
const RESOURCE_LOG_LIMIT: u64 = 1 << 20;
fn write_resource_log(data: Option<&Path>, warnings: &[String], reports: &mut Vec<String>) {
    use std::io::Write;
    if warnings.is_empty() {
        return;
    }
    if let Some(data) = data {
        let path = data.join("Velocity.log");
        let result = (|| {
            std::fs::create_dir_all(data)?;
            // The same warnings are logged on every configuration load, so keep the file bounded:
            // past the limit the current log becomes `Velocity.log.1` (replacing an older one).
            if std::fs::metadata(&path).is_ok_and(|m| m.len() > RESOURCE_LOG_LIMIT) {
                std::fs::rename(&path, data.join("Velocity.log.1"))?;
            }
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)?;
            let seconds = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            file.write_all(
                format!(
                    "[{seconds}] {} resource warning(s)\n{}\n",
                    warnings.len(),
                    warnings.join("\n")
                )
                .as_bytes(),
            )
        })();
        if let Err(error) = result {
            eprintln!("{}: {error}", path.display());
            if !reports.is_empty() {
                // Even an unwritable profile must not send these through the alert queue.
                reports.push(format!(
                    "Could not write {}: {error}; {}",
                    path.display(),
                    warnings.join("; ")
                ));
            }
        }
    }
}
/// The palette title of a snippet without a description: its file name without
/// the `.sublime-snippet` extension.
fn snippet_title(name: &str) -> &str {
    let file = name.rsplit('/').next().unwrap_or(name);
    file.strip_suffix(".sublime-snippet").unwrap_or(file)
}

#[cfg(test)]
thread_local! {
    static AFTER_USER_RESOURCE_READ: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

impl Configuration {
    #[cfg(test)]
    fn load(data_dir: Option<PathBuf>, project: Option<PathBuf>) -> Result<Self, String> {
        Self::load_with_window_overrides(
            data_dir,
            project,
            Some(Default::default()),
            Default::default(),
            ConfigKind::Eager,
            &mut Default::default(),
        )
        .result
    }

    fn load_with_window_overrides(
        data_dir: Option<PathBuf>,
        project: Option<PathBuf>,
        window_overrides: Option<serde_json::Map<String, Value>>,
        preferred_languages: crate::ui_language::PreferredLanguages,
        kind: ConfigKind,
        phases: &mut crate::report::StartupPhases,
    ) -> ConfigLoad {
        let mut stamps = Vec::new();
        let result = Self::read(
            data_dir,
            project,
            window_overrides,
            preferred_languages,
            &mut stamps,
            kind,
            phases,
        );
        stamps.sort_by(|a, b| a.0.cmp(&b.0));
        stamps.dedup_by(|a, b| a.0 == b.0);
        ConfigLoad { result, stamps }
    }

    fn read(
        data_dir: Option<PathBuf>,
        project: Option<PathBuf>,
        window_overrides: Option<serde_json::Map<String, Value>>,
        preferred_languages: crate::ui_language::PreferredLanguages,
        stamps: &mut ConfigStamps,
        kind: ConfigKind,
        phases: &mut crate::report::StartupPhases,
    ) -> Result<Self, String> {
        if let Some(path) = &project {
            stamps.push((path.clone(), stamp(path)));
        }
        let mut resources = Resources::default();
        crate::theme_ui::bundled(&mut resources);
        #[cfg(windows)]
        let platform_preferences = "Preferences (Windows).sublime-settings";
        #[cfg(not(windows))]
        let platform_preferences = "Preferences (OSX).sublime-settings";
        for (name, source) in [
            (
                "Distraction Free.sublime-settings",
                include_str!("../resources/Packages/Default/Distraction Free.sublime-settings"),
            ),
            (
                "Rust Function.sublime-snippet",
                include_str!("../resources/Packages/Default/Rust Function.sublime-snippet"),
            ),
            (
                "Preferences.sublime-settings",
                include_str!("../resources/Packages/Default/Preferences.sublime-settings"),
            ),
            #[cfg(not(windows))]
            (
                platform_preferences,
                include_str!("../resources/Packages/Default/Preferences (OSX).sublime-settings"),
            ),
            #[cfg(windows)]
            (
                platform_preferences,
                include_str!(
                    "../resources/Packages/Default/Preferences (Windows).sublime-settings"
                ),
            ),
            #[cfg(not(windows))]
            (
                "Velocity.sublime-keymap",
                include_str!("../resources/Packages/Default/Velocity.sublime-keymap"),
            ),
            #[cfg(windows)]
            (
                PLATFORM_KEYMAP,
                include_str!("../resources/Packages/Default/Default (Windows).sublime-keymap"),
            ),
            (
                "Velocity Vintage.sublime-keymap",
                include_str!("../resources/Packages/Default/Velocity Vintage.sublime-keymap"),
            ),
            (
                "Main.sublime-menu",
                include_str!("../resources/Packages/Default/Main.sublime-menu"),
            ),
            (
                "Context.sublime-menu",
                include_str!("../resources/Packages/Default/Context.sublime-menu"),
            ),
            (
                "Tab Context.sublime-menu",
                include_str!("../resources/Packages/Default/Tab Context.sublime-menu"),
            ),
            (
                "Side Bar.sublime-menu",
                include_str!("../resources/Packages/Default/Side Bar.sublime-menu"),
            ),
            (
                "Side Bar Mount Point.sublime-menu",
                include_str!("../resources/Packages/Default/Side Bar Mount Point.sublime-menu"),
            ),
            (
                "Widget Context.sublime-menu",
                include_str!("../resources/Packages/Default/Widget Context.sublime-menu"),
            ),
            (
                "Find in Files.sublime-menu",
                include_str!("../resources/Packages/Default/Find in Files.sublime-menu"),
            ),
            (
                "Velocity.sublime-mousemap",
                include_str!("../resources/Packages/Default/Velocity.sublime-mousemap"),
            ),
            (
                "Velocity.sublime-commands",
                include_str!("../resources/Packages/Default/Velocity.sublime-commands"),
            ),
        ] {
            resources.insert(
                &format!("Packages/Default/{name}"),
                source.as_bytes().to_vec(),
                PathBuf::from("<embedded>"),
            );
        }
        for (name, source) in [
            (
                "Velocity Dark.sublime-color-scheme",
                include_str!(
                    "../resources/Packages/Color Scheme - Velocity/Velocity Dark.sublime-color-scheme"
                ),
            ),
            (
                "Velocity Light.sublime-color-scheme",
                include_str!(
                    "../resources/Packages/Color Scheme - Velocity/Velocity Light.sublime-color-scheme"
                ),
            ),
        ] {
            resources.insert(
                &format!("Packages/Color Scheme - Velocity/{name}"),
                source.as_bytes().to_vec(),
                PathBuf::from("<embedded>"),
            );
        }
        for &(path, bytes) in crate::ui_language::SHIPPED {
            resources.insert_embedded(path, bytes, PathBuf::from("<embedded>"));
        }
        let mut resource_messages = Vec::new();
        let mut resource_reports = Vec::new();
        let mut mouse_warnings = Vec::new();
        let mut scheme_warnings = Vec::new();
        let mut resource_log = Vec::new();
        crate::shipped::register(&crate::language::packages_root(), &mut resources);
        if let Some(bundle) = crate::release::bundle_resources() {
            collect_resource_warnings(
                "package",
                resources
                    .load_with_warnings(&bundle)
                    .iter()
                    .map(|warning| (None, warning)),
                &mut resource_reports,
                &mut resource_log,
            );
        }
        if let Some(data) = &data_dir {
            // Initialize a fresh profile before watching it. Observe inputs
            // before reading them: sampling at the end can associate old
            // bytes with the stamp of a concurrent atomic replacement.
            let _ = std::fs::create_dir_all(data.join("Packages/User"));
            #[cfg(windows)]
            for name in ["Preferences.sublime-settings", platform_preferences] {
                // A same-tick sibling rename may leave the directory stamp
                // unchanged. Observe these paths even before their first save.
                let path = data.join("Packages/User").join(name);
                stamps.push((path.clone(), stamp(&path)));
            }
            let (mut settings_reports, warnings): (Vec<_>, Vec<_>) = resources
                .load_with_warnings_observing(data, |path| {
                    stamps.push((path.to_owned(), stamp(path)));
                })
                .into_iter()
                .partition(|w| crate::settings_reports::source(w).is_some());
            #[cfg(test)]
            if let Some(after_read) = AFTER_USER_RESOURCE_READ.with(|hook| hook.borrow_mut().take())
            {
                after_read();
            }
            // Retain each settings filename and location for the non-modal report.
            resource_log.extend(settings_reports.iter().cloned());
            resource_reports.append(&mut settings_reports);
            let paths: Vec<_> = warnings
                .iter()
                .map(|w| w.split_once(": ").map_or(w.as_str(), |(p, _)| p))
                .collect();
            collect_resource_warnings(
                "package",
                warnings.iter().zip(paths).map(|(w, p)| (Some(p), w)),
                &mut resource_reports,
                &mut resource_log,
            );
        }
        // Resolve ignored_packages before loading any resource category. Reading
        // the preferences first is necessary to allow re-enabling a package.
        let mut ignored = std::collections::BTreeSet::new();
        for resource in resources.matching(".sublime-settings") {
            if (resource.path.starts_with("Packages/Default/")
                || resource.path.starts_with("Packages/User/"))
                && (resource.path.ends_with("/Preferences.sublime-settings")
                    || resource.path.rsplit('/').next() == Some(platform_preferences))
                && let Ok(doc) = resource.text().and_then(Document::parse)
                && let Some(values) = doc.value.get("ignored_packages").and_then(Value::as_array)
            {
                ignored = values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
            }
        }
        // Application-only: https://www.sublimetext.com/docs/settings.html
        // Project files cannot enable/disable packages, including Vintage.
        resources.set_ignored(ignored);
        phases.mark("resources_scanned");
        let translations = vel_i18n::Catalogs::load(&resources, &mut resource_messages);
        phases.mark("translations_ready");
        let mut settings = Settings::default();
        let mut user = serde_json::Map::new();
        let mut syntax_settings = crate::shipped::SyntaxSettings::default();
        // Derive known key types from our shipped settings, not from an
        // installed package which might itself need validation.
        let schema = Document::parse(include_str!(
            "../resources/Packages/Default/Preferences.sublime-settings"
        ))?
        .value;
        let schema = schema.as_object().expect("shipped settings object");
        for resource in resources.matching(".sublime-settings") {
            let file = resource.path.rsplit('/').next().unwrap_or("");
            if let Some(name) = file.strip_suffix(".sublime-settings")
                && !name.starts_with("Preferences")
            {
                syntax_settings.add(name, resource, &mut resource_messages);
                continue;
            }
            let source = if resource.origin.is_absolute()
                && resource
                    .origin
                    .extension()
                    .is_some_and(|ext| ext == "sublime-settings")
            {
                resource.origin.to_string_lossy()
            } else {
                std::borrow::Cow::Borrowed(resource.path.as_str())
            };
            let mut doc = match resource.text().and_then(Document::parse) {
                Ok(doc) => doc,
                Err(e) => {
                    let report = if e.starts_with(|ch: char| ch.is_ascii_digit()) {
                        format!("{source}:{e}")
                    } else {
                        format!("{source}: {e}")
                    };
                    resource_reports.push(report.clone());
                    resource_log.push(report.clone());
                    resource_messages.push(report);
                    continue;
                }
            };
            let Some(values) = doc.value.as_object_mut() else {
                resource_reports.push(format!("{source}: settings must be an object"));
                continue;
            };
            let issues = vel_config::settings::validate_layer(values, schema);
            if !issues.is_empty() {
                let report = format!("{source}: {}", issues.join("; "));
                resource_log.push(report.clone());
                resource_reports.push(report);
            }
            match resource.path.as_str() {
                "Packages/Default/Preferences.sublime-settings" => {
                    settings.replace(Layer::Default, doc.value)?
                }
                path if path.strip_prefix("Packages/Default/") == Some(platform_preferences) => {
                    settings.replace(Layer::Platform, doc.value)?
                }
                _ => {
                    if file == "Preferences.sublime-settings" || file == platform_preferences {
                        user.extend(doc.value.as_object().ok_or("settings object")?.clone());
                    }
                }
            }
        }
        settings.replace(Layer::User, Value::Object(user))?;
        let mut window_overrides = window_overrides.unwrap_or_else(|| {
            settings
                .get("new_window_settings")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default()
        });
        // A window's settings must not configure the next window recursively.
        window_overrides.remove("new_window_settings");
        let issues = vel_config::settings::validate_layer(&mut window_overrides, schema);
        if !issues.is_empty() {
            resource_reports.push(format!("new_window_settings: {}", issues.join("; ")));
        }
        if !window_overrides.is_empty() {
            settings.replace(Layer::Window, Value::Object(window_overrides))?;
        }
        settings.replace(
            Layer::Syntax,
            Value::Object(syntax_settings.values("Plain Text.sublime-syntax", "Plain Text")),
        )?;
        let mut distraction_settings = serde_json::json!({"line_numbers":false,"gutter":false,"draw_centered":true,"wrap_width":80,"word_wrap":true,"scroll_past_end":true,"show_minimap":false}).as_object().expect("object").clone();
        for resource in resources.matching("Distraction Free.sublime-settings") {
            if let Ok(doc) = resource.text().and_then(Document::parse)
                && let Some(values) = doc.value.as_object()
            {
                distraction_settings.extend(values.clone());
            }
        }
        let mut project_builds = Vec::new();
        let project_model = project
            .as_deref()
            .map(vel_session::project::Project::load)
            .transpose()?
            .unwrap_or_default();
        for value in &project_model.build_systems {
            let name = value["name"]
                .as_str()
                .ok_or("project build system needs name")?;
            project_builds.push(vel_build::BuildSystem::from_value(name, value.clone())?);
        }
        let build_systems = crate::shipped::Builds::new(
            resources
                .matching(".sublime-build")
                .into_iter()
                .cloned()
                .collect(),
            project_builds,
        );
        let mut folder_values = Vec::new();
        if let Some(path) = &project {
            let doc = Document::parse(&std::fs::read_to_string(path).map_err(|e| e.to_string())?)?;
            let mut project_settings = doc
                .value
                .get("settings")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            if let Some(values) = project_settings.as_object_mut() {
                values.remove("ignored_packages");
                let issues = vel_config::settings::validate_layer(values, schema);
                if !issues.is_empty() {
                    resource_reports.push(format!("{}: {}", path.display(), issues.join("; ")));
                }
            }
            settings.replace(Layer::Project, project_settings)?;
            folder_values = doc
                .value
                .get("folders")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
        }
        if let (_, Some(warning)) = fileio::fallback_encoding(&settings.editor().fallback_encoding)
        {
            resource_messages.push(warning);
        }
        let mut folders = Vec::new();
        let patterns = |value: Option<&Value>| -> Vec<String> {
            value
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        for folder in folder_values {
            let path = PathBuf::from(folder["path"].as_str().ok_or("project folder needs path")?);
            let path = if path.is_absolute() {
                path
            } else {
                project
                    .as_ref()
                    .and_then(|p| p.parent())
                    .unwrap_or(Path::new("."))
                    .join(path)
            };
            let path = absolute_folder(&path)?;
            let mut excludes = Vec::new();
            for key in [
                "file_exclude_patterns",
                "folder_exclude_patterns",
                "binary_file_patterns",
            ] {
                let mut list = patterns(settings.get(key));
                list.extend(patterns(folder.get(key)));
                excludes.push(list);
            }
            folders.push(vel_search::project::Folder {
                name: folder["name"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        path.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned()
                    }),
                path,
                rules: vel_search::project::Rules::new(&excludes[0], &excludes[1], &excludes[2])?
                    .with_includes(
                    &patterns(folder.get("file_include_patterns")),
                    &patterns(folder.get("folder_include_patterns")),
                )?,
                follow_links: folder["follow_symlinks"].as_bool().unwrap_or(true),
            });
        }
        phases.mark("settings_ready");
        #[cfg(not(windows))]
        let mut keymap = Keymap::default();
        #[cfg(windows)]
        let mut keymap = Keymap::windows();
        let registry = Editor::command_registry();
        let known = |name: &str| name == "noop" || registry.kind(name).is_some();
        let mut unsupported: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        let mut menus = Vec::new();
        let mut context_menus: BTreeMap<String, Vec<MenuItem>> = BTreeMap::new();
        let mut mousemap = Vec::new();
        let mut scrollbar_mousemap = Vec::new();
        const LEGACY: &str = "Packages/User/Velocity.sublime-keymap";
        let keymaps = resources.matching(".sublime-keymap");
        #[cfg(not(windows))]
        let mut keymaps = keymaps;
        let mut ignored_user_keymaps = IgnoredKeymaps::new();
        // Public User naming: https://www.sublimetext.com/docs/key_bindings.html#user-bindings
        // Only macOS generated a legacy Velocity-named User file.
        #[cfg(not(windows))]
        if let Some(i) = keymaps.iter().position(|r| r.path == LEGACY) {
            let first_user = keymaps
                .iter()
                .position(|r| r.path.starts_with("Packages/User/"))
                .unwrap_or(i);
            keymaps[first_user..=i].rotate_right(1);
        }
        for resource in keymaps {
            let package = resource.path.split('/').nth(1).unwrap_or_default();
            let file = resource.path.rsplit('/').next().unwrap_or_default();
            // https://www.sublimetext.com/docs/key_bindings.html#user-bindings
            let is_default = file == "Default.sublime-keymap" || file == PLATFORM_KEYMAP;
            let ours = package == "Default"
                && (file == "Velocity Vintage.sublime-keymap"
                    || (!cfg!(windows) && file == "Velocity.sublime-keymap"));
            if !(is_default || ours || (!cfg!(windows) && resource.path == LEGACY)) {
                if package == "User"
                    && !matches!(
                        file,
                        "Default (Linux).sublime-keymap"
                            | "Default (OSX).sublime-keymap"
                            | "Default (Windows).sublime-keymap"
                    )
                {
                    ignored_user_keymaps.insert(
                        resource.path.clone(),
                        IgnoredKeymap {
                            origin: resource.origin.clone(),
                            stamp: stamp(&resource.origin),
                        },
                    );
                }
                continue;
            }
            // Velocity does not provide Python commands; don't let an unavailable
            // package binding shadow a working shortcut. User bindings stay explicit.
            match resource.text().and_then(|source| {
                keymap.append_resource(
                    source,
                    &resource.path,
                    |invocation| {
                        package == "User"
                            || crate::panels::usable_package_binding(
                                &resources,
                                &known,
                                &invocation.command,
                                &invocation.args,
                                0,
                            )
                    },
                    &mut resource_messages,
                )
            }) {
                Ok(skipped) => unsupported.entry(package).or_default().0 += skipped,
                Err(e) => resource_messages.push(format!("{}: {e}", resource.path)),
            }
        }
        phases.mark("keymap_ready");
        // The public Menus reference allows Widget.sublime-settings to select
        // another resource by filename. The other surfaces keep their own names.
        let widget_context_menu = resources
            .matching("/Widget.sublime-settings")
            .into_iter()
            .rev()
            .find_map(|resource| {
                let doc = Document::parse(resource.text().ok()?).ok()?;
                let name = doc.value["context_menu"].as_str()?.trim();
                if name.is_empty() || name.contains(['/', '\\']) {
                    return None;
                }
                Some(if name.ends_with(".sublime-menu") {
                    name.to_owned()
                } else {
                    format!("{name}.sublime-menu")
                })
            })
            .unwrap_or_else(|| "Widget Context.sublime-menu".into());
        for resource in resources.matching(".sublime-menu") {
            let name = resource.path.rsplit('/').next().unwrap_or_default();
            let target = match name {
                "Main.sublime-menu" => &mut menus,
                "Context.sublime-menu"
                | "Tab Context.sublime-menu"
                | "Side Bar.sublime-menu"
                | "Side Bar Mount Point.sublime-menu"
                | "Widget Context.sublime-menu"
                | "Find in Files.sublime-menu" => context_menus.entry(name.to_owned()).or_default(),
                _ if name == widget_context_menu => {
                    context_menus.entry(name.to_owned()).or_default()
                }
                _ => continue,
            };
            match resource.text().and_then(ui::parse) {
                Ok(mut items) => {
                    #[cfg(windows)]
                    if resource.path == "Packages/Default/Main.sublime-menu" {
                        adapt_windows_menu(&mut items);
                    }
                    let package = resource.path.split('/').nth(1).unwrap_or_default();
                    unsupported.entry(package).or_default().1 += count_unknown_menu(&items, &known);
                    if resource.shipped() && !resource.path.starts_with("Packages/Default/") {
                        filter_shipped_menu(&mut items, &known);
                    }
                    ui::merge_menu(target, items);
                }
                Err(e) => resource_messages.push(format!("{}: {e}", resource.path)),
            }
        }
        let report_locale = std::cell::OnceCell::new();
        for (package, (count, shipped)) in resources.plugins() {
            if shipped {
                // Shipped packages have a known compatibility level, presented
                // in Package: Enable/Disable rather than as startup errors.
                continue;
            }
            let (keys, items) = unsupported.get(package).copied().unwrap_or_default();
            let version = resources
                .get(&format!("Packages/{package}/package-metadata.json"))
                .and_then(|r| r.text().ok())
                .and_then(|s| Document::parse(s).ok())
                .and_then(|doc| doc.value["version"].as_str().map(str::to_owned));
            // Reports are deduplicated by package/version, not by the individual missing commands.
            let tr = report_locale.get_or_init(|| {
                let mut tr = vel_i18n::Translator::default();
                tr.replace(translations.clone());
                let language = [Layer::User, Layer::Platform, Layer::Default]
                    .into_iter()
                    .find_map(|layer| settings.layer(layer).get("ui_language"))
                    .and_then(Value::as_str)
                    .unwrap_or(crate::ui_language::DEFAULT_UI_LANGUAGE);
                tr.select(
                    preferred_languages
                        .resolve(&tr, language)
                        .unwrap_or_default(),
                );
                tr
            });
            resource_reports.push(tr.tr_fmt(
                "Package {name}: {count} Python files not run; {keys} bindings / {items} menu items reference unavailable commands",
                &[
                    ("name", version.map_or_else(|| package.to_owned(), |v| format!("{package} ({v})"))),
                    ("count", count.to_string()),
                    ("keys", keys.to_string()),
                    ("items", items.to_string()),
                ],
            ));
        }
        for resource in resources.matching(".sublime-mousemap") {
            match resource.text().and_then(ui::parse_mousemap_with_warnings) {
                Ok((items, warnings)) => {
                    if resource.path.ends_with("/Scroll Bar.sublime-mousemap") {
                        scrollbar_mousemap.extend(items);
                    } else {
                        mousemap.extend(items);
                    }
                    mouse_warnings.extend(warnings.into_iter().map(|w| (resource.path.clone(), w)));
                }
                Err(e) => mouse_warnings.push((resource.path.clone(), e)),
            }
        }
        phases.mark("menus_mousemap_ready");
        if let Some(tools) = menus.iter_mut().find(|m| m.id == "tools") {
            let choices=std::iter::once("Automatic").chain(build_systems.names()).map(|name|serde_json::json!({"caption":name,"command":"set_build_system","args":{"name":name}})).collect::<Vec<_>>();
            if let Some(item) = tools
                .children
                .iter_mut()
                .find(|m| m.command.as_deref() == Some("show_build_systems"))
            {
                item.caption = "Build System".into();
                item.command = None;
                item.children = serde_json::from_value(serde_json::json!(choices))
                    .map_err(|e| e.to_string())?;
                item.children.extend(ui::parse::<MenuItem>(
                    r#"[{"caption":"New Build System","command":"new_build_system"}]"#,
                )?);
            }
        }
        let mut palette_extra = Vec::new();
        crate::release::extend_commands(&mut palette_extra, &mut menus, &settings);
        crate::ui_language::menu_rows(&mut menus, &translations);
        let packed = (!matches!(kind, ConfigKind::Eager))
            .then(crate::language::Assets::packed)
            .flatten();
        let using_pack = packed.is_some();
        let bundled = packed.unwrap_or_else(crate::language::Assets::bundled)?;
        phases.mark("bundled_syntax_ready");
        let assets = crate::language::Assets::with_package_overlay(
            &bundled,
            data_dir.as_deref(),
            &resources,
            using_pack,
        )?;
        phases.mark("effective_syntax_ready");
        let user_syntaxes: Vec<_> = resources
            .matching("")
            .into_iter()
            .filter(|r| {
                user_resource(r, data_dir.as_deref())
                    && [
                        ".sublime-syntax",
                        ".tmLanguage",
                        ".tmLanguage.json",
                        ".tmPreferences",
                    ]
                    .iter()
                    .any(|suffix| r.path.ends_with(suffix))
            })
            .collect();
        let (mut metadata_warnings, mut syntax_warnings): (Vec<_>, Vec<_>) = assets
            .syntaxes
            .warnings()
            .iter()
            .filter_map(|warning| {
                let path = user_syntaxes
                    .iter()
                    .find(|r| warning_from_path(warning, &r.path))
                    .map(|r| r.path.as_str());
                (path.is_some() || !bundled.syntaxes.warnings().contains(warning))
                    .then_some((path, warning.as_str()))
            })
            .partition(|(path, _)| path.is_some_and(|p| p.ends_with(".tmPreferences")));
        metadata_warnings.sort_by_key(|(path, _)| *path);
        syntax_warnings.sort_by_key(|(path, _)| *path);
        collect_resource_warnings(
            "syntax",
            syntax_warnings,
            &mut resource_reports,
            &mut resource_log,
        );
        collect_resource_warnings(
            "syntax metadata",
            metadata_warnings,
            &mut resource_reports,
            &mut resource_log,
        );
        let catalog = crate::completion::Catalog::load(&resources, &mut resource_messages);
        phases.mark("catalogs_ready");
        let (palette, startup_palette) = if matches!(kind, ConfigKind::FirstFrame) {
            (
                Vec::new(),
                Some(PaletteSeed {
                    menus: menus.clone(),
                    extra: palette_extra,
                    translations: translations.clone(),
                }),
            )
        } else {
            (
                load_palette(
                    &resources,
                    &menus,
                    palette_extra,
                    &translations,
                    &assets,
                    &catalog,
                    &mut resource_messages,
                ),
                None,
            )
        };
        phases.mark("palette_commands_ready");
        let mut scheme_resources = Vec::new();
        for suffix in [
            ".sublime-color-scheme",
            ".hidden-color-scheme",
            ".tmTheme",
            ".hidden-tmTheme",
        ] {
            for resource in resources.matching(suffix) {
                match resource.text() {
                    Ok(source) => scheme_resources.push((resource.path.clone(), source.to_owned())),
                    Err(e) => scheme_warnings.push((resource.path.clone(), e)),
                }
            }
        }
        let mut schemes = BTreeMap::new();
        let mut scheme_errors = BTreeMap::new();
        for (path, _) in &scheme_resources {
            let name = path.rsplit('/').next().expect("resource filename");
            if !schemes.contains_key(name) {
                // A User override can contribute warnings to a bundled scheme filename.
                let warning_path = scheme_resources
                    .iter()
                    .rev()
                    .find(|(p, _)| {
                        p.rsplit('/').next() == Some(name)
                            && resources
                                .get(p)
                                .is_some_and(|r| user_resource(r, data_dir.as_deref()))
                    })
                    .map_or(path, |(p, _)| p);
                let scheme = vel_color_scheme::load_resources(
                    scheme_resources
                        .iter()
                        .map(|(p, s)| (p.as_str(), s.as_str())),
                    name,
                )
                .map_err(|e| e.to_string());
                match scheme {
                    Ok(scheme) => {
                        scheme_warnings.extend(
                            scheme
                                .warnings()
                                .iter()
                                .map(|warning| (warning_path.clone(), warning.clone())),
                        );
                        schemes.insert(name.to_owned(), Arc::new(scheme));
                    }
                    Err(e) => {
                        scheme_errors.insert(name.to_owned(), e.clone());
                        scheme_warnings.push((warning_path.clone(), e));
                    }
                }
            }
        }
        for (name, source) in [
            (
                "Velocity Dark.sublime-color-scheme",
                include_str!(
                    "../resources/Packages/Color Scheme - Velocity/Velocity Dark.sublime-color-scheme"
                ),
            ),
            (
                "Velocity Light.sublime-color-scheme",
                include_str!(
                    "../resources/Packages/Color Scheme - Velocity/Velocity Light.sublime-color-scheme"
                ),
            ),
        ] {
            if !schemes.contains_key(name) {
                let scheme = vel_color_scheme::load_resources([(name, source)], name)
                    .map_err(|e| e.to_string())?;
                schemes.insert(name.to_owned(), Arc::new(scheme));
            }
        }
        phases.mark("color_schemes_ready");
        let appearance = appearance::Catalog::load(&resources, &schemes, &scheme_errors);
        let themes = crate::theme_ui::Themes::prepare(
            &resources,
            &schemes,
            &settings,
            &mut resource_messages,
        );
        phases.mark("theme_prepared");
        let macro_paths = resources
            .matching(".sublime-macro")
            .into_iter()
            .map(|r| r.path.clone())
            .collect();
        for (kind, warnings) in [
            ("mousemap", &mouse_warnings),
            ("color-scheme", &scheme_warnings),
        ] {
            collect_resource_warnings(
                kind,
                warnings.iter().map(|(path, warning)| {
                    let user = resources
                        .get(path)
                        .is_some_and(|r| user_resource(r, data_dir.as_deref()));
                    (user.then_some(path.as_str()), format!("{path}: {warning}"))
                }),
                &mut resource_reports,
                &mut resource_log,
            );
        }
        write_resource_log(data_dir.as_deref(), &resource_log, &mut resource_reports);
        phases.mark("resource_reports_ready");
        Ok(Self {
            appearance,
            resource_reports,
            ignored_user_keymaps,
            themes,
            macro_paths,
            resources,
            settings,
            keymap,
            syntax_settings,
            schemes,
            assets,
            catalog,
            menus,
            context_menus,
            widget_context_menu,
            mousemap,
            scrollbar_mousemap,
            palette,
            startup_palette,
            translations,
            folders,
            distraction_settings,
            build_systems,
            project_model,
            messages: resource_messages,
        })
    }
}
impl Workspace {
    pub fn new(
        text: Text,
        data_dir: Option<PathBuf>,
        project: Option<PathBuf>,
        watch: bool,
    ) -> Result<Self, String> {
        Self::new_with_window_overrides(
            Some(text),
            data_dir,
            project,
            watch,
            Some(Default::default()),
        )
    }

    pub(crate) fn new_for_startup(
        text: Text,
        data_dir: Option<PathBuf>,
        project: Option<PathBuf>,
        watch: bool,
        phases: &mut crate::report::StartupPhases,
    ) -> Result<Self, String> {
        Self::new_with_configuration(
            (!text.is_empty()).then_some(text),
            data_dir,
            project,
            (
                watch,
                if watch {
                    ConfigKind::FirstFrame
                } else {
                    ConfigKind::Eager
                },
            ),
            Some(Default::default()),
            Default::default(),
            phases,
        )
    }

    pub(crate) fn new_window(data_dir: Option<PathBuf>, watch: bool) -> Result<Self, String> {
        Self::new_with_window_overrides(None, data_dir, None, watch, None)
    }

    pub(crate) fn new_empty(
        data_dir: Option<PathBuf>,
        project: Option<PathBuf>,
        watch: bool,
    ) -> Result<Self, String> {
        Self::new_with_window_overrides(None, data_dir, project, watch, Some(Default::default()))
    }

    /// The Settings window's own workspace: the same data directory and language
    /// preference as this one, no document watcher and no window overrides. Built only
    /// when the window opens and dropped with it.
    pub(crate) fn for_settings_window(&self) -> Result<Self, String> {
        // It shares this window's syntax assets too: the legacy copy would stay
        // loaded for the rest of the process after the Settings window closed.
        let mut workspace = Self::new_with_configuration(
            None,
            self.data_dir.clone(),
            None,
            (
                false,
                if self.packed_syntaxes {
                    ConfigKind::InteractiveReload
                } else {
                    ConfigKind::Eager
                },
            ),
            Some(Default::default()),
            self.preferred_languages,
            &mut Default::default(),
        )?;
        workspace.dark = self.dark;
        if let Err(e) = workspace.themes.select(&workspace.settings, workspace.dark) {
            workspace.messages.push(e);
        }
        Ok(workspace)
    }

    pub(crate) fn new_with_window_overrides(
        text: Option<Text>,
        data_dir: Option<PathBuf>,
        project: Option<PathBuf>,
        watch: bool,
        window_overrides: Option<serde_json::Map<String, Value>>,
    ) -> Result<Self, String> {
        Self::new_configured(
            text,
            data_dir,
            project,
            watch,
            window_overrides,
            Default::default(),
            &mut Default::default(),
        )
    }

    #[cfg(test)]
    pub(crate) fn new_with_preferred_languages(
        text: Text,
        data_dir: Option<PathBuf>,
        watch: bool,
        preferred: &'static [&'static str],
    ) -> Result<Self, String> {
        Self::new_configured(
            Some(text),
            data_dir,
            None,
            watch,
            Some(Default::default()),
            crate::ui_language::PreferredLanguages(preferred),
            &mut Default::default(),
        )
    }

    fn new_configured(
        text: Option<Text>,
        data_dir: Option<PathBuf>,
        project: Option<PathBuf>,
        watch: bool,
        window_overrides: Option<serde_json::Map<String, Value>>,
        preferred_languages: crate::ui_language::PreferredLanguages,
        phases: &mut crate::report::StartupPhases,
    ) -> Result<Self, String> {
        // Another interactive window shares the syntax set and preference metadata the
        // first one decoded from the binary (the same corpus as the legacy loader),
        // with their compiled contexts, regex machines and caches. Loading the legacy
        // copy instead cost each later window's process a second syntax set (4.6 MB)
        // and a second set of caches (2026-11-28 runner call trees).
        let kind = if watch && crate::language::Assets::packed_ready() {
            ConfigKind::InteractiveReload
        } else {
            ConfigKind::Eager
        };
        Self::new_with_configuration(
            text,
            data_dir,
            project,
            (watch, kind),
            window_overrides,
            preferred_languages,
            phases,
        )
    }

    fn new_with_configuration(
        text: Option<Text>,
        data_dir: Option<PathBuf>,
        project: Option<PathBuf>,
        (watch, kind): (bool, ConfigKind),
        window_overrides: Option<serde_json::Map<String, Value>>,
        preferred_languages: crate::ui_language::PreferredLanguages,
        phases: &mut crate::report::StartupPhases,
    ) -> Result<Self, String> {
        let ConfigLoad {
            result,
            stamps: config_stamp,
        } = Configuration::load_with_window_overrides(
            data_dir.clone(),
            project.clone(),
            window_overrides,
            preferred_languages,
            kind,
            phases,
        );
        let Configuration {
            appearance,
            themes,
            resources,
            settings,
            keymap,
            syntax_settings,
            schemes,
            assets,
            catalog,
            menus,
            context_menus,
            widget_context_menu,
            mousemap,
            scrollbar_mousemap,
            palette,
            startup_palette,
            translations,
            macro_paths,
            folders,
            distraction_settings,
            build_systems,
            project_model,
            messages: resource_messages,
            mut resource_reports,
            ignored_user_keymaps,
        } = result?;
        report_ignored_keymaps(
            &IgnoredKeymaps::new(),
            &ignored_user_keymaps,
            &mut resource_reports,
        );
        let empty = text.is_none();
        let tab = Tab::new(text.unwrap_or_default(), &settings, &keymap)?;
        let mut localizer = vel_i18n::Translator::default();
        localizer.replace(translations);
        let mut messages = resource_messages;
        let startup_watcher = watch && matches!(kind, ConfigKind::FirstFrame);
        let watcher = if watch && !startup_watcher {
            match Watcher::new() {
                Ok(w) => Some(w),
                Err(e) => {
                    messages.push(format!("file notifications unavailable: {e}"));
                    None
                }
            }
        } else {
            None
        };
        phases.mark("watchers_prepared");
        let mut themes = crate::theme_ui::Themes::publish(themes);
        if let Err(e) = themes.select(&settings, true) {
            messages.push(e);
        }
        let mut workspace = Self {
            jump_history: Default::default(),
            appearance,
            appearance_preview: None,
            appearance_commits: Vec::new(),
            appearance_notice: None,
            appearance_revision: 0,
            #[cfg(test)]
            appearance_writes: Default::default(),
            themes,
            git: Default::default(),
            catalog,
            assets,
            syntax_settings,
            schemes,
            dark: true,
            workbench: vel_ui::workbench::Workbench::default(),
            distraction_free: false,
            window_layers: Default::default(),
            fullscreen: false,
            build_systems,
            build_system: None,
            project_model,
            distraction_settings,
            recent_files: Vec::new(),
            recent_folders: Vec::new(),
            recent_projects: Vec::new(),
            recent_menu_state: None,
            folders,
            transient_folders: Default::default(),
            tabs: vec![tab],
            closed_sheet_generation: 0,
            active: 0,
            empty,
            closed_files: Vec::new(),
            open_as_choices: Default::default(),
            closed_text: BTreeMap::new(),
            stdout_sheets: Default::default(),
            menus,
            context_menus,
            widget_context_menu,
            mousemap,
            scrollbar_mousemap,
            palette,
            startup_palette,
            ui: crate::ui_text::Chrome::new(&localizer),
            translations: localizer,
            preferred_languages,
            unsupported_ui_language: None,
            ui_language_notice: None,
            menu_revision: 0,
            palette_revision: 0,
            macro_paths,
            messages,
            resources,
            load_notices: Vec::new(),
            resource_reports,
            ignored_user_keymaps,
            settings_reports: Default::default(),
            io_workers: Default::default(),
            restore_in_progress: false,
            restore_source: None,
            restore_deadline: None,
            pending_revert: None,
            goto_recent_files: Vec::new(),
            pane_extent: (1200., 800.),
            pane_minimum: (120., 112.),
            restore_original: None,
            telemetry: Vec::new(),
            telemetry_enabled: None,
            settings,
            keymap,
            watcher,
            startup_watcher,
            packed_syntaxes: !matches!(kind, ConfigKind::Eager),
            data_dir,
            project,
            config_stamp,
            config_rx: None,
            last_config_check: None,
            reload_requested: false,
            settings_page: None,
            settings_writes: Vec::new(),
            settings_write_generation: 0,
            config_write_generation: (0, false),
            settings_write_overrides: BTreeMap::new(),
            settings_reload_deferred: false,
            resource_revision: 0,
        };
        if !empty {
            workspace.workbench.groups[0].add(workspace.tabs[0].sheet, false);
        }
        workspace.configure_language(0, None, true)?;
        workspace.observe_navigation();
        workspace.sync_ui_language();
        phases.mark("workspace_ready");
        Ok(workspace)
    }

    pub(crate) fn startup_services_pending(&self) -> bool {
        self.startup_watcher
    }

    pub(crate) fn start_startup_watcher(&mut self) -> bool {
        if !std::mem::take(&mut self.startup_watcher) {
            return false;
        }
        match Watcher::new() {
            Ok(mut watcher) => {
                let paths: std::collections::BTreeSet<_> = self
                    .tabs
                    .iter()
                    .filter(|tab| tab.lazy.is_none() && !tab.goto_preview)
                    .filter_map(|tab| tab.path.clone())
                    .collect();
                for path in &paths {
                    if let Err(error) = watcher.watch(path) {
                        self.load_notices.push(error);
                    }
                }
                self.watcher = Some(watcher);
                // Registration samples the *current* disk version. Check against the
                // version read at open so writes between open and present aren't lost.
                self.apply_file_changes(vec![Ok(paths.into_iter().collect())]);
            }
            Err(error) => self
                .messages
                .push(format!("file notifications unavailable: {error}")),
        }
        true
    }

    pub(crate) fn palette_pending(&self) -> bool {
        self.startup_palette.is_some()
    }

    pub(crate) fn finish_startup_palette(&mut self) {
        let Some(seed) = self.startup_palette.take() else {
            return;
        };
        self.palette = load_palette(
            &self.resources,
            &seed.menus,
            seed.extra,
            &seed.translations,
            &self.assets,
            &self.catalog,
            &mut self.messages,
        );
        self.palette_revision = self.palette_revision.wrapping_add(1);
    }
    /// Refreshes the resource-defined recent submenu only when its source changes.
    pub fn refresh_file_menus(&mut self) -> bool {
        let recent = [
            &self.recent_files,
            &self.recent_folders,
            &self.recent_projects,
        ];
        if self
            .recent_menu_state
            .as_ref()
            .is_some_and(|(old, revision)| {
                *revision == self.resource_revision && old.iter().eq(recent)
            })
        {
            return false;
        }
        let mut children = Vec::new();
        // The documented Main menu allows named submenus; separate file, folder and
        // project history without inserting a non-action when history is empty.
        // https://www.sublimetext.com/docs/menus.html
        for (caption, paths, command, key) in [
            ("File", &self.recent_files, "open_recent_file", "file"),
            ("Folder", &self.recent_folders, "open_recent_folder", "dir"),
            ("Project", &self.recent_projects, "open_project", "file"),
        ] {
            if !paths.is_empty() {
                let mut entries: Vec<_> = paths.iter().take(20).map(|path| {
                    serde_json::json!({"caption":path.display().to_string(), "command":command, "args":{key:path}})
                }).collect();
                if key == "file" && command == "open_project" {
                    entries.push(serde_json::json!({"caption":"-"}));
                    entries.push(serde_json::json!({
                        "caption":"Remove Deleted", "command":"remove_deleted_recent_projects"
                    }));
                }
                children.push(serde_json::json!({"caption":caption,"children":entries}));
            }
        }
        if !children.is_empty() {
            children.push(serde_json::json!({"caption":"-"}));
            children
                .push(serde_json::json!({"caption":"Clear Items", "command":"clear_recent_items"}));
        }
        if let Some(menu) = self
            .menus
            .iter_mut()
            .find(|menu| menu.id == "file")
            .and_then(|menu| {
                menu.children
                    .iter_mut()
                    .find(|item| item.id == "open_recent")
            })
        {
            menu.children =
                serde_json::from_value(serde_json::json!(children)).expect("authored recent menu");
        }
        self.recent_menu_state = Some((
            [
                self.recent_files.clone(),
                self.recent_folders.clone(),
                self.recent_projects.clone(),
            ],
            self.resource_revision,
        ));
        self.menu_revision = self.menu_revision.wrapping_add(1);
        true
    }
    pub(crate) fn ui_language_setting(&self) -> &str {
        self.global_string("ui_language", crate::ui_language::DEFAULT_UI_LANGUAGE)
    }

    pub(crate) fn resolve_ui_language(&self, requested: &str) -> Option<vel_i18n::Language> {
        self.preferred_languages
            .resolve(&self.translations, requested)
    }

    pub(crate) fn sync_ui_language(&mut self) -> bool {
        let setting = self.ui_language_setting();
        let language = self.resolve_ui_language(setting);
        if language.is_some() {
            self.unsupported_ui_language = None;
        } else if self.unsupported_ui_language.as_deref() != Some(setting) {
            self.unsupported_ui_language = Some(setting.to_owned());
            // The previous prepared text survives replacement of the catalog
            // snapshot, including deletion of the catalog that supplied it.
            self.ui_language_notice = Some(self.ui.unsupported_language.to_string());
        }
        self.translations.select(language.unwrap_or_default());
        if self.ui.revision == self.translations.revision() {
            return false;
        }
        self.ui = crate::ui_text::Chrome::new(&self.translations);
        for tab in &mut self.tabs {
            tab.editor.translations = self.translations.clone();
        }
        true
    }
    pub(crate) fn checked_ui_language(&self) -> &str {
        if self.ui_language_setting() == "auto" {
            "auto"
        } else {
            self.translations.matched_language().as_str()
        }
    }
    pub(crate) fn palette_caption(&self, item: &PaletteItem) -> Option<String> {
        let tr = &self.translations;
        if tr.language().is_english() {
            return None;
        }
        let caption = item.caption.as_str();
        // Only the UI prefix is translated; names can contain colons and can
        // even be identical to catalog keys.
        let prefix = match item.invocation.command.as_str() {
            "set_ui_language" => {
                return crate::ui_language::palette_caption(tr, item)
                    .or_else(|| tr.palette_caption(caption));
            }
            "set_file_type" => Some(("Set Syntax", tr.tr("Set Syntax"))),
            "insert_snippet" if caption.starts_with("Snippet: ") => {
                Some(("Snippet", tr.tr("Snippet")))
            }
            "set_build_system" if item.invocation.args["name"] != "Automatic" => {
                Some(("Tools: Build System", tr.tr("Tools: Build System")))
            }
            "run_macro_file" if caption.starts_with("Macro: ") => {
                let name = macro_name(caption);
                return tr
                    .palette_caption("Macro")
                    .map(|prefix| format!("{prefix}{}{name}", tr.palette_separator()));
            }
            "set_indentation" if item.invocation.args["width"].is_u64() => {
                Some(("Tab Width", tr.tr("Tab Width")))
            }
            "set_dictionary" if caption.starts_with("System: ") => {
                Some(("System", tr.tr("System")))
            }
            "open_project" | "build"
                if caption == item.invocation.args["file"]
                    || item.invocation.args["_build_picker"] == true =>
            {
                return None;
            }
            _ => None,
        };
        if let Some((prefix, localized)) = prefix {
            let name = caption.strip_prefix(prefix)?.strip_prefix(": ")?;
            return (localized != prefix)
                .then(|| format!("{localized}{}{name}", tr.palette_separator()));
        }
        tr.palette_caption(caption)
    }
    pub fn track_settings_write(
        &mut self,
        rx: std::sync::mpsc::Receiver<Result<(), String>>,
        overrides: &[(String, Option<Value>)],
    ) {
        self.settings_write_generation = self.settings_write_generation.wrapping_add(1);
        let generation = self.settings_write_generation;
        self.settings_write_overrides.extend(
            overrides
                .iter()
                .map(|(key, value)| (key.clone(), (generation, value.clone()))),
        );
        self.settings_writes.push((rx, generation));
    }
    #[cfg(test)]
    pub(crate) fn settings_write_count(&self) -> u64 {
        self.settings_write_generation
    }
    /// Stops a failed writer's values from surviving an in-flight resource load.
    /// Matching values keep unrelated keys and later writes to the same key intact.
    pub(crate) fn discard_settings_write(&mut self, pairs: &[(&str, Value)]) {
        for (key, value) in pairs {
            if self
                .settings_write_overrides
                .get(*key)
                .is_some_and(|(_, pending)| pending.as_ref() == Some(value))
            {
                self.settings_write_overrides.remove(*key);
            }
        }
    }
    #[cfg(test)]
    pub fn wait_resources(&mut self) {
        // Tests explicitly await a disk observation as well as any asynchronous worker.
        // Force that observation without changing the production polling interval.
        self.last_config_check = None;
        let deadline = Instant::now() + std::time::Duration::from_secs(120);
        loop {
            self.poll();
            if !self.resources_busy() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "resource worker did not finish: {:?}",
                self.messages
            );
            std::thread::park_timeout(std::time::Duration::from_millis(1));
        }
    }
    /// Tests can observe one disk scan/publication while preferences IO remains blocked.
    #[cfg(test)]
    pub(crate) fn poll_resource_reload(&mut self) {
        self.last_config_check = None;
        self.poll();
        if let Some(pending) = self.config_rx.take() {
            // Synchronize with the resource worker, then let the real poll path
            // publish its result with the original settings-write generation.
            // The timeout guards a stuck worker, not an expected timing window.
            let result = pending
                .recv_timeout(Duration::from_secs(120))
                .expect("resource worker did not complete");
            let (tx, rx) = std::sync::mpsc::channel();
            tx.send(result).expect("resource completion receiver");
            self.config_rx = Some(rx);
            self.poll();
        }
    }
    pub fn request_resource_reload(&mut self) {
        self.reload_requested = true;
    }
    pub fn resources_busy(&self) -> bool {
        self.config_rx.is_some()
            || self.reload_requested
            || !self.settings_writes.is_empty()
            || !self.appearance_commits.is_empty()
    }
    pub fn ignored_packages(&self) -> std::collections::BTreeSet<String> {
        self.settings
            .get("ignored_packages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    }
    /// Consume only the active tab's explicit location-centering request.
    pub fn take_pending_center(&mut self) -> bool {
        std::mem::take(&mut self.tabs[self.active].pending_center)
    }
    pub fn editor(&self) -> &Editor {
        &self.tabs[self.active].editor
    }
    pub fn editor_mut(&mut self) -> &mut Editor {
        if let Err(e) = self.prepare_sheet(self.active) {
            self.messages.push(e);
        }
        self.observe_navigation();
        self.refresh_appearance(self.active);
        &mut self.tabs[self.active].editor
    }
    /// A resource load is running on the worker and has not been published yet.
    #[cfg(test)]
    pub(crate) fn resource_load_in_flight(&self) -> bool {
        self.config_rx.is_some()
    }
    pub fn language_busy(&self) -> bool {
        self.config_rx.is_some()
            || self.reload_requested
            || self.tabs.iter().any(|tab| {
                tab.loading.is_some()
                    || tab.binary_loading.is_some()
                    || tab.editor.layout.borrow().long.pending()
                    || tab.editor.reindent_job.is_some()
                    || tab
                        .editor
                        .language
                        .borrow()
                        .as_ref()
                        .is_some_and(|l| l.busy())
            })
    }
    pub fn is_empty(&self) -> bool {
        self.empty
    }
    /// Attach the empty window's input adapter when its first edit arrives.
    pub fn materialize_input(&mut self) {
        if self.empty && self.tabs[0].dirty() {
            self.empty = false;
            self.workbench.groups[self.workbench.active].add(self.tabs[0].sheet, false);
        }
    }
    pub fn new_file(&mut self) -> Result<(), String> {
        self.new_text_tab(Text::new())
    }
    pub(crate) fn show_find_results(&mut self) -> Result<usize, String> {
        if let Some(index) = self.tabs.iter().position(|tab| tab.find_results) {
            self.select(index);
            return Ok(index);
        }
        self.new_file()?;
        let index = self.active;
        let tab = &mut self.tabs[index];
        tab.find_results = true;
        tab.suggested_name = Some("Find Results".into());
        tab.editor
            .settings
            .set(Layer::View, "word_wrap", Value::Bool(false));
        self.configure_language(index, Some(crate::panels::find_results::SYNTAX), false)?;
        Ok(index)
    }
    pub(crate) fn new_text_tab(&mut self, text: Text) -> Result<(), String> {
        self.materialize_input();
        if self.empty {
            self.tabs.clear();
            self.empty = false;
        }
        self.tabs
            .push(Tab::new(text, &self.settings, &self.keymap)?);
        self.active = self.tabs.len() - 1;
        self.insert_opened_sheet(self.workbench.active, self.tabs[self.active].sheet);
        // An untitled buffer is already loaded; it has no disk indentation to detect.
        self.tabs[self.active].needs_detection = false;
        self.tabs[self.active].indentation_initialized = true;
        self.configure_language(self.active, None, false)?;
        Ok(())
    }
    pub(crate) fn insert_opened_sheet(&mut self, group: usize, sheet: u64) {
        if self.settings.bool("open_tabs_after_current", false) {
            self.workbench.groups[group].add_after_current(sheet);
        } else {
            self.workbench.groups[group].add(sheet, false);
        }
    }
    pub(crate) fn open_file_argument(
        &mut self,
        argument: &str,
        args: &Value,
    ) -> Result<(), String> {
        let res = argument.strip_prefix("res://").unwrap_or(argument);
        if res.starts_with("Packages/") {
            let resource = self
                .resources
                .get(res)
                .ok_or_else(|| format!("resource {res} is not available"))?;
            let text = Text::from(resource.text()?);
            self.new_text_tab(text)?;
            let tab = &mut self.tabs[self.active];
            tab.suggested_name = res.rsplit('/').next().map(str::to_owned);
            tab.editor.read_only = true;
            return Ok(());
        }
        let flag = |key: &str| args[key].as_bool().unwrap_or(false);
        // Preserve Velocity's existing filename-suffix shortcut for commands
        // that predate the explicit flag; false requests a literal filename.
        let location = if args["encoded_position"].as_bool().unwrap_or(true) {
            FileLocation::parse(argument)
        } else {
            FileLocation::path_only(argument.into())
        };
        let transient = flag("transient");
        let force_group = flag("force_group");
        let force_clone = flag("force_clone");
        let clear_right = flag("clear_to_right");
        let additive = clear_right || flag("add_to_selection");
        if !force_group && !force_clone && !additive {
            return if transient {
                self.preview(location)
            } else {
                self.open(location)
            };
        }

        let _request = self.navigation_request();
        self.materialize_input();
        let group = self.workbench.active;
        let previous = additive.then(|| self.workbench.groups.clone());
        let path = absolute(&location.path)?;
        let mut existing = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| tab.path.as_ref() == Some(&path));
        let first = existing.next().map(|(i, _)| i);
        let in_group = |i: usize| self.workbench.group_of(self.tabs[i].sheet) == Some(group);
        let local = first
            .filter(|&i| in_group(i))
            .or_else(|| existing.find(|(i, _)| in_group(*i)).map(|(i, _)| i));
        if let Some(source) = first.filter(|_| force_clone || (force_group && local.is_none())) {
            self.clone_file_into_group(source, group)?;
            let tab = &mut self.tabs[self.active];
            tab.transient = transient;
            tab.preview_recent_pending = transient;
            if location.has_position() {
                tab.goto(&location);
            }
            if !transient {
                crate::workbench::remember(&mut self.recent_files, path);
            }
        } else if let Some(index) = local.filter(|&index| force_group && Some(index) != first) {
            self.select(index);
            if !transient {
                self.make_permanent(index);
            }
            self.goto_open_tab(index, &location);
        } else if transient {
            self.preview(location)?;
        } else {
            self.open_mode(location, false)?;
        }
        if let Some(previous) = previous {
            let group = self.workbench.active;
            let sheet = self.tabs[self.active].sheet;
            let previous = &previous[group];
            let end = previous
                .focused
                .filter(|_| clear_right)
                .and_then(|id| previous.sheets.iter().position(|&s| s == id))
                .map_or(previous.sheets.len(), |i| i + 1);
            let current = &mut self.workbench.groups[group];
            current.selected.extend(
                previous.sheets[..end]
                    .iter()
                    .filter(|id| previous.selected.contains(id) && current.sheets.contains(id)),
            );
            current.selected.insert(sheet);
        }
        if !transient {
            self.remember_goto_file();
        }
        self.observe_navigation();
        Ok(())
    }
    pub(crate) fn can_open_as(&self, index: usize, mode: crate::open_requests::OpenAs) -> bool {
        use crate::open_requests::OpenAs;
        let tab = &self.tabs[index];
        if tab.path.is_none()
            || tab.load_failed
            || tab.deleted
            || tab.dirty()
            || tab.find_results
            || tab.editor.marked.is_some()
            || tab.loading.is_some()
            || tab.binary_loading.is_some()
        {
            return false;
        }
        let binary = tab.editor.binary.is_some();
        let wants_image = tab.is_image_file();
        match mode {
            OpenAs::Text => binary && wants_image,
            OpenAs::Image => {
                (binary
                    || (tab.disk_saved.is_some()
                        && tab
                            .path
                            .as_deref()
                            .is_some_and(crate::image_decode::known_extension)))
                    && (!wants_image || (tab.image_failed && tab.open_as().is_none()) || !binary)
            }
        }
    }
    pub(crate) fn set_open_as(
        &mut self,
        index: usize,
        mode: crate::open_requests::OpenAs,
    ) -> Result<(), String> {
        use crate::open_requests::OpenAs;
        if !self.can_open_as(index, mode) {
            return Ok(());
        }
        let tab = &mut self.tabs[index];
        let path = tab.path.clone().expect("file-backed tab");
        if mode == OpenAs::Image && tab.editor.binary.is_none() {
            tab.load_hex(String::new(), false)?;
        }
        self.open_as_choices.borrow_mut().insert(path.clone(), mode);
        for tab in &mut self.tabs {
            if tab.path.as_ref() == Some(&path) {
                tab.open_as_choices = Some(self.open_as_choices.clone());
                tab.image_failed = false;
            }
        }
        Ok(())
    }
    pub fn select(&mut self, index: usize) {
        self.observe_navigation();
        if !self.empty
            && index < self.tabs.len()
            && self.workbench.group_of(self.tabs[index].sheet).is_some()
        {
            if index != self.active {
                self.editor_mut().completion.hide();
                self.editor_mut().cancel_composition();
            }
            self.editor_mut().keymap.cancel();
            self.activate_sheet(index);
            self.refresh_appearance(index);
            self.observe_navigation();
        }
    }
    pub fn open(&mut self, location: FileLocation) -> Result<(), String> {
        let _request = self.navigation_request();
        let result = self.open_mode(location, false);
        if result.is_ok() {
            self.remember_goto_file();
        }
        self.observe_navigation();
        result
    }
    pub(crate) fn remember_goto_file(&mut self) {
        if let Some(path) = self.tabs[self.active].path.clone() {
            self.goto_recent_files
                .retain(|other| !vel_config::paths::equal(other, &path));
            self.goto_recent_files.insert(0, path);
            self.goto_recent_files.truncate(100);
        }
    }
    pub fn preview(&mut self, location: FileLocation) -> Result<(), String> {
        let target = absolute(&location.path)?;
        if !self.tabs.iter().any(|t| t.path.as_ref() == Some(&target))
            && let Some(index) = self.tabs.iter().position(|t| {
                t.transient
                    && !t.dirty()
                    && self.workbench.group_of(t.sheet) == Some(self.workbench.active)
            })
        {
            self.close(index)?;
        }
        self.open_mode(location, true)
    }
    pub(crate) fn open_from_sidebar(
        &mut self,
        location: FileLocation,
        permanent: bool,
    ) -> Result<(), String> {
        let _request = self.navigation_request();
        // D4126: the legacy single-file sidebar selection may switch groups.
        if !self.settings.bool("select_across_groups", false) {
            let group = self.workbench.active;
            let path = absolute(&location.path)?;
            let mut matches = self.tabs.iter().enumerate().filter(|(_, tab)| {
                tab.path.as_ref() == Some(&path) && self.workbench.group_of(tab.sheet).is_some()
            });
            if let Some((first, tab)) = matches.next() {
                let in_group = |tab: &Tab| self.workbench.group_of(tab.sheet) == Some(group);
                if let Some(index) = if in_group(tab) {
                    Some(first)
                } else {
                    matches
                        .find(|(_, tab)| in_group(tab))
                        .map(|(index, _)| index)
                } {
                    self.select(index);
                } else {
                    self.observe_navigation();
                    self.clone_file_into_group(first, group)?;
                    let clone = &mut self.tabs[self.active];
                    clone.transient = !permanent;
                    clone.preview_recent_pending = !permanent;
                }
                if permanent {
                    self.make_permanent(self.active);
                    self.remember_goto_file();
                }
                self.observe_navigation();
                return Ok(());
            }
        }
        if permanent {
            self.open(location)
        } else {
            self.preview(location)
        }
    }
    fn open_tab_for_path(&self, path: &Path) -> Option<usize> {
        self.tabs.iter().position(|tab| {
            !tab.goto_preview
                && (tab
                    .path
                    .as_deref()
                    .is_some_and(|p| vel_config::paths::equal(p, path))
                    || (tab.load_failed
                        && tab
                            .path
                            .as_deref()
                            .and_then(|p| absolute(p).ok())
                            .as_deref()
                            == Some(path)))
        })
    }
    fn visit_open_tab(
        &mut self,
        index: usize,
        path: &Path,
        location: &FileLocation,
        preview: bool,
    ) -> Result<(), String> {
        if self.tabs[index].load_failed {
            let identity = self.tabs[index].session_document;
            for tab in &mut self.tabs {
                if tab.session_document == identity {
                    tab.path = Some(path.to_owned());
                }
            }
            self.retry_failed_document(index)?;
        }
        self.select(index);
        if !preview {
            self.make_permanent(index);
        }
        self.goto_open_tab(index, location);
        Ok(())
    }
    fn load_open_tab(
        &mut self,
        path: &Path,
        location: FileLocation,
        sheet: u64,
    ) -> Result<Tab, String> {
        let started = Instant::now();
        let mut tab = Tab::new_with_sheet(Text::new(), &self.settings, &self.keymap, sheet)?;
        tab.open_as_choices = Some(self.open_as_choices.clone());
        tab.path = Some(path.to_owned());
        tab.location = location.has_position().then_some(location);
        let decode = fileio::DecodeOptions::from_settings(&tab.editor.preferences);
        let metadata = fileio::regular::metadata(path, true)?;
        let size = metadata.as_ref().map_or(0, |m| m.len());
        if metadata.is_none() {
            if let Some(location) = tab.location.take() {
                tab.goto(&location);
            }
        } else {
            let background = size
                >= tab
                    .editor
                    .settings
                    .usize("large_file.load_bytes", 64 * 1024 * 1024) as u64;
            if tab.open_as() == Some(crate::open_requests::OpenAs::Image) {
                tab.load_hex(String::new(), false)?;
            } else {
                let workers = self.settings.usize("io_workers", 0);
                tab.load_disk(
                    decode,
                    background || workers != 0,
                    false,
                    Some((&self.io_workers, workers)),
                )?;
            }
        }
        // Installation suppresses restore-thread filesystem probes. Ordinary
        // opens still need initial stamps for focus-save and deletion handling,
        // unless the file changed while it was read (the load kept its sample).
        if !tab.reload_deferred {
            tab.disk_saved = vel_session::DiskStamp::read(path);
            tab.disk_stamp = stamp(path);
        }
        record_telemetry(
            &mut self.telemetry,
            self.telemetry_enabled,
            || serde_json::json!({"kind":"file_open","phase":if tab.loading.is_some(){"queued"}else{"fully_loaded"},"source_bytes":size,"elapsed_ms":started.elapsed().as_secs_f64()*1000.,"background":tab.loading.is_some(),"path":path}),
        );
        Ok(tab)
    }
    fn watch_open_path(&mut self, path: &Path) -> Result<(), String> {
        if let Some(w) = self.watcher.as_mut()
            && path.parent().is_some_and(Path::is_dir)
        {
            w.watch(path)?;
        }
        Ok(())
    }
    fn open_mode(&mut self, location: FileLocation, preview: bool) -> Result<(), String> {
        self.materialize_input();
        let path = absolute(&location.path)?;
        if let Some(index) = self.open_tab_for_path(&path) {
            return self.visit_open_tab(index, &path, &location, preview);
        }
        let mut tab = self.load_open_tab(&path, location, crate::workbench::next_sheet())?;
        tab.transient = preview;
        tab.preview_recent_pending = preview;
        self.watch_open_path(&path)?;
        // The empty window's input adapter is never a sheet, including during preview.
        if self.empty
            || (self.tabs.len() == 1
                && !preview
                && self.tabs[0].path.is_none()
                && !self.tabs[0].find_results
                && !self.tabs[0].editor.buffer.is_dirty()
                && self.tabs[0].editor.text.len_bytes() == 0)
        {
            self.workbench
                .groups
                .iter_mut()
                .for_each(|g| g.remove(self.tabs[0].sheet));
            self.tabs[0] = tab;
            self.active = 0;
        } else {
            self.tabs.push(tab);
            self.active = self.tabs.len() - 1;
        }
        self.empty = false;
        self.insert_opened_sheet(self.workbench.active, self.tabs[self.active].sheet);
        if !preview {
            crate::workbench::remember(&mut self.recent_files, path);
        }
        self.configure_language(self.active, None, true)?;
        Ok(())
    }
    pub(crate) fn preview_goto(
        &mut self,
        location: FileLocation,
        group: usize,
    ) -> Result<(), String> {
        let path = absolute(&location.path)?;
        // Prefer a clone already in the destination group.
        let existing = self.open_tab_for_path(&path).map(|first| {
            if self.workbench.group_of(self.tabs[first].sheet) == Some(group) {
                first
            } else {
                self.tabs
                    .iter()
                    .enumerate()
                    .skip(first + 1)
                    .find(|(_, tab)| {
                        tab.path.as_ref() == Some(&path)
                            && self.workbench.group_of(tab.sheet) == Some(group)
                    })
                    .map_or(first, |(index, _)| index)
            }
        });
        if let Some(index) = existing {
            return self.visit_open_tab(index, &path, &location, true);
        }
        let slot = self.tabs.iter().rposition(|tab| tab.goto_preview);
        if let Some(index) = slot.filter(|&i| self.tabs[i].path.as_deref() == Some(&path)) {
            self.active = index;
            self.workbench.active = group;
            self.goto_open_tab(index, &location);
            return Ok(());
        }
        let sheet = slot.map_or_else(crate::workbench::next_sheet, |i| self.tabs[i].sheet);
        let mut tab = self.load_open_tab(&path, location, sheet)?;
        tab.goto_preview = true;
        if let Some(index) = slot {
            // Replacing the preview drops the previous load receiver (and cancels
            // that load) without running the real tab open/close lifecycle.
            self.tabs[index] = tab;
            self.active = index;
        } else {
            self.active = self.tabs.len();
            self.tabs.push(tab);
        }
        self.workbench.active = group;
        self.configure_language(self.active, None, true)
    }
    pub(crate) fn discard_goto_preview(&mut self) {
        if let Some(index) = self.tabs.iter().rposition(|tab| tab.goto_preview) {
            let tab = self.tabs.remove(index);
            self.closed_sheet_generation = self.closed_sheet_generation.wrapping_add(1);
            if tab.editor.text.len_bytes() >= vel_platform::memory_pressure::LARGE_WORK_BYTES {
                vel_platform::memory_pressure::request();
            }
            if self.active == index {
                self.active = self.workbench.groups[self.workbench.active]
                    .focused
                    .and_then(|sheet| self.sheet_index(sheet))
                    .unwrap_or(0);
            } else if self.active > index {
                self.active -= 1;
            }
        }
    }
    pub(crate) fn commit_goto_preview(&mut self) {
        if !self.tabs[self.active].goto_preview {
            self.discard_goto_preview();
            self.make_permanent(self.active);
            return;
        }
        let path = self.tabs[self.active]
            .path
            .clone()
            .expect("Goto preview path");
        if self.empty {
            // Same input adapter as before the preview; it was never a real tab.
            self.tabs.remove(0);
            self.active -= 1;
            self.empty = false;
        }
        let tab = &mut self.tabs[self.active];
        tab.goto_preview = false;
        tab.preview_recent_pending = true;
        let sheet = tab.sheet;
        self.insert_opened_sheet(self.workbench.active, sheet);
        self.make_permanent(self.active);
        if let Err(error) = self.watch_open_path(&path) {
            self.load_notices.push(error);
        }
    }
    fn goto_open_tab(&mut self, index: usize, location: &FileLocation) {
        if location.has_position() {
            let tab = &mut self.tabs[index];
            // A duplicate open during loading retains the explicit jump until
            // the final text is installed. A bare reopen leaves it alone.
            if tab.loading.is_some() || tab.binary_loading.is_some() {
                tab.location = Some(location.clone());
            }
            tab.goto(location);
        }
    }
    pub fn save(&mut self, index: usize, path: Option<PathBuf>) -> Result<(), String> {
        self.materialize_input();
        if self.empty {
            return Err("no file to save".into());
        }
        self.prepare_sheet(index)?;
        self.make_permanent(index);
        let tab = &mut self.tabs[index];
        // Never serialize a binary/hex document through text, even if a future
        // attachment path accidentally loses its read-only flag.
        if tab.binary_document || tab.editor.binary.is_some() {
            return Err("binary/hex documents cannot be saved as text".into());
        }
        if tab.loading.is_some() || tab.binary_loading.is_some() || tab.editor.read_only {
            return Err("file is loading or its load failed".into());
        }
        let (conflict, expected_disk) = self.resolve_save_conflict(index, path.as_deref())?;
        if conflict {
            return Err("File changed on disk; confirm Overwrite or Cancel".into());
        }
        let tab = &mut self.tabs[index];
        tab.editor.unmark();
        let target = path
            .or_else(|| tab.path.clone())
            .ok_or("save requires a path")?;
        let target = absolute(&target)?;
        self.settings_reports.claim(&target);
        let settings = tab.editor.settings.editor();
        fileio::prepare_save(
            &mut tab.editor.buffer,
            tab.editor.id,
            &settings,
            &tab.tracker,
        );
        tab.editor.sync();
        let workers = self.settings.usize("io_workers", 0);
        let saved = if workers == 0 {
            fileio::save_report_with_expected(
                &target,
                tab.editor.buffer.text(),
                &tab.format,
                settings.atomic_save,
                self.data_dir.as_deref(),
                expected_disk,
            )?
        } else {
            let (target, text, format, data) = (
                target.clone(),
                tab.editor.buffer.text().clone(),
                tab.format.clone(),
                self.data_dir.clone(),
            );
            self.io_workers.write(workers, move || {
                fileio::save_report_with_expected(
                    &target,
                    &text,
                    &format,
                    settings.atomic_save,
                    data.as_deref(),
                    expected_disk,
                )
            })??
        };
        if let Some(warning) = saved.warning {
            self.resource_reports.push(warning);
        }
        let content = saved.content;
        tab.editor.buffer.mark_saved();
        tab.editor.sync();
        tab.saved_format = tab.format.clone();
        tab.disk_content = Some(content);
        tab.saved_in_effective_encoding = true;
        // Plain save records this revision's codec and raw identity. Only an
        // explicit encoding command can change the independent preference.
        tab.restored_format_dirty = false;
        tab.tracker.mark_saved(&tab.editor.buffer);
        if tab.find_results {
            // An explicit Save As makes the scratch report an ordinary file.
            tab.find_results = false;
            tab.suggested_name = None;
        }
        tab.path = Some(target.clone());
        tab.deleted = false;
        let document = tab.session_document;
        tab.disk_stamp = stamp(&target);
        tab.reload_deferred = false;
        tab.disk_saved = vel_session::DiskStamp::read(&target);
        tab.overwrite_allowed = None;
        tab.editor.annotations.borrow_mut().reference(
            tab.editor.buffer.text().clone(),
            tab.path.clone(),
            self.data_dir.as_ref().map(|d| d.join("Packages")),
        );
        let saved_format = tab.saved_format.clone();
        let encoding_chosen = tab.encoding_chosen;
        let preferred_encoding = tab.preferred_encoding.clone();
        let disk_saved = tab.disk_saved.clone();
        self.sync_shared();
        for sibling in &mut self.tabs {
            if sibling.session_document == document {
                sibling.deleted = false;
                // Hidden restored clones have not attached to the live buffer yet.
                // Their lazy metadata must reflect this save too, including Save As.
                if let Some((saved, _)) = &mut sibling.lazy {
                    sibling.path = Some(target.clone());
                    sibling.format = saved_format.clone();
                    sibling.saved_format = saved_format.clone();
                    sibling.encoding_chosen = encoding_chosen;
                    sibling.preferred_encoding = preferred_encoding.clone();
                    sibling.disk_saved = disk_saved.clone();
                    sibling.disk_content = Some(content);
                    sibling.saved_in_effective_encoding = true;
                    sibling.restored_format_dirty = false;
                    saved.path = Some(target.clone());
                    saved.disk = disk_saved.clone();
                    saved.disk_content = Some(content);
                    saved.saved_in_effective_encoding = true;
                    saved.effective_encoding = Some(saved_format.encoding.clone());
                    saved.encoding = saved_format.encoding.clone();
                    saved.encoding_chosen = sibling.encoding_chosen;
                    saved.preferred_encoding = sibling.preferred_encoding.clone();
                    saved.encoding_display_name = saved_format.display_name.clone();
                    saved.bom = saved_format.bom;
                    saved.line_ending = saved_format.line_ending.name().into();
                    saved.dirty = false;
                }
            } else if sibling.path.as_ref() == Some(&target) {
                sibling.deleted = false;
            }
        }
        if let Some(w) = self.watcher.as_mut()
            && let Err(e) = w.watch(&target)
        {
            self.resource_reports.push(e);
        }
        if let Err(e) = self.configure_language(index, None, true) {
            self.messages.push(e);
        }
        crate::workbench::remember(&mut self.recent_files, target);
        Ok(())
    }
    /// Saves dirty documents independently. Cancellation skips that document; errors
    /// include its path/name and do not prevent later documents from being saved.
    #[cfg(test)]
    pub fn save_all(&mut self, mut choose: impl FnMut(&str) -> Option<PathBuf>) -> Vec<String> {
        let mut errors = Vec::new();
        for index in 0..self.tabs.len() {
            if !self.tabs[index].dirty() {
                continue;
            }
            let path = if self.tabs[index].path.is_none() {
                let Some(path) = choose(&self.tabs[index].name()) else {
                    continue;
                };
                Some(path)
            } else {
                None
            };
            if let Err(error) = self.save(index, path) {
                errors.push(format!("{}: {error}", self.tabs[index].name()));
            }
        }
        errors
    }
    pub fn close(&mut self, index: usize) -> Result<(), String> {
        if self.empty {
            return Ok(());
        }
        self.prepare_close_sheet(index)?;
        self.tabs[index].editor.rebase_navigation(true);
        let tab = &self.tabs[index];
        let sheet = tab.sheet;
        let active = self.tabs[self.active].sheet;
        let group = self
            .workbench
            .group_of(sheet)
            .unwrap_or(self.workbench.active);
        let order = &self.workbench.groups[group].sheets;
        let at = order.iter().position(|id| *id == sheet).unwrap_or(0);
        let selected = &self.workbench.groups[group].selected;
        let neighbor = order
            .iter()
            .skip(at + 1)
            .find(|id| selected.contains(id))
            .or_else(|| order[..at].iter().rev().find(|id| selected.contains(id)))
            .or_else(|| order.get(at + 1))
            .or_else(|| at.checked_sub(1).and_then(|i| order.get(i)))
            .copied();
        if (!tab.transient || tab.dirty())
            && let Some(path) = &tab.path
        {
            self.closed_files.push((
                path.clone(),
                vel_session::ViewState::capture(&tab.editor.buffer, tab.editor.id, tab.scroll)
                    .with_line_height(tab.scroll_line_height),
            ));
            if self.closed_files.len() > 100 {
                self.closed_files.remove(0);
            }
        }
        if self.stdout_sheets.contains(&sheet) {
            self.closed_text.insert(sheet, tab.editor.text.clone());
        }
        for group in &mut self.workbench.groups {
            group.remove(sheet);
        }
        let large = tab.editor.text.len_bytes() >= vel_platform::memory_pressure::LARGE_WORK_BYTES;
        let closed = self.tabs.remove(index);
        self.closed_sheet_generation = self.closed_sheet_generation.wrapping_add(1);
        if let Some(path) = closed.path.as_deref()
            && let Some(watcher) = self.watcher.as_mut()
            && !self.tabs.iter().any(|tab| {
                !tab.goto_preview && tab.lazy.is_none() && tab.path.as_deref() == Some(path)
            })
        {
            watcher.unwatch(path);
        }
        drop(closed);
        if large {
            vel_platform::memory_pressure::request();
        }
        if self.tabs.is_empty() {
            self.tabs
                .push(Tab::new(Text::new(), &self.settings, &self.keymap)?);
            self.active = 0;
            self.empty = true;
            self.tabs[0].needs_detection = false;
            self.tabs[0].indentation_initialized = true;
            self.configure_language(0, None, false)?;
        } else {
            let target = if sheet == active {
                neighbor
            } else {
                Some(active)
            }
            .and_then(|id| self.sheet_index(id))
            .or_else(|| {
                self.workbench
                    .groups
                    .iter()
                    .filter_map(|g| g.focused)
                    .find_map(|id| self.sheet_index(id))
            })
            .unwrap_or(0);
            self.active = target;
            // Closing one sheet retains the surviving side-by-side selection. The
            // public multi-select guide does not specify close's selection behavior;
            // preserve it rather than treating close as an ordinary tab click.
            // https://www.sublimetext.com/docs/tab_multi-select.html
            let target_group = self.workbench.group_of(self.tabs[target].sheet);
            let selection = target_group.map(|g| self.workbench.groups[g].selected.clone());
            self.select(target);
            if let Some(group) = target_group {
                self.workbench.groups[group]
                    .selected
                    .extend(selection.unwrap_or_default());
            }
        }
        Ok(())
    }
    /// Reopens the most recently closed path with its selection, folds and viewport.
    pub fn reopen_last_file(&mut self) -> Result<bool, String> {
        while let Some((path, state)) = self.closed_files.pop() {
            // A deleted file is skipped, rather than reopened as a new named buffer.
            if matches!(path.try_exists(), Ok(false)) {
                continue;
            }
            self.open(FileLocation {
                path,
                line: 1,
                column: 1,
            })?;
            let tab = &mut self.tabs[self.active];
            tab.pending_center = false;
            tab.restore_scroll(&state);
            if tab.loading.is_some() || tab.binary_loading.is_some() {
                tab.pending_restore = Some(state);
            } else {
                state.apply(&mut tab.editor.buffer, tab.editor.id);
                tab.editor.restore_folded_view_state(&state, tab.editor.id);
                tab.editor.sync();
            }
            return Ok(true);
        }
        Ok(false)
    }
    /// Reopens a saved file with an explicit codec. Failed decoding leaves edits intact.
    // The Sublime changelogs document `revert` scroll position (dev build 4063),
    // not its undo history. Keep the existing confirmation before a disk reload.
    // https://www.sublimetext.com/dev#build-4063
    pub fn revert(&mut self, confirm: impl FnOnce() -> bool) -> Result<(), String> {
        let tab = &self.tabs[self.active];
        let path = tab.path.clone().ok_or("Revert requires a file on disk")?;
        if tab.dirty() && !confirm() {
            return Ok(());
        }
        let format = tab.format.clone();
        let options = fileio::DecodeOptions {
            encoding: Some(format.display_encoding().to_owned()),
            default_line_ending: format.line_ending,
            ..fileio::DecodeOptions::from_settings(&tab.editor.preferences)
        };
        self.pending_revert = Some(PendingRevert {
            sheet: tab.sheet,
            revision: tab.editor.revision,
            task: fileio::load_background_with_options(path.clone(), options)?,
            path,
            format,
        });
        Ok(())
    }
    pub fn open_stdin(&mut self, stdin: &[u8]) -> Result<u64, String> {
        // Builds 4113/4121/4200: detect syntax and indentation from stdin,
        // start with an empty undo stack, and don't translate Tab.
        // https://www.sublimetext.com/download
        let options = fileio::DecodeOptions::from_settings(&self.editor().preferences);
        let loaded = fileio::decode_bytes(stdin, &options)?;
        self.new_file()?;
        let tab = &mut self.tabs[self.active];
        tab.install_restored(loaded);
        tab.needs_detection = true;
        let sheet = tab.sheet;
        self.reconcile_installs();
        self.configure_language(self.active, None, true)?;
        self.editor_mut()
            .settings
            .set(Layer::View, "translate_tabs_to_spaces", Value::Bool(false));
        self.editor_mut().sync();
        self.stdout_sheets.insert(sheet);
        Ok(sheet)
    }
    fn poll_revert(&mut self) -> bool {
        let Some(pending) = self.pending_revert.as_ref() else {
            return false;
        };
        let result = match pending.task.result.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(error) => Err(error.to_string()),
        };
        let pending = self.pending_revert.take().unwrap();
        let Some(index) = self.sheet_index(pending.sheet) else {
            return false;
        };
        let tab = &mut self.tabs[index];
        if tab.path.as_ref() != Some(&pending.path)
            || tab.editor.revision != pending.revision
            || tab.format != pending.format
            || tab.editor.marked.is_some()
        {
            self.load_notices
                .push("Revert canceled: the tab changed while reading from disk".into());
            return true;
        }
        match result {
            Ok(mut loaded) => {
                loaded.format.line_ending = pending.format.line_ending;
                tab.external = None;
                tab.loading = None;
                tab.binary_loading = None;
                tab.restore_history = None;
                tab.install(loaded);
                tab.deleted = false;
                tab.restored_format_dirty = false;
                self.reconcile_installs();
            }
            Err(error) => self
                .load_notices
                .push(fileio::regular::error(&pending.path, error)),
        }
        true
    }
    pub fn reopen_encoding(&mut self, name: &str) -> Result<(), String> {
        let index = self.active;
        let tab = &self.tabs[index];
        let path = tab.path.as_deref().ok_or("Reopen requires a saved file")?;
        // The current command reference documents encoding arguments, but does
        // not confirm a Hexadecimal reopen value or menu entry. Keep existing
        // text-reopen behavior; automatic binary opens still use binary::Task.
        // https://docs.sublimetext.io/reference/commands.html#reopen
        let requested = fileio::encoding_format(name, tab.format.line_ending)?;
        let options = fileio::DecodeOptions {
            encoding: Some(name.into()),
            default_line_ending: tab.format.line_ending,
            ..fileio::DecodeOptions::from_settings(&tab.editor.preferences)
        };
        let (_, routing) = fileio::decoding_for_path(path, &options)?;
        let loaded = fileio::load_with_options(path, &options, |_| {});
        let prepared = match loaded {
            Ok(loaded) => Ok(loaded),
            Err(notice) if routing.binary.is_some() => {
                Err((crate::binary::Task::open(path.to_owned())?, notice))
            }
            Err(error) => return Err(error),
        };
        // A failed Reopen must preserve the choice, pending conflict, jobs and edits.
        self.tabs[index].loading = None;
        self.tabs[index].binary_loading = None;
        self.tabs[index].external = None;
        self.tabs[index].external_load = false;
        self.tabs[index].preferred_encoding = Some(name.into());
        self.tabs[index].encoding_chosen = true;
        match prepared {
            Ok(loaded) => self.tabs[index].install_restored(loaded),
            Err((task, notice)) => {
                let tab = &mut self.tabs[index];
                tab.load_warning = Some(notice);
                tab.restore_history = None;
                tab.editor.source_bytes = task.total;
                tab.binary_document = true;
                tab.binary_loading = Some(task);
                tab.editor.refresh_policy();
                tab.editor.read_only = true;
                tab.load_failed = false;
            }
        }
        self.tabs[index].format.display_name = requested.display_name.filter(|caption| {
            self.tabs[index].encoding_chosen
                && caption.ends_with("with BOM") == self.tabs[index].format.bom
        });
        self.tabs[index].saved_format = self.tabs[index].format.clone();
        self.tabs[index].deleted = false;
        self.tabs[index].restored_format_dirty = false;
        self.reconcile_installs();
        Ok(())
    }
    /// Automatic resource stamps are sampled at most every 500 ms. Explicit reloads,
    /// including completed package/settings writes, bypass the interval on the next poll.
    fn configuration_changed(&mut self, now: Instant) -> bool {
        if !self.reload_requested
            && self.last_config_check.is_some_and(|last| {
                now.saturating_duration_since(last) < Duration::from_millis(500)
            })
        {
            return false;
        }
        self.last_config_check = Some(now);
        self.reload_requested
            || self
                .config_stamp
                .iter()
                .any(|(path, saved)| stamp(path) != *saved)
    }
    /// Process watcher notifications and release deferred changes when their document
    /// becomes active. A visible clone can own the buffer without being the active sheet.
    pub(crate) fn apply_file_changes(&mut self, events: Vec<Result<Vec<PathBuf>, String>>) -> bool {
        self.apply_watch_changes(events, false)
    }

    fn apply_watch_changes(
        &mut self,
        events: Vec<Result<Vec<PathBuf>, String>>,
        rescan: bool,
    ) -> bool {
        let mut paths = std::collections::HashSet::new();
        for event in events {
            match event {
                Ok(changes) => paths.extend(changes),
                Err(e) => self.load_notices.push(e),
            }
        }
        let active_document = self.tabs[self.active].session_document;
        let mut changed = false;
        let mut failed = Vec::new();
        let mut close_deleted = Vec::new();
        for tab in &mut self.tabs {
            if tab.goto_preview
                || tab.loading.is_some()
                || tab.binary_loading.is_some()
                || tab.lazy.is_some()
                || tab.editor.shared_shadow
            {
                continue;
            }
            let Some(path) = tab.path.as_ref() else {
                continue;
            };
            let notified = rescan || paths.contains(path);
            if !notified && !tab.reload_deferred {
                continue;
            }
            let defer = tab.defer_external_reload(active_document);
            let ready = tab.reload_deferred && !defer;
            if !notified && !ready {
                continue;
            }
            let disk_stamp = stamp(path);
            if !ready && tab.disk_stamp == disk_stamp {
                continue;
            }
            tab.disk_stamp = disk_stamp;
            tab.reload_deferred = false;
            if matches!(path.try_exists(), Ok(false)) {
                if self.settings.bool("close_deleted_files", false) && !tab.dirty() {
                    close_deleted.push(tab.sheet);
                }
                tab.deleted = true;
                tab.transient = false;
                tab.external = None;
                changed = true;
                continue;
            }
            let metadata = match fileio::regular::metadata(path, false) {
                Ok(metadata) => metadata,
                Err(error) => {
                    self.resource_reports.push(error);
                    if !tab.dirty() {
                        failed.push(tab.sheet);
                    }
                    changed = true;
                    continue;
                }
            };
            if tab.deleted {
                tab.deleted = false;
                changed = true;
            }
            if defer {
                tab.reload_deferred = true;
                continue;
            }
            if tab.editor.binary.is_some() {
                match crate::binary::Task::open(path.clone()) {
                    Ok(task) => tab.binary_loading = Some(task),
                    Err(e) => self.load_notices.push(e),
                }
                continue;
            }
            let path = path.clone();
            let background = metadata.is_some_and(|m| {
                m.len()
                    >= tab
                        .editor
                        .settings
                        .usize("large_file.load_bytes", 64 * 1024 * 1024)
                        as u64
            });
            let workers = self.settings.usize("io_workers", 0);
            if let Err(error) = tab.load_disk(
                tab.decode_options(),
                background || workers != 0,
                true,
                Some((&self.io_workers, workers)),
            ) {
                if error == fileio::IO_QUEUE_FULL {
                    tab.reload_deferred = true;
                    continue;
                }
                self.resource_reports
                    .push(fileio::regular::error(&path, error));
                if !tab.dirty() {
                    failed.push(tab.sheet);
                }
            }
            changed = true;
        }
        for sheet in failed {
            if let Some(index) = self.sheet_index(sheet) {
                let _ = self.fail_shared_document(index);
            }
        }
        for sheet in close_deleted {
            if let Some(index) = self.sheet_index(sheet)
                && let Err(error) = self.close(index)
            {
                self.messages.push(error);
            }
        }
        changed
    }

    /// Polls jobs/notifications and reports whether scene content changed.
    pub fn poll(&mut self) -> bool {
        self.materialize_input();
        let enabled = self.settings.bool("show_git_status", true);
        let mut roots: Vec<_> = self.folders.iter().map(|f| f.path.clone()).collect();
        for tab in &self.tabs {
            if let Some(parent) = tab.path.as_deref().and_then(Path::parent)
                && !roots.iter().any(|root| parent.starts_with(root))
            {
                roots.push(parent.to_owned());
            }
        }
        let mut changed = self.poll_revert();
        changed |= self.poll_appearance_commits();
        changed |= self.git.poll(enabled, roots);
        if changed {
            for tab in &mut self.tabs {
                if !tab.editor.shared_shadow {
                    tab.editor.annotations.borrow_mut().refresh_reference();
                }
            }
        }
        let mut failed_documents = Vec::new();
        for tab in &mut self.tabs {
            if tab.lazy.is_some() || tab.editor.shared_shadow {
                continue;
            }
            changed |= tab.editor.poll_language();
            changed |= tab.editor.annotations.borrow_mut().poll();
            if let Some(error) = tab
                .editor
                .annotations
                .borrow_mut()
                .current
                .as_mut()
                .and_then(|p| p.error.take())
            {
                // Diff references also load the document on a worker. A failed
                // guarded read is the same non-modal notice as a failed reload.
                if tab
                    .path
                    .as_ref()
                    .is_some_and(|path| error.starts_with(&format!("{}: ", path.display())))
                {
                    self.resource_reports.push(error);
                } else {
                    self.messages.push(error);
                }
            }
            changed |= tab.editor.poll_completion();
            if let Some(language) = tab.editor.language.borrow_mut().as_mut() {
                self.messages.append(&mut language.errors);
            }
            // Promotion and recent-file bookkeeping are applied after shared buffers sync.
            if let Some(message) = tab.load_warning.take() {
                self.load_notices.push(message);
                changed = true;
            }
            if let Some(snapshot) = tab.loading.as_ref().and_then(LoadTask::try_snapshot)
                && !tab.external_load
            {
                let source_bytes = tab.loading.as_ref().map_or(0, |t| t.total);
                let pending_location = tab.location.take();
                tab.install(Loaded {
                    disk_content: None,
                    encoding_chosen: false,
                    unchanged: false,
                    warning: None,
                    text: snapshot.text,
                    format: tab.format.clone(),
                    source_bytes,
                });
                tab.location = pending_location;
                tab.editor.source_bytes = source_bytes;
                tab.editor.document_metrics = snapshot.metrics;
                tab.editor.read_only = true;
                tab.editor.refresh_policy();
                record_telemetry(
                    &mut self.telemetry,
                    self.telemetry_enabled,
                    || serde_json::json!({"kind":"file_open","phase":"prefix","elapsed_ms":tab.load_started.map(|s|s.elapsed().as_secs_f64()*1000.),"loaded_bytes":tab.editor.text.len_bytes(),"source_bytes":source_bytes}),
                );
                changed = true;
            }
            if let Some(result) =
                tab.binary_loading
                    .as_ref()
                    .and_then(|t| match t.result.try_recv() {
                        Ok(result) => Some(result),
                        Err(std::sync::mpsc::TryRecvError::Empty) => None,
                        Err(e) => Some(Err(e.to_string())),
                    })
            {
                tab.binary_loading = None;
                match result {
                    Ok(snapshot) => {
                        *tab.editor.language.borrow_mut() = None;
                        tab.editor.binary = Some(snapshot);
                        tab.binary_document = true;
                        tab.editor.read_only = true;
                        tab.editor.caret = 0;
                        tab.editor.anchor = 0;
                        tab.pending_restore = None;
                        tab.needs_detection = false;
                        tab.disk_stamp = tab.path.as_deref().and_then(stamp);
                    }
                    Err(e) => {
                        self.resource_reports.push(
                            tab.path
                                .as_deref()
                                .map_or_else(|| e.clone(), |p| fileio::regular::error(p, &e)),
                        );
                        if !tab.dirty() {
                            failed_documents.push(tab.sheet);
                        }
                    }
                }
                changed = true;
            }
            let result = tab
                .loading
                .as_ref()
                .and_then(|t| match t.result.try_recv() {
                    Ok(v) => Some(v),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(e) => Some(Err(format!("file worker disconnected: {e}"))),
                });
            if let Some(result) = result {
                record_telemetry(
                    &mut self.telemetry,
                    self.telemetry_enabled,
                    || serde_json::json!({"kind":"file_open","phase":"fully_loaded","elapsed_ms":tab.load_started.map(|s|s.elapsed().as_secs_f64()*1000.),"source_bytes":tab.loading.as_ref().map(|t|t.total),"path":tab.path,"success":result.is_ok(),"background":true}),
                );
                tab.load_started = None;
                let binary = tab.loading.as_ref().is_some_and(LoadTask::binary);
                tab.loading = None;
                let read_from = tab.read_from.take();
                let external = tab.external_load;
                match result {
                    Ok(loaded) => {
                        if external {
                            tab.accept_reload(loaded);
                        } else {
                            tab.install(loaded);
                        }
                        if let Some(before) = read_from {
                            tab.verify_read(before);
                        }
                    }
                    Err(error) if binary => {
                        if let Err(error) = tab.load_hex(error, external) {
                            self.load_notices.push(error);
                        }
                    }
                    Err(error) => {
                        self.resource_reports.push(tab.path.as_deref().map_or_else(
                            || error.clone(),
                            |path| fileio::regular::error(path, &error),
                        ));
                        if !tab.dirty() {
                            tab.failed_load(error);
                            failed_documents.push(tab.sheet);
                        }
                    }
                }
                tab.external_load = false;
                changed = true;
            }
        }
        let (events, rescan) = self
            .watcher
            .as_mut()
            .map_or_else(Default::default, |watcher| {
                watcher.reconcile(
                    self.tabs
                        .iter()
                        .filter(|tab| !tab.goto_preview && tab.lazy.is_none())
                        .filter_map(|tab| tab.path.as_deref()),
                    self.folders.iter().map(|folder| folder.path.as_path()),
                );
                watcher.take_changes()
            });
        for sheet in failed_documents {
            if let Some(index) = self.sheet_index(sheet) {
                let _ = self.fail_shared_document(index);
            }
        }
        changed |= self.apply_watch_changes(events, rescan);
        self.reconcile_installs();
        let had_pending_writes = !self.settings_writes.is_empty();
        let mut settled_settings = false;
        self.settings_writes
            .retain(|(rx, generation)| match rx.try_recv() {
                Ok(Ok(())) => {
                    settled_settings = true;
                    false
                }
                Ok(Err(error)) => {
                    settled_settings = true;
                    self.settings_write_overrides
                        .retain(|_, (id, _)| id != generation);
                    self.resource_reports.push(error);
                    false
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => true,
                Err(error) => {
                    settled_settings = true;
                    self.settings_write_overrides
                        .retain(|_, (id, _)| id != generation);
                    self.messages.push(error.to_string());
                    false
                }
            });
        if settled_settings {
            // Publish one settled load for a batch, not each intermediate file.
            if self.settings_writes.is_empty() {
                self.reload_requested = true;
            } else {
                self.settings_reload_deferred = true;
            }
        }
        if had_pending_writes
            && self.settings_writes.is_empty()
            && std::mem::take(&mut self.settings_reload_deferred)
        {
            self.reload_requested = true;
        }
        if self.config_rx.is_none() && self.configuration_changed(Instant::now()) {
            self.reload_requested = false;
            self.config_write_generation = (
                self.settings_write_generation,
                !self.settings_writes.is_empty(),
            );
            let (tx, rx) = std::sync::mpsc::channel();
            let data = self.data_dir.clone();
            let project = self.project.clone();
            let overrides = self.settings.layer(Layer::Window).clone();
            let preferred_languages = self.preferred_languages;
            let kind = if self.packed_syntaxes {
                ConfigKind::InteractiveReload
            } else {
                ConfigKind::Eager
            };
            match std::thread::Builder::new()
                .name("velocity-resources".into())
                .spawn(move || {
                    let _ = tx.send(Configuration::load_with_window_overrides(
                        data,
                        project,
                        Some(overrides),
                        preferred_languages,
                        kind,
                        &mut Default::default(),
                    ));
                }) {
                Ok(_) => self.config_rx = Some(rx),
                Err(error) => self.messages.push(error.to_string()),
            }
        }
        let config_result = self.config_rx.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(load) => Some((load.result, Some(load.stamps))),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(error) => Some((Err(error.to_string()), None)),
        });
        if let Some((result, stamps)) = config_result {
            self.config_rx = None;
            let project_changed = self.project.as_ref().is_some_and(|project| {
                self.config_stamp
                    .iter()
                    .any(|(path, saved)| path == project && stamp(path) != *saved)
            });
            if let Some(stamps) = stamps {
                self.config_stamp = stamps;
            }
            let folders = self
                .folders
                .iter()
                .filter(|f| !project_changed || self.transient_folders.contains(&f.path))
                .cloned()
                .collect::<Vec<_>>();
            match result {
                Ok(mut config) => {
                    // A resource worker may have read an intermediate result from our
                    // serial preferences queue. Preserve only our newer keys until a
                    // load started after that generation drained; unrelated/external
                    // edits still publish, and the next settled load is authoritative.
                    let (generation, pending_at_start) = self.config_write_generation;
                    if generation != self.settings_write_generation
                        || pending_at_start
                        || !self.settings_writes.is_empty()
                    {
                        let mut values = config.settings.layer(Layer::User).clone();
                        for (key, (_, value)) in &self.settings_write_overrides {
                            if let Some(value) = value {
                                values.insert(key.clone(), value.clone());
                            } else {
                                values.remove(key);
                            }
                        }
                        let _ = config.settings.replace(Layer::User, Value::Object(values));
                        if self.settings_writes.is_empty() {
                            self.reload_requested = true;
                        } else {
                            // The global queue can be blocked by installs. Keep the
                            // overlay, allow external changes, and retry once at drain.
                            self.settings_reload_deferred = true;
                        }
                    } else {
                        self.settings_write_overrides.clear();
                    }
                    self.messages.append(&mut config.messages);
                    report_ignored_keymaps(
                        &self.ignored_user_keymaps,
                        &config.ignored_user_keymaps,
                        &mut config.resource_reports,
                    );
                    self.ignored_user_keymaps = config.ignored_user_keymaps;
                    self.resource_reports.append(&mut config.resource_reports);
                    self.themes = crate::theme_ui::Themes::publish(config.themes);
                    if let Err(e) = self.themes.select(&config.settings, self.dark) {
                        self.messages.push(e);
                    }
                    self.resource_revision = self.resource_revision.wrapping_add(1);
                    self.assets = config.assets;
                    self.catalog = config.catalog;
                    self.syntax_settings = config.syntax_settings;
                    self.schemes = config.schemes;
                    self.appearance = config.appearance;
                    self.folders = config.folders;
                    for folder in &self.folders {
                        self.transient_folders.remove(&folder.path);
                    }
                    self.distraction_settings = config.distraction_settings;
                    self.build_systems = config.build_systems;
                    self.project_model = config.project_model;
                    self.settings = config.settings;
                    self.translations.replace(config.translations);
                    self.sync_ui_language();
                    if let Err(error) = self.refresh_window_layers() {
                        self.messages.push(error);
                    }
                    // User resource changes must preserve folders added at runtime. An
                    // actual project-file edit still replaces its configured folder list.
                    for mut folder in folders {
                        if self
                            .folders
                            .iter()
                            .any(|f| vel_config::paths::equal(&f.path, &folder.path))
                        {
                            continue;
                        }
                        let patterns = |key| {
                            self.settings
                                .get(key)
                                .and_then(Value::as_array)
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .collect::<Vec<_>>()
                        };
                        match vel_search::project::Rules::new(
                            &patterns("file_exclude_patterns"),
                            &patterns("folder_exclude_patterns"),
                            &patterns("binary_file_patterns"),
                        ) {
                            Ok(rules) => folder.rules = rules,
                            Err(error) => self.messages.push(error),
                        }
                        self.folders.push(folder);
                    }
                    self.keymap = config.keymap;
                    self.menus = config.menus;
                    self.context_menus = config.context_menus;
                    self.widget_context_menu = config.widget_context_menu;
                    self.mousemap = config.mousemap;
                    self.scrollbar_mousemap = config.scrollbar_mousemap;
                    if self.palette != config.palette {
                        self.palette_revision = self.palette_revision.wrapping_add(1);
                    }
                    self.palette = config.palette;
                    self.startup_palette = None;
                    self.macro_paths = config.macro_paths;
                    self.resources = config.resources;
                    for tab in &mut self.tabs {
                        for layer in [
                            Layer::Default,
                            Layer::Platform,
                            Layer::User,
                            Layer::Window,
                            Layer::Project,
                        ] {
                            let values = self.settings.layer(layer).clone();
                            let _ = tab.editor.settings.replace(layer, Value::Object(values));
                        }
                        tab.editor.sync();
                        tab.language_settings_revision = u64::MAX;
                        // Resource reloads preserve the buffer's detected/manual indentation.
                        tab.editor.keymap = self.keymap.clone();
                    }
                    changed = true;
                }
                Err(e) => self.messages.push(e),
            }
        }
        for index in 0..self.tabs.len() {
            if self.tabs[index].lazy.is_some() {
                continue;
            }
            if self.tabs[index].needs_detection
                || self.tabs[index].language_settings_revision
                    != self.tabs[index].editor.settings.revision()
            {
                if let Err(e) =
                    self.configure_language(index, None, self.tabs[index].needs_detection)
                {
                    self.messages.push(e);
                    self.tabs[index].language_settings_revision =
                        self.tabs[index].editor.settings.revision();
                }
                changed = true;
            }
            if self.tabs[index].loading.is_none()
                && self.tabs[index]
                    .editor
                    .language
                    .borrow()
                    .as_ref()
                    .is_some_and(|l| l.name == "Plain Text")
                && self.tabs[index].path.as_ref().is_some_and(|p| {
                    vel_highlight::detect_syntax(
                        &self.assets.syntaxes,
                        &p.to_string_lossy(),
                        &crate::large::first_line(&self.tabs[index].editor.text),
                        None,
                    )
                    .is_some()
                })
                && self.tabs[index].editor.settings.get("syntax").is_none()
            {
                if let Err(e) = self.configure_language(index, None, true) {
                    self.messages.push(e);
                }
                changed = true;
            }
        }
        changed
    }
    pub fn set_file_type(&mut self, syntax: &str) -> Result<(), String> {
        self.configure_language(self.active, Some(syntax), false)
    }
    pub(crate) fn adopt_transferred_settings(&self, tab: &mut Tab) {
        for layer in [
            Layer::Default,
            Layer::Platform,
            Layer::User,
            Layer::Window,
            Layer::Project,
            Layer::Syntax,
            Layer::DistractionFree,
        ] {
            let values = self.settings.layer(layer).clone();
            let _ = tab.editor.settings.replace(layer, Value::Object(values));
        }
        tab.editor.theme = self.themes.ui.clone();
        tab.editor.keymap = self.keymap.clone();
        tab.editor.snippets = self.catalog.snippets.clone();
        tab.language_settings_revision = u64::MAX;
    }
    pub fn set_appearance(&mut self, dark: bool) -> Result<(), String> {
        self.dark = dark;
        if let Err(e) = self.themes.select(&self.settings, dark) {
            self.messages.push(e);
        }
        for index in 0..self.tabs.len() {
            self.configure_language(index, None, false)?;
        }
        if self.appearance_preview.is_some() {
            self.recolor_appearance()?;
        }
        Ok(())
    }
    pub(crate) fn configure_language(
        &mut self,
        index: usize,
        explicit: Option<&str>,
        detect: bool,
    ) -> Result<(), String> {
        if self.tabs[index].lazy.is_none() {
            let tab = &self.tabs[index];
            tab.editor.annotations.borrow_mut().set_location(
                tab.path.clone(),
                self.data_dir.as_ref().map(|d| d.join("Packages")),
            );
        }
        let tab = &mut self.tabs[index];
        tab.editor.theme = self.themes.ui.clone();
        tab.editor.translations = self.translations.clone();
        if tab.loading.is_some() || tab.binary_loading.is_some() || tab.editor.binary.is_some() {
            *tab.editor.language.borrow_mut() = None;
            tab.needs_detection = false;
            tab.language_settings_revision = tab.editor.settings.revision();
            return Ok(());
        }
        tab.editor.snippets = self.catalog.snippets.clone();
        tab.editor.snippet_path = tab.path.clone();
        let previous = tab
            .editor
            .language
            .borrow()
            .as_ref()
            .map(|l| l.syntax.clone());
        let setting_override = tab.editor.settings.get("syntax").and_then(Value::as_str);
        let detection_limit = tab.editor.settings.usize("syntax_detection_size_limit", 0) as u64;
        let autodetect = detection_limit == 0
            || tab
                .editor
                .source_bytes
                .max(tab.editor.text.len_bytes() as u64)
                <= detection_limit;
        let chosen = explicit
            .or(setting_override)
            .or(if detect { None } else { previous.as_deref() })
            .or_else(|| {
                autodetect
                    .then(|| {
                        extension_syntax(
                            &self.assets.syntaxes,
                            &self.syntax_settings,
                            tab.path.as_deref(),
                        )
                    })
                    .flatten()
            });
        let reference = (autodetect || chosen.is_some())
            .then(|| {
                vel_highlight::detect_syntax(
                    &self.assets.syntaxes,
                    &crate::language::syntax_path(tab.path.as_deref()),
                    &if chosen.is_none() {
                        crate::large::first_line(&tab.editor.text)
                    } else {
                        String::new()
                    },
                    chosen,
                )
            })
            .flatten()
            .or_else(|| {
                explicit
                    .is_none()
                    .then(|| self.assets.syntaxes.find_syntax_plain_text())
            })
            .ok_or("Unknown syntax")?;
        let syntax = reference.path.clone();
        let name = reference.name.clone();
        if explicit.is_some() {
            tab.editor
                .settings
                .set(Layer::View, "syntax", serde_json::json!(syntax));
        }
        let values = self.syntax_settings.values(&syntax, &name);
        if tab.editor.settings.layer(Layer::Syntax) != &values {
            tab.editor
                .settings
                .replace(Layer::Syntax, Value::Object(values))?;
        }
        if detect && tab.lazy.is_none() && tab.needs_detection {
            if tab.editor.settings.bool("detect_indentation", true)
                && let Some(indentation) = crate::language::detect_indentation(&tab.editor.text)
            {
                indentation.apply(&mut tab.editor);
            }
            // A load with detection disabled also consumes the one load-time attempt.
            tab.needs_detection = false;
            tab.indentation_initialized = true;
        }
        tab.editor.sync();
        let settings = Value::Object(tab.editor.settings.effective());
        let selected = vel_color_scheme::selected_scheme(&settings, self.dark);
        let basename = selected.rsplit('/').next().unwrap_or(selected);
        let scheme = self
            .schemes
            .get(basename)
            .or_else(|| {
                self.schemes
                    .get(&format!("{basename}.sublime-color-scheme"))
            })
            .cloned()
            .or_else(|| {
                self.schemes
                    .get(if self.dark {
                        "Velocity Dark.sublime-color-scheme"
                    } else {
                        "Velocity Light.sublime-color-scheme"
                    })
                    .cloned()
            })
            .ok_or_else(|| format!("Unknown color scheme {selected}"))?;
        if previous.as_deref() != Some(syntax.as_str())
            || !tab
                .editor
                .language
                .borrow()
                .as_ref()
                .is_some_and(|l| Arc::ptr_eq(&l.assets, &self.assets))
        {
            tab.editor
                .attach_language(self.assets.clone(), &syntax, scheme)?;
        } else if let Some(language) = tab.editor.language.borrow_mut().as_mut() {
            language.recolor(
                scheme,
                tab.editor.text.clone(),
                tab.editor.preferences.tab_size,
            );
        }
        tab.language_settings_revision = tab.editor.settings.revision();
        Ok(())
    }
    /// A confirmed disk replacement is a normal edit in open, unchanged buffers.
    pub(crate) fn accept_file_replacement(
        &mut self,
        change: &vel_search::files::FileChange,
        identity: fileio::ContentIdentity,
    ) {
        let path = &change.path;
        for tab in &mut self.tabs {
            if tab.path.as_ref() != Some(path)
                || tab.dirty()
                || tab.editor.read_only
                || tab.loading.is_some()
                || tab.lazy.is_some()
                || tab.editor.buffer.text() != change.before()
                || tab.editor.buffer.id() != tab.document
            {
                continue;
            }
            vel_search::text::apply(&mut tab.editor.buffer, tab.editor.id, change.edits());
            tab.editor.buffer.mark_saved();
            tab.editor.sync();
            tab.tracker.mark_saved(&tab.editor.buffer);
            tab.disk_content = Some(identity);
            tab.disk_stamp = stamp(path);
            tab.disk_saved = vel_session::DiskStamp::read(path);
            tab.external = None;
            tab.overwrite_allowed = None;
            tab.editor.annotations.borrow_mut().reference(
                tab.editor.buffer.text().clone(),
                tab.path.clone(),
                self.data_dir.as_ref().map(|d| d.join("Packages")),
            );
        }
        self.sync_shared();
    }
    pub fn reload_external(&mut self, index: usize, reload: bool) {
        if let Some(loaded) = self.tabs[index].external.take() {
            if reload {
                self.tabs[index].install(loaded);
                self.reconcile_installs();
                let _ = self.prepare_sheet(self.active);
            } else {
                let tab = &mut self.tabs[index];
                tab.disk_content = loaded.disk_content;
                tab.overwrite_allowed = None;
                tab.saved_format = loaded.format;
                tab.editor.buffer.invalidate_saved_revision();
                tab.editor.sync();
                tab.legacy_stamp_matches = false;
                tab.legacy_saved_text = None;
                tab.legacy_text_fingerprint = None;
                tab.disk_saved = tab.path.as_deref().and_then(vel_session::DiskStamp::read);
                tab.disk_stamp = tab.path.as_deref().and_then(stamp);
            }
        }
    }
    pub fn default_data_dir() -> Option<PathBuf> {
        #[cfg(windows)]
        return vel_config::paths::windows_data_dir(
            std::env::var_os("VELOCITY_DATA_DIR").as_deref(),
            std::env::var_os("APPDATA").as_deref(),
        );
        #[cfg(not(windows))]
        std::env::var_os("VELOCITY_DATA_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join("Library/Application Support/Velocity"))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_keys::{self, chord};
    use vel_config::{json, keymap::Resolution};

    const QUICK_ADD_NEXT: &str = chord("super+d", "ctrl+d");

    #[test]
    fn windows_default_shortcuts_resolve_with_their_expected_arguments() {
        use vel_config::keymap::{Invocation, Key, SettingsContext};
        let settings = Settings::default();
        let context = SettingsContext(&settings);
        let mut keys = Keymap::windows();
        keys.append(include_str!(
            "../resources/Packages/Default/Default (Windows).sublime-keymap"
        ))
        .unwrap();
        let registry = Editor::command_registry();
        for (sequence, command, args) in [
            (&["ctrl+e"][..], "slurp_find_string", json!({})),
            (&["ctrl+shift+e"], "slurp_replace_string", json!({})),
            (
                &["alt+o"],
                "switch_file",
                json!({"extensions":["h","cpp","c","cc","mm","m"]}),
            ),
            (&["ctrl+up"], "scroll_lines", json!({"amount":1})),
            (&["ctrl+down"], "scroll_lines", json!({"amount":-1})),
            (&["ctrl+0"], "focus_side_bar", json!({})),
            (&["ctrl+g"], "goto_line", json!({})),
            (&["ctrl+d"], "find_under_expand", json!({})),
            (&["alt+1"], "select_by_index", json!({"index":0})),
            (&["alt+9"], "select_by_index", json!({"index":-1})),
            (&["ctrl+1"], "focus_group", json!({"group":0})),
            (&["ctrl+shift+1"], "move_to_group", json!({"group":0})),
            (&["ctrl+y"], "redo_or_repeat", json!({})),
            (&["ctrl+shift+z"], "redo", json!({})),
            (&["ctrl+k", "ctrl+k"], "delete_to_hard_eol", json!({})),
            (&["ctrl+k", "ctrl+s"], "save_all", json!({})),
            (&["ctrl+k", "ctrl+b"], "toggle_side_bar", json!({})),
            (&["ctrl+k", "ctrl+="], "reset_font_size", json!({})),
            (&["alt+-"], "jump_back", json!({})),
            (&["alt+shift+-"], "jump_forward", json!({})),
            (&["alt+."], "close_tag", json!({})),
        ] {
            assert!(registry.kind(command).is_some(), "{command}");
            let (last, prefix) = sequence.split_last().unwrap();
            for key in prefix {
                assert_eq!(
                    keys.resolve(&Key::parse_windows(key).unwrap(), &context),
                    Resolution::Pending,
                    "{sequence:?}"
                );
            }
            assert_eq!(
                keys.resolve(&Key::parse_windows(last).unwrap(), &context),
                Resolution::Command(Invocation {
                    command: command.into(),
                    args
                }),
                "{sequence:?}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_keymap_defaults_and_user_override() {
        use vel_config::keymap::{Key, Resolution, SettingsContext};
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("profile");
        let mut defaults = Workspace::new_empty(Some(profile.clone()), None, false).unwrap();
        fn expects(w: &mut Workspace, key: &str, command: &str, args: Value) {
            assert_eq!(
                w.keymap.resolve(
                    &Key::parse_windows(key).unwrap(),
                    &SettingsContext(&w.settings)
                ),
                Resolution::Command(vel_config::keymap::Invocation {
                    command: command.into(),
                    args
                }),
                "{key}"
            );
        }
        for (key, command) in [
            ("f3", "find_next"),
            ("f11", "toggle_full_screen"),
            ("ctrl+g", "goto_line"),
            ("ctrl+q", "exit"),
            ("ctrl+tab", "next_view_in_stack"),
            ("ctrl+f6", "next_window"),
            ("ctrl+shift+f6", "prev_window"),
        ] {
            expects(&mut defaults, key, command, serde_json::json!({}));
        }
        expects(
            &mut defaults,
            "ctrl+p",
            "show_overlay",
            serde_json::json!({"overlay":"goto"}),
        );
        expects(
            &mut defaults,
            "ctrl+shift+p",
            "show_overlay",
            serde_json::json!({"overlay":"command_palette"}),
        );
        let prefs = defaults
            .menus
            .iter()
            .find(|m| m.id == "preferences")
            .unwrap();
        assert!(prefs.children.iter().any(
            |row| row.args["user_file"] == "${packages}/User/Default (Windows).sublime-keymap"
        ));
        let user = profile.join("Packages/User");
        std::fs::create_dir_all(&user).unwrap();
        for (name, command) in [
            ("Default.sublime-keymap", "paste"),
            ("Default (OSX).sublime-keymap", "copy"),
            ("Default (Windows).sublime-keymap", "save_all"),
        ] {
            std::fs::write(
                user.join(name),
                format!(r#"[{{"keys":["ctrl+p"],"command":"{command}"}}]"#),
            )
            .unwrap();
        }
        let mut overridden = Workspace::new_empty(Some(profile), None, false).unwrap();
        expects(&mut overridden, "ctrl+p", "save_all", serde_json::json!({}));
    }
    #[test]
    fn first_frame_uses_profile_language_scheme_and_keymap() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("Packages/User");
        std::fs::create_dir_all(user.join("Localization")).unwrap();
        std::fs::write(
            user.join("Preferences.sublime-settings"),
            "{\r\n  // Prefer the OS UI language\r\n  \"ui_language\":\"auto\",\r\n  \"color_scheme\":\"Velocity Light.sublime-color-scheme\",\r\n}\r\n",
        )
        .unwrap();
        std::fs::write(
            user.join(PLATFORM_KEYMAP),
            format!("[\r\n  // User binding\r\n  {{\"keys\":[\"{QUICK_ADD_NEXT}\"],\"command\":\"insert\",\"args\":{{\"characters\":\"first\"}}}},\r\n]\r\n"),
        )
        .unwrap();
        std::fs::write(
            user.join("Localization/zh-Hans.sublime-localization"),
            "{\r\n  // User translation\r\n  \"language_name\":\"简体中文\",\r\n  \"strings\":{\"Where…\":\"在此位置…\",},\r\n}\r\n",
        )
        .unwrap();
        let preferred = crate::ui_language::PreferredLanguages(&["zh-Hans-CN", "en"]);
        let mut startup = Workspace::new_with_configuration(
            None,
            Some(dir.path().into()),
            None,
            (true, ConfigKind::FirstFrame),
            Some(Default::default()),
            preferred,
            &mut Default::default(),
        )
        .unwrap();
        assert!(startup.startup_services_pending() && startup.palette_pending());
        assert_eq!(startup.ui_language_setting(), "auto");
        assert_eq!(startup.translations.matched_language().as_str(), "zh-Hans");
        assert_eq!(startup.translations.tr("Where…"), "在此位置…");
        assert!(!startup.themes.ui.borrow().palette.is_dark());
        assert!(
            startup
                .editor_mut()
                .key(&test_keys::parse(QUICK_ADD_NEXT))
                .unwrap()
        );
        assert_eq!(startup.editor().text.to_string(), "first");

        let eager = Workspace::new_with_preferred_languages(
            Text::new(),
            Some(dir.path().into()),
            false,
            &["zh-Hans-CN", "en"],
        )
        .unwrap();
        assert_eq!(
            startup.assets.syntaxes.warnings(),
            eager.assets.syntaxes.warnings()
        );
        let before = startup.palette_revision;
        startup.finish_startup_palette();
        assert_eq!(startup.palette, eager.palette);
        assert_eq!(startup.palette_revision, before + 1);
        startup.finish_startup_palette();
        assert_eq!(startup.palette_revision, before + 1);
    }

    #[test]
    fn startup_watcher_reconciles_changes_before_registration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("watch.txt");
        std::fs::write(&path, "old").unwrap();
        let mut startup =
            Workspace::new_for_startup(Text::new(), None, None, true, &mut Default::default())
                .unwrap();
        startup
            .open(FileLocation {
                path: path.clone(),
                line: 1,
                column: 1,
            })
            .unwrap();
        assert!(startup.watcher.is_none());
        std::fs::write(path, "changed before present").unwrap();
        assert!(startup.start_startup_watcher());
        assert!(startup.watcher.is_some());
        startup.poll();
        assert_eq!(startup.editor().text.to_string(), "changed before present");
        assert!(!startup.start_startup_watcher());
    }

    #[test]
    fn closing_an_earlier_tab_keeps_the_active_tab() {
        let mut workspace = Workspace::new(Text::from("a"), None, None, false).unwrap();
        for text in ["b", "c", "d"] {
            workspace.new_file().unwrap();
            workspace
                .editor_mut()
                .run("insert", &json!({"characters": text}))
                .unwrap();
        }
        workspace.select(2);
        assert_eq!(workspace.editor().text.to_string(), "c");
        workspace.close(0).unwrap();
        assert_eq!(workspace.active, 1);
        assert_eq!(workspace.editor().text.to_string(), "c");
        // Closing a later tab leaves the active one alone too.
        workspace.close(2).unwrap();
        assert_eq!(workspace.editor().text.to_string(), "c");
    }
    #[test]
    fn tabs_cli_positions_edit_save_and_format_dirty_state() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "a\r\n你好\r\n").unwrap();
        std::fs::write(&b, b"caf\xe9").unwrap();
        let mut workspace = Workspace::new(Text::new(), None, None, false).unwrap();
        workspace
            .open(FileLocation {
                path: a.clone(),
                line: 2,
                column: 2,
            })
            .unwrap();
        assert_eq!(workspace.tabs.len(), 1);
        assert_eq!(workspace.editor().caret, 5);
        assert_eq!(
            workspace.tabs[0].format.line_ending,
            fileio::LineEnding::CrLf
        );
        workspace
            .editor_mut()
            .run("insert", &json!({"characters":"!"}))
            .unwrap();
        assert!(workspace.tabs[0].dirty());
        workspace.save(0, None).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), "a\r\n你!好\r\n".as_bytes());
        assert!(!workspace.tabs[0].dirty());
        workspace
            .open(FileLocation {
                path: b,
                line: 1,
                column: 1,
            })
            .unwrap();
        assert_eq!(workspace.tabs.len(), 2);
        assert_eq!(workspace.editor().text.to_string(), "café");
        workspace.select(0);
        workspace.editor_mut().run("undo", &json!({})).unwrap();
        assert!(workspace.tabs[0].dirty());
        assert_eq!(workspace.editor().text.to_string(), "a\n你好\n");
        workspace.tabs[0].format.line_ending = fileio::LineEnding::Lf;
        workspace.close(1).unwrap();
        assert_eq!(workspace.active, 0);
        workspace.new_file().unwrap();
        assert!(workspace.tabs[1].path.is_none());
        workspace.close(1).unwrap();
        workspace.close(0).unwrap();
        assert_eq!(workspace.tabs.len(), 1);
    }
    #[test]
    fn titles_follow_active_document_dirty_project_and_deleted_state() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("文書.rs");
        std::fs::write(&file, "saved").unwrap();
        let project = dir.path().join("Example.sublime-project");
        std::fs::write(&project, "{}").unwrap();
        let mut workspace = Workspace::new(Text::new(), None, None, false).unwrap();
        assert_eq!(workspace.window_title(), "Untitled");
        workspace
            .open(FileLocation {
                path: file,
                line: 1,
                column: 1,
            })
            .unwrap();
        assert_eq!(workspace.window_title(), "文書.rs");
        workspace
            .editor_mut()
            .run("insert", &json!({"characters":"changed "}))
            .unwrap();
        assert_eq!(workspace.window_title(), "文書.rs •");
        workspace.project = Some(project);
        assert_eq!(workspace.window_title(), "文書.rs • — Example");
        workspace.tabs[0].deleted = true;
        assert_eq!(workspace.tabs[0].label(), "文書.rs [deleted] •");
        assert_eq!(workspace.window_title(), "文書.rs [deleted] • — Example");
        workspace.save(0, None).unwrap();
        assert!(!workspace.tabs[0].deleted);
        assert_eq!(workspace.window_title(), "文書.rs — Example");
        workspace.new_file().unwrap();
        assert_eq!(workspace.window_title(), "Untitled — Example");
    }
    #[test]
    fn deleted_clean_files_are_dirty_and_retain_contents_across_session_restore() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let file = root.join("clean.txt");
        let contents = "clean contents\nretained after trash 你好\n";
        std::fs::write(&file, contents).unwrap();
        let mut workspace = Workspace::new(Text::new(), None, None, false).unwrap();
        workspace
            .open(FileLocation {
                path: file.clone(),
                line: 1,
                column: 1,
            })
            .unwrap();
        workspace.clone_file().unwrap();
        assert!(workspace.tabs.iter().all(|tab| !tab.dirty()));
        assert!(!workspace.editor().buffer.is_dirty());
        std::fs::remove_file(&file).unwrap();
        crate::context_menu::apply_delete(&mut workspace, std::slice::from_ref(&file));
        // Cloning after deletion must retain the same missing-file state too.
        workspace.clone_file().unwrap();
        assert!(workspace.tabs.iter().all(|tab| tab.deleted && tab.dirty()));
        assert!(!workspace.editor().buffer.is_dirty());
        assert!(
            workspace
                .tabs
                .iter()
                .all(|tab| tab.label() == "clean.txt [deleted] •")
        );
        assert_eq!(workspace.window_title(), "clean.txt [deleted] •");
        let (window, documents) = workspace.snapshot().unwrap();
        assert_eq!(documents.len(), 1);
        assert!(documents[0].dirty);
        assert!(documents[0].history.as_ref().unwrap().retains_text());
        let mut session = vel_session::Session {
            windows: vec![window],
            documents,
            ..Default::default()
        };
        session.apply_policy(vel_session::HotExit::Always, false, true);
        let session: vel_session::Session =
            serde_json::from_slice(&serde_json::to_vec(&session).unwrap()).unwrap();
        session.validate().unwrap();
        let mut restored = Workspace::new(Text::new(), None, None, false).unwrap();
        restored
            .restore_window(&session.windows[0], &session.documents)
            .unwrap();
        assert!(restored.tabs.iter().any(|tab| tab.lazy.is_some()));
        for index in 0..restored.tabs.len() {
            restored.prepare_sheet(index).unwrap();
            assert!(restored.tabs[index].deleted);
            assert!(restored.tabs[index].dirty());
            assert_eq!(restored.tabs[index].label(), "clean.txt [deleted] •");
            assert_eq!(restored.tabs[index].editor.text, contents);
        }
        assert!(!file.exists());
        let (_, retained) = restored.snapshot().unwrap();
        assert_eq!(retained.len(), 1);
        assert!(retained[0].dirty);
        assert_eq!(
            retained[0]
                .history
                .as_ref()
                .unwrap()
                .current_text()
                .unwrap(),
            contents
        );
    }
    #[test]
    fn deleted_file_save_clears_shared_and_lazy_markers_including_save_as() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let original = root.join("original.txt");
        let replacement = root.join("replacement.txt");
        let contents = "saved contents\n";
        std::fs::write(&original, contents).unwrap();
        let mut workspace = Workspace::new(Text::new(), None, None, false).unwrap();
        workspace
            .open(FileLocation {
                path: original.clone(),
                line: 1,
                column: 1,
            })
            .unwrap();
        for _ in 0..2 {
            workspace.clone_file().unwrap();
        }
        std::fs::remove_file(&original).unwrap();
        crate::context_menu::apply_delete(&mut workspace, std::slice::from_ref(&original));
        let (window, documents) = workspace.snapshot().unwrap();
        let session: vel_session::Session = serde_json::from_slice(
            &serde_json::to_vec(&vel_session::Session {
                windows: vec![window],
                documents,
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();
        let mut restored = Workspace::new(Text::new(), None, None, false).unwrap();
        restored
            .restore_window(&session.windows[0], &session.documents)
            .unwrap();
        assert_eq!(restored.tabs.len(), 3);
        assert!(restored.tabs.iter().any(|tab| tab.lazy.is_some()));
        let active = restored.active;
        assert!(restored.save(active, Some(dir.path().to_owned())).is_err());
        assert!(restored.tabs.iter().all(|tab| tab.deleted && tab.dirty()));
        restored.save(active, Some(replacement.clone())).unwrap();
        assert!(!original.exists());
        assert_eq!(std::fs::read_to_string(&replacement).unwrap(), contents);
        assert!(restored.tabs.iter().all(|tab| !tab.deleted && !tab.dirty()));
        assert!(
            restored
                .tabs
                .iter()
                .all(|tab| tab.path.as_ref() == Some(&replacement))
        );
        for index in 0..restored.tabs.len() {
            restored.select(index);
            assert_eq!(restored.editor().text, contents);
            assert!(!restored.tabs[index].deleted);
            assert!(!restored.tabs[index].dirty());
        }
        std::fs::remove_file(&replacement).unwrap();
        crate::context_menu::apply_delete(&mut restored, std::slice::from_ref(&replacement));
        assert!(restored.tabs.iter().all(|tab| tab.deleted && tab.dirty()));
        restored.save(restored.active, None).unwrap();
        assert_eq!(std::fs::read_to_string(&replacement).unwrap(), contents);
        assert!(restored.tabs.iter().all(|tab| !tab.deleted && !tab.dirty()));
        let (_, documents) = restored.snapshot().unwrap();
        assert_eq!(documents.len(), 1);
        assert!(!documents[0].dirty);
    }
    #[test]
    fn pending_center_only_tracks_locations_and_is_consumed_per_active_tab() {
        let mut workspace =
            Workspace::new(Text::from("first\n你好\nthird"), None, None, false).unwrap();
        assert!(!workspace.take_pending_center());
        workspace.editor_mut().set_caret(1, false);
        assert!(!workspace.take_pending_center());
        workspace.tabs[0].goto(&FileLocation {
            path: "unused".into(),
            line: 2,
            column: 2,
        });
        assert_eq!(workspace.editor().caret, "first\n你".len());
        workspace.new_file().unwrap();
        assert!(!workspace.take_pending_center());
        workspace.select(0);
        assert!(workspace.take_pending_center());
        assert!(!workspace.take_pending_center());
        workspace.tabs[0].install(Loaded {
            disk_content: None,
            encoding_chosen: false,
            unchanged: false,
            warning: None,
            text: Text::from("replacement"),
            format: Format::default(),
            source_bytes: 11,
        });
        assert!(!workspace.take_pending_center());
        // The same install path is used when an asynchronous text load finishes.
        workspace.tabs[0].location = Some(FileLocation {
            path: "unused".into(),
            line: 2,
            column: 2,
        });
        workspace.tabs[0].install(Loaded {
            disk_content: None,
            encoding_chosen: false,
            unchanged: false,
            warning: None,
            text: Text::from("first\n你好\nthird"),
            format: Format::default(),
            source_bytes: 18,
        });
        assert_eq!(workspace.editor().caret, "first\n你".len());
        assert!(workspace.take_pending_center());
        assert!(!workspace.take_pending_center());
    }
    #[test]
    fn stamp_polling_waits_500ms_but_explicit_reload_bypasses_the_interval() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("Packages/User");
        std::fs::create_dir_all(&user).unwrap();
        let prefs = user.join("Preferences.sublime-settings");
        std::fs::write(&prefs, r#"{"font_size":15}"#).unwrap();
        let mut workspace =
            Workspace::new(Text::new(), Some(dir.path().into()), None, false).unwrap();
        let now = Instant::now();
        assert!(!workspace.configuration_changed(now));
        std::fs::write(&prefs, r#"{"font_size":20,"tab_size":4}"#).unwrap();
        assert!(!workspace.configuration_changed(now + Duration::from_millis(499)));
        assert!(workspace.configuration_changed(now + Duration::from_millis(500)));
        workspace.request_resource_reload();
        assert!(workspace.configuration_changed(now + Duration::from_millis(501)));
        workspace.last_config_check = Some(Instant::now());
        workspace.poll();
        assert!(workspace.config_rx.is_some() || workspace.resource_revision > 0);
        workspace.wait_resources();
        assert_eq!(workspace.editor().preferences.font_size, 20.);
    }
    #[test]
    fn first_launch_without_user_packages_dir_succeeds_and_creates_it() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("Velocity");
        let mut workspace = Workspace::new(Text::new(), Some(data.clone()), None, false).unwrap();
        assert!(data.join("Packages/User").is_dir());
        assert!(!workspace.configuration_changed(Instant::now()));
    }
    #[test]
    fn atomic_preferences_write_during_resource_load_is_not_lost() {
        use std::io::Write as _;

        for already_exists in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let user = dir.path().join("Packages/User");
            let prefs = user.join("Preferences.sublime-settings");
            if already_exists {
                std::fs::create_dir_all(&user).unwrap();
                std::fs::write(&prefs, r#"{"ui_language":"en"}"#).unwrap();
            }
            let mut reader =
                Workspace::new(Text::new(), Some(dir.path().into()), None, false).unwrap();
            // Like the preferences writer, stage a hidden sibling. Creating it
            // can trigger a directory-stamp reload before the rename.
            let mut staged = tempfile::Builder::new()
                .prefix(".velocity-session-")
                .tempfile_in(&user)
                .unwrap();
            staged.write_all(br#"{"ui_language":"zh-Hans"}"#).unwrap();
            let now = Instant::now();
            reader.request_resource_reload();
            assert!(reader.configuration_changed(now));
            reader.reload_requested = false;
            AFTER_USER_RESOURCE_READ.with(|hook| {
                hook.replace(Some(Box::new(move || {
                    staged.persist(&prefs).unwrap();
                })));
            });
            // Force the rename between reading resources and completing this
            // load, then deliver the result through the real worker mailbox.
            let result = Configuration::load_with_window_overrides(
                Some(dir.path().into()),
                None,
                Some(Default::default()),
                Default::default(),
                ConfigKind::Eager,
                &mut Default::default(),
            );
            let (tx, rx) = std::sync::mpsc::channel();
            tx.send(result).unwrap();
            reader.config_rx = Some(rx);
            reader.poll();
            assert_ne!(reader.ui_language_setting(), "zh-Hans");

            assert!(
                reader.configuration_changed(now + Duration::from_millis(500)),
                "the reader missed an atomic preferences write (already_exists={already_exists})"
            );
            reader.wait_resources();
            assert_eq!(reader.ui_language_setting(), "zh-Hans");
        }
    }
    #[test]
    fn context_menu_resources_layer_and_hot_reload_with_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("Packages/Fixture");
        let user = dir.path().join("Packages/User");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(
            package.join("Context.sublime-menu"),
            r#"[{"id":"fixture","caption":"Package","command":"copy"}]"#,
        )
        .unwrap();
        let menu = user.join("Context.sublime-menu");
        std::fs::write(
            &menu,
            r#"[{"id":"fixture","caption":"User","command":"paste"}]"#,
        )
        .unwrap();
        let mut workspace =
            Workspace::new(Text::new(), Some(dir.path().into()), None, false).unwrap();
        for name in [
            "Context.sublime-menu",
            "Tab Context.sublime-menu",
            "Side Bar.sublime-menu",
        ] {
            assert!(!workspace.context_menus[name].is_empty(), "{name}");
        }
        let item = workspace.context_menus["Context.sublime-menu"]
            .iter()
            .find(|item| item.id == "fixture")
            .unwrap();
        assert_eq!(item.caption, "User");
        assert_eq!(item.command.as_deref(), Some("paste"));
        assert!(!workspace.menus.iter().any(|item| item.id == "fixture"));
        std::fs::write(
            menu,
            r#"[{"id":"fixture","caption":"Reloaded","command":"cut"}]"#,
        )
        .unwrap();
        workspace.wait_resources();
        let item = workspace.context_menus["Context.sublime-menu"]
            .iter()
            .find(|item| item.id == "fixture")
            .unwrap();
        assert_eq!(item.caption, "Reloaded");
        assert_eq!(item.command.as_deref(), Some("cut"));
    }

    #[cfg(unix)]
    #[test]
    fn package_watch_ignores_profile_links_and_still_reloads_linked_user() {
        use std::{fs, os::unix::fs::symlink};
        let data = tempfile::tempdir().unwrap();
        let linked = tempfile::tempdir().unwrap();
        let packages = data.path().join("Packages");
        fs::create_dir(&packages).unwrap();
        symlink("..", packages.join("Cycle")).unwrap();
        symlink(linked.path(), packages.join("User")).unwrap();
        for directory in ["Cache", "Index", "Backup", ".git"] {
            fs::create_dir(data.path().join(directory)).unwrap();
            if directory == ".git" {
                fs::create_dir(linked.path().join(directory)).unwrap();
            } else {
                // Real directories in a package may have these names. Only
                // links into private profile directories should be ignored.
                symlink(data.path().join(directory), linked.path().join(directory)).unwrap();
            }
        }
        let preferences = linked.path().join("Preferences.sublime-settings");
        fs::write(&preferences, r#"{"font_size":17}"#).unwrap();
        let mut workspace =
            Workspace::new(Text::new(), Some(data.path().into()), None, false).unwrap();
        let start = Instant::now();
        assert!(!workspace.configuration_changed(start));
        for directory in ["Cache", "Index", "Backup", ".git"] {
            fs::write(
                data.path().join(directory).join("Never.sublime-settings"),
                "not JSON",
            )
            .unwrap();
        }
        fs::write(
            linked.path().join(".git/Never.sublime-settings"),
            "not JSON",
        )
        .unwrap();
        assert!(!workspace.configuration_changed(start + Duration::from_secs(1)));
        fs::write(&preferences, r#"{"font_size":19, "tab_size":7}"#).unwrap();
        assert!(workspace.configuration_changed(start + Duration::from_secs(2)));
        workspace.request_resource_reload();
        workspace.wait_resources();
        assert_eq!(workspace.editor().preferences.font_size, 19.);
    }
    /// Small paired helper for the stamp traversal changed in round 14. This measures
    /// resource polling alone; the main agent still runs full typing/scroll gates.
    #[test]
    #[ignore = "manual release benchmark; prints paired resource-stamp scan costs"]
    fn benchmark_resource_stamp_polling() {
        let mut workspace = Workspace::new(Text::new(), None, None, false).unwrap();
        let iterations = 10_000;
        let mut baseline = Vec::new();
        let mut throttled = Vec::new();
        for _ in 0..5 {
            let before = Instant::now();
            for _ in 0..iterations {
                std::hint::black_box(
                    workspace
                        .config_stamp
                        .iter()
                        .any(|(path, saved)| stamp(path) != *saved),
                );
            }
            baseline.push(before.elapsed().as_secs_f64() * 1e9 / iterations as f64);
            workspace.last_config_check = None;
            let before = Instant::now();
            for _ in 0..iterations {
                std::hint::black_box(workspace.configuration_changed(Instant::now()));
            }
            throttled.push(before.elapsed().as_secs_f64() * 1e9 / iterations as f64);
        }
        println!(
            "{}",
            serde_json::json!({"kind":"resource_stamp_poll_benchmark","iterations_per_sample":iterations,"resource_stamps":workspace.config_stamp.len(),"baseline_ns_per_poll":baseline,"throttled_ns_per_poll":throttled,"scope":"resource stamps only; no GUI/GPU or full-frame performance claim"})
        );
    }
    #[test]
    fn settings_project_view_and_user_keymap_precedence_reload() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("Packages/User");
        std::fs::create_dir_all(&user).unwrap();
        let prefs = user.join("Preferences.sublime-settings");
        std::fs::write(&prefs, "{ // user\n\"tab_size\": 2, \"font_size\": 16,}").unwrap();
        std::fs::write(user.join("Plain Text.sublime-settings"), "{\"tab_size\":3}").unwrap();
        std::fs::write(
            user.join(PLATFORM_KEYMAP),
            json!([{"keys":[QUICK_ADD_NEXT],"command":"insert","args":{"characters":"override"}}])
                .to_string(),
        )
        .unwrap();
        let project = dir.path().join("test.sublime-project");
        std::fs::write(&project, "{\"settings\":{\"tab_size\":6,\"rulers\":[72]}}").unwrap();
        let mut workspace =
            Workspace::new(Text::new(), Some(dir.path().into()), Some(project), false).unwrap();
        // Project settings apply, but syntax-specific settings override them, like Sublime.
        assert_eq!(workspace.editor().preferences.rulers, vec![72]);
        assert_eq!(workspace.editor().preferences.tab_size, 3);
        assert_eq!(workspace.editor().preferences.font_size, 16.);
        assert!(
            matches!(workspace.editor_mut().resolve_key(&test_keys::parse(QUICK_ADD_NEXT)),
                Resolution::Command(i) if i.command == "insert" && i.args["characters"] == "override")
        );
        workspace
            .editor_mut()
            .settings
            .set(Layer::View, "tab_size", json!(8));
        workspace.editor_mut().sync();
        std::fs::write(&prefs, "{\"font_size\":20,\"tab_size\":4}").unwrap();
        workspace.wait_resources();
        assert_eq!(workspace.editor().preferences.font_size, 20.);
        assert_eq!(workspace.editor().preferences.tab_size, 8);
        assert!(
            matches!(workspace.editor_mut().resolve_key(&test_keys::parse(QUICK_ADD_NEXT)),
                Resolution::Command(i) if i.command == "insert" && i.args["characters"] == "override")
        );
    }
    #[test]
    fn reload_decisions_and_failed_load_do_not_overwrite_edits() {
        let mut workspace = Workspace::new(Text::from("saved"), None, None, false).unwrap();
        workspace
            .editor_mut()
            .run("insert", &json!({"characters":"local "}))
            .unwrap();
        let loaded = || Loaded {
            disk_content: None,
            encoding_chosen: false,
            unchanged: false,
            warning: None,
            text: Text::from("external"),
            format: Format::default(),
            source_bytes: 8,
        };
        workspace.tabs[0].external = Some(loaded());
        workspace.reload_external(0, false);
        assert_eq!(workspace.editor().text.to_string(), "local saved");
        workspace.tabs[0].external = Some(loaded());
        workspace.reload_external(0, true);
        assert_eq!(workspace.editor().text.to_string(), "external");
        assert!(!workspace.tabs[0].dirty());
        workspace.editor_mut().read_only = true;
        workspace
            .editor_mut()
            .run("insert", &json!({"characters":"bad"}))
            .unwrap();
        assert_eq!(workspace.editor().text.to_string(), "external");
        assert!(
            workspace
                .save(0, Some(PathBuf::from("/tmp/velocity-should-not-write")))
                .is_err()
        );
    }
}

#[cfg(test)]
#[path = "package_tests.rs"]
mod package_tests;

#[cfg(test)]
mod round15_tests {
    use super::*;
    use vel_config::{json, settings::Layer};
    fn location(path: PathBuf) -> FileLocation {
        FileLocation {
            path,
            line: 1,
            column: 1,
        }
    }
    fn workspace() -> Workspace {
        Workspace::new(Text::new(), None, None, false).unwrap()
    }
    #[test]
    fn a_change_while_a_reload_reads_is_loaded_instead_of_acknowledged() {
        // A tool that rewrites a file in steps (truncate, write, touch) can change it
        // while a background reload reads. Notifications are not processed while the tab
        // loads, and the finished load used to acknowledge the *current* stamp, so the
        // newer bytes were never loaded (CI showed an empty, stale or mixed decode).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap().join("doc.txt");
        std::fs::write(&path, "first revision\n").unwrap();
        let mut w = workspace();
        w.settings
            .set(Layer::User, "large_file.load_bytes", json!(1));
        w.open(location(path.clone())).unwrap();
        let settle = |w: &mut Workspace, done: &dyn Fn(&Workspace) -> bool| {
            let until = Instant::now() + Duration::from_secs(10);
            while !done(w) {
                w.poll();
                assert!(Instant::now() < until, "load did not settle");
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        settle(&mut w, &|w| w.tabs[0].loading.is_none());
        let base = std::fs::metadata(&path).unwrap().modified().unwrap();
        let rewrite = |text: &str, seconds: u64| {
            use std::io::Write as _;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&path)
                .unwrap();
            file.write_all(text.as_bytes()).unwrap();
            file.set_modified(base + Duration::from_secs(seconds))
                .unwrap();
        };
        rewrite("older revision\n", 2);
        w.apply_file_changes(vec![Ok(vec![path.clone()])]);
        let task = w.tabs[0].loading.as_ref().expect("background reload");
        let (read, total) = (task.read.clone(), task.total);
        // The worker has read every byte of the older revision; only then does the file
        // change again (same length, so the worker's final EOF read is unaffected).
        let until = Instant::now() + Duration::from_secs(10);
        while read.load(Ordering::Relaxed) < total {
            assert!(Instant::now() < until, "reload did not read");
            std::thread::sleep(Duration::from_millis(1));
        }
        rewrite("newer revision\n", 4);
        let newer = stamp(&path);
        settle(&mut w, &|w| {
            w.tabs[0].loading.is_none() && w.tabs[0].disk_stamp == newer
        });
        assert_eq!(w.editor().text.to_string(), "newer revision\n");
        assert_eq!(
            w.tabs[0].disk_saved,
            vel_session::DiskStamp::read(&path),
            "focus-save compares this revision with the disk"
        );
    }
    #[test]
    fn missing_paths_are_named_new_documents_and_save_errors_keep_the_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap().join("new.txt");
        let mut w = workspace();
        w.open(location(path.clone())).unwrap();
        assert_eq!(w.tabs[0].path.as_ref(), Some(&path));
        assert_eq!(w.editor().text.len_bytes(), 0);
        assert!(w.tabs[0].label().contains("[new]"));
        w.editor_mut()
            .run("insert", &json!({"characters":"created"}))
            .unwrap();
        w.save(0, None).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "created");
        assert_eq!(w.recent_files.first(), Some(&path));
        assert!(!w.tabs[0].dirty());
        assert!(!w.tabs[0].label().contains("[new]"));
        let missing_parent = dir.path().join("missing/child.txt");
        w.open(location(missing_parent.clone())).unwrap();
        w.editor_mut()
            .run("insert", &json!({"characters":"keep me"}))
            .unwrap();
        let error = w.save(w.active, None).unwrap_err();
        assert!(error.contains("missing"));
        assert!(!missing_parent.parent().unwrap().exists());
        assert_eq!(w.editor().text.to_string(), "keep me");
        assert!(w.tabs[w.active].dirty());
    }
    #[test]
    fn active_close_chooses_right_then_left_in_its_own_group() {
        let mut w = workspace();
        for text in ["left", "right", "other"] {
            w.new_file().unwrap();
            w.editor_mut()
                .run("insert", &json!({"characters":text}))
                .unwrap();
        }
        w.workbench
            .set_layout(vel_ui::workbench::Layout::grid(2, 1))
            .unwrap();
        w.workbench.move_selected(1).unwrap();
        let other = w.tabs[3].sheet;
        w.select(1);
        let right = w.tabs[2].sheet;
        w.close(1).unwrap();
        assert_eq!(w.tabs[w.active].sheet, right);
        assert_eq!(w.workbench.active, 0);
        w.close(w.active).unwrap();
        assert_ne!(w.tabs[w.active].sheet, other);
        assert_eq!(w.workbench.active, 0);
    }
    #[test]
    fn focusing_an_unused_pane_never_edits_another_groups_document() {
        let mut w = Workspace::new(Text::from("original group"), None, None, false).unwrap();
        w.workbench
            .set_layout(vel_ui::workbench::Layout::grid(2, 1))
            .unwrap();
        w.workbench.active = 1;
        w.focus_current_group().unwrap();
        assert_eq!(w.editor().text.len_bytes(), 0);
        w.editor_mut()
            .run("insert", &json!({"characters":"new group"}))
            .unwrap();
        assert_eq!(w.tabs[0].editor.text.to_string(), "original group");
        assert_eq!(w.workbench.group_of(w.tabs[w.active].sheet), Some(1));
    }
    #[test]
    fn final_close_leaves_no_sheet_and_typing_or_new_file_materializes_on_demand() {
        let mut w = workspace();
        w.close(0).unwrap();
        assert!(w.is_empty());
        assert!(w.workbench.groups[0].sheets.is_empty());
        assert_eq!(w.window_title(), "untitled");
        let (window, docs) = w.snapshot().unwrap();
        assert!(window.sheets.is_empty());
        assert!(docs.is_empty());
        let mut restored = workspace();
        restored.restore_window(&window, &docs).unwrap();
        assert!(restored.is_empty());
        restored
            .editor_mut()
            .run("insert", &json!({"characters":"first edit"}))
            .unwrap();
        restored.sync_shared();
        assert!(!restored.is_empty());
        assert_eq!(restored.workbench.groups[0].sheets.len(), 1);
        assert_eq!(restored.editor().text.to_string(), "first edit");
        w.new_file().unwrap();
        assert!(!w.is_empty());
        assert_eq!(w.tabs.len(), 1);
        assert_eq!(w.workbench.groups[0].sheets.len(), 1);
    }
    #[test]
    fn save_all_skips_clean_tabs_and_continues_after_cancel_and_failure() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = workspace();
        // The initial clean untitled tab must not summon a save dialog.
        for text in ["cancel this", "failure", "save this", "read only"] {
            w.new_file().unwrap();
            w.editor_mut()
                .run("insert", &json!({"characters":text}))
                .unwrap();
        }
        w.editor_mut().read_only = true;
        let mut prompted = Vec::new();
        let errors = w.save_all(|name| {
            prompted.push(name.to_owned());
            match name {
                "cancel this" => None,
                "failure" => Some(dir.path().join("absent/child")),
                other => Some(dir.path().join(other)),
            }
        });
        assert!(!prompted.iter().any(|s| s == "Untitled"));
        assert_eq!(errors.len(), 2);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("save this")).unwrap(),
            "save this"
        );
        assert!(w.tabs[1].dirty());
        assert!(w.tabs[2].dirty());
        assert!(!w.tabs[3].dirty());
        assert_eq!(
            w.recent_files.first(),
            Some(&dir.path().canonicalize().unwrap().join("save this"))
        );
        assert!(w.tabs[4].dirty());
    }
    #[test]
    fn explicit_reopen_decodes_selected_codec_and_failure_preserves_dirty_contents() {
        // Round 20 deliberately falls back when a codec rejects readable bytes;
        // I/O failure is the unchanged-document case from lane B's original test.
        explicit_reopen_decodes_selected_codec_and_io_failure_preserves_dirty_contents();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("short.txt");
        std::fs::write(&path, b"a").unwrap();
        let mut w = workspace();
        w.open(location(path)).unwrap();
        w.reopen_encoding("UTF-16 LE with BOM").unwrap();
        assert_eq!(w.editor().text, "a");
        assert_eq!(w.tabs[0].format.encoding, "UTF-8");
    }
    #[test]
    fn explicit_reopen_decodes_selected_codec_and_io_failure_preserves_dirty_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("encoding.txt");
        std::fs::write(&path, "é").unwrap();
        let mut w = workspace();
        w.open(location(path.clone())).unwrap();
        w.reopen_encoding("Western (Windows 1252)").unwrap();
        assert_eq!(w.editor().text.to_string(), "Ã©");
        w.editor_mut()
            .run("insert", &json!({"characters":"keep unsaved "}))
            .unwrap();
        let old = w.editor().text.clone();
        std::fs::remove_file(&path).unwrap();
        assert!(w.reopen_encoding("UTF-16 LE with BOM").is_err());
        assert_eq!(w.editor().text, old);
        assert!(w.tabs[0].dirty());
    }
    #[test]
    fn snippets_without_a_description_use_their_file_name() {
        assert_eq!(
            snippet_title("Packages/Rust/Snippets/bench.sublime-snippet"),
            "bench"
        );
        assert_eq!(snippet_title("loose.sublime-snippet"), "loose");
        assert_eq!(snippet_title("Packages/User/odd"), "odd");
    }
    #[test]
    fn disabled_hot_exit_routes_cli_projects_to_a_separate_window() {
        let mut w = workspace();
        w.settings.set(Layer::User, "hot_exit", json!(false));
        w.editor_mut()
            .run("insert", &json!({"characters":"preserve"}))
            .unwrap();
        let options = crate::cli::Options {
            project: Some("project.sublime-project".into()),
            ..Default::default()
        };
        use crate::open_requests::{OpenWindow, Route, RouteRequest, WindowRoute, route};
        let mut request = crate::instance::Request::new(&options, None).unwrap();
        let request = RouteRequest::prepare(&mut request, OpenWindow::Never);
        let mut window = WindowRoute::default();
        window.update(&w, true);
        assert_eq!(route(&request, &[(1, window)].into(), Some(1)), Route::New);
    }
    #[test]
    fn close_deleted_files_closes_clean_watched_tabs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deleted.txt");
        std::fs::write(&path, "clean").unwrap();
        let mut w = Workspace::new(Text::new(), None, None, true).unwrap();
        w.settings
            .set(Layer::User, "close_deleted_files", json!(true));
        w.open(location(path.clone())).unwrap();
        std::fs::remove_file(path).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !w.is_empty() {
            w.poll();
            assert!(Instant::now() < deadline);
            std::thread::park_timeout(Duration::from_millis(2));
        }
        assert!(w.workbench.groups[0].sheets.is_empty());
    }
    #[test]
    fn untitled_names_are_unicode_bounded_and_close_names_have_no_dirty_marker() {
        let mut w = workspace();
        let first = "你好".repeat(40);
        w.editor_mut()
            .run(
                "insert",
                &json!({"characters":format!("  {first}\nsecond")}),
            )
            .unwrap();
        assert_eq!(w.tabs[0].name().chars().count(), 50);
        assert!(!w.tabs[0].name().contains('•'));
        assert!(w.tabs[0].label().ends_with(" •"));
        w.editor_mut()
            .settings
            .set(Layer::View, "set_unsaved_view_name", json!(false));
        assert_eq!(w.tabs[0].name(), "Untitled");
    }
    #[test]
    fn reopen_stack_is_window_local_and_restores_selection_and_scroll() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("closed.txt");
        std::fs::write(&path, "abcdef\n").unwrap();
        let mut w = workspace();
        w.open(location(path.clone())).unwrap();
        w.editor_mut()
            .set_selection(Selection::from_regions([Region::new(4, 1)]));
        w.tabs[0].scroll = (15., 96.);
        w.close(0).unwrap();
        assert_eq!(w.closed_files.len(), 1);
        assert!(workspace().closed_files.is_empty());
        w.reopen_last_file().unwrap();
        assert!(w.closed_files.is_empty());
        assert_eq!(w.tabs[0].path.as_ref(), Some(&path.canonicalize().unwrap()));
        assert_eq!(w.editor().regions(), vec![Region::new(4, 1)]);
        assert_eq!(w.tabs[0].scroll, (15., 96.));
        assert!(!w.take_pending_center());
    }
    #[test]
    fn project_save_as_preserves_relative_paths_unknown_folder_keys_and_absent_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let old_dir = dir.path().join("old");
        let new_dir = dir.path().join("new");
        std::fs::create_dir_all(old_dir.join("src")).unwrap();
        std::fs::create_dir(&new_dir).unwrap();
        let old = old_dir.join("app.sublime-project");
        std::fs::write(&old, r#"{"folders":[{"path":"src","file_include_patterns":["*.rs"],"folder_include_patterns":["code*"],"custom_folder":{"keep":7}}],"custom_project":42}"#).unwrap();
        let mut w = Workspace::new(Text::new(), None, Some(old.clone()), false).unwrap();
        assert!(w.folders[0].follow_links);
        let new = new_dir.join("renamed.sublime-project");
        w.save_project_as(new.clone()).unwrap();
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&new).unwrap()).unwrap();
        assert_eq!(
            Path::new(saved["folders"][0]["path"].as_str().unwrap()),
            Path::new("../old/src"),
        );
        assert_eq!(saved["folders"][0]["custom_folder"]["keep"], 7);
        assert_eq!(saved["custom_project"], 42);
        for key in [
            "follow_symlinks",
            "file_exclude_patterns",
            "folder_exclude_patterns",
            "binary_file_patterns",
        ] {
            assert!(saved["folders"][0].get(key).is_none(), "{key}");
        }
        assert!(saved.get("settings").is_none());
        assert!(saved.get("build_systems").is_none());
        assert!(w.folders[0].rules.allows(
            &w.folders[0].path,
            &w.folders[0].path.join("main.rs"),
            false,
            true
        ));
        assert!(!w.folders[0].rules.allows(
            &w.folders[0].path,
            &w.folders[0].path.join("main.txt"),
            false,
            true
        ));
        assert!(
            std::fs::read_to_string(old)
                .unwrap()
                .contains("\"path\":\"src\"")
        );
    }
    #[test]
    fn documented_exclusions_hide_build_objects_but_allow_target() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("app.sublime-project");
        std::fs::write(&project, r#"{"folders":[{"path":"."}]}"#).unwrap();
        let w = Workspace::new(Text::new(), None, Some(project), false).unwrap();
        let folder = &w.folders[0];
        for name in ["target", "node_modules", "src"] {
            assert!(
                folder
                    .rules
                    .allows(&folder.path, &folder.path.join(name), true, false),
                "{name}"
            );
        }
        for name in [".svn", ".hg", ".git", "CVS"] {
            assert!(
                !folder
                    .rules
                    .allows(&folder.path, &folder.path.join(name), true, false),
                "{name}"
            );
        }
        for name in [
            ".DS_Store",
            "module.o",
            "module.obj",
            "app.sublime-workspace",
            "module.pyc",
        ] {
            assert!(
                !folder
                    .rules
                    .allows(&folder.path, &folder.path.join(name), false, true),
                "{name}"
            );
        }
    }
    #[test]
    fn indentation_choices_survive_resource_and_external_reload_and_project_watching() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("profile");
        let file = dir.path().join("indent.rs");
        let project = dir.path().join("project.sublime-project");
        std::fs::write(&file, "fn main() {\n  first();\n  second();\n}\n").unwrap();
        std::fs::write(&project, r#"{"folders":[{"path":"."}]}"#).unwrap();
        let mut w = Workspace::new(Text::new(), Some(data), None, true).unwrap();
        w.open_project(project).unwrap();
        assert!(w.watcher.is_some());
        w.open(location(file.clone())).unwrap();
        assert_eq!(w.editor().settings.usize("tab_size", 0), 2);
        w.editor_mut()
            .settings
            .set(Layer::View, "tab_size", json!(8));
        w.editor_mut()
            .settings
            .set(Layer::View, "translate_tabs_to_spaces", json!(false));
        w.request_resource_reload();
        w.wait_resources();
        assert_eq!(w.editor().settings.usize("tab_size", 0), 8);
        std::fs::write(&file, "fn main() {\n    changed();\n    again();\n}\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !w.editor().text.to_string().contains("changed") {
            w.poll();
            assert!(Instant::now() < deadline, "{:?}", w.messages);
            std::thread::park_timeout(Duration::from_millis(2));
        }
        assert_eq!(w.editor().settings.usize("tab_size", 0), 8);
        assert!(!w.editor().settings.bool("translate_tabs_to_spaces", true));
        std::fs::remove_file(&file).unwrap();
        while !w.tabs[w.active].deleted {
            w.poll();
            assert!(Instant::now() < deadline);
            std::thread::park_timeout(Duration::from_millis(2));
        }
        assert!(w.tabs[w.active].dirty());
        assert!(w.tabs[w.active].label().contains("[deleted]"));
        w.save(w.active, None).unwrap();
        assert!(file.exists());
        assert!(!w.tabs[w.active].deleted);
    }
    #[test]
    fn missing_files_do_not_abort_restore_and_close_deleted_files_keeps_dirty_text() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.txt");
        let healthy = dir.path().join("healthy.txt");
        std::fs::write(&missing, "saved").unwrap();
        std::fs::write(&healthy, "healthy").unwrap();
        let mut w = workspace();
        w.open(location(missing.clone())).unwrap();
        w.open(location(healthy)).unwrap();
        let (window, documents) = w.snapshot().unwrap();
        std::fs::remove_file(missing).unwrap();
        let mut restored = workspace();
        restored.restore_window(&window, &documents).unwrap();
        assert_eq!(restored.tabs.len(), 2);
        restored.select(0);
        assert!(restored.tabs[0].deleted);
        assert!(!restored.tabs[0].dirty());
        assert!(restored.tabs[0].load_failed);
        assert!(restored.tabs[0].editor.read_only);
        restored.select(1);
        assert_eq!(restored.editor().text.to_string(), "healthy");
        let mut close_deleted = workspace();
        close_deleted
            .settings
            .set(Layer::User, "close_deleted_files", json!(true));
        close_deleted.restore_window(&window, &documents).unwrap();
        assert_eq!(close_deleted.tabs.len(), 1);
        let mut dirty_docs = documents;
        dirty_docs[0].dirty = true;
        let mut buffer = vel_buffer::Buffer::new("retained dirty text");
        buffer.new_view();
        dirty_docs[0].history = Some(vel_session::History::capture(&buffer));
        close_deleted.restore_window(&window, &dirty_docs).unwrap();
        assert_eq!(close_deleted.tabs.len(), 2);
        close_deleted.select(0);
        assert_eq!(
            close_deleted.editor().text.to_string(),
            "retained dirty text"
        );
    }
    #[cfg(unix)]
    #[test]
    fn failing_visible_and_lazy_restore_is_reported_once_and_other_tabs_load() {
        // Both lanes' assertions now use the shared non-modal recovery channel.
        failing_visible_and_lazy_restore_is_status_only_and_other_tabs_load();
    }
    #[cfg(unix)]
    #[test]
    fn failing_visible_and_lazy_restore_is_status_only_and_other_tabs_load() {
        use std::os::unix::net::UnixListener;
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("failed.txt");
        let good = dir.path().join("good.txt");
        std::fs::write(&bad, "bad").unwrap();
        std::fs::write(&good, "good").unwrap();
        let mut w = workspace();
        w.open(location(bad.clone())).unwrap();
        w.open(location(good)).unwrap();
        let (mut window, documents) = w.snapshot().unwrap();
        // Bypass the saved history to exercise the plain file loader's lazy failure.
        let mut documents = documents;
        documents[0].history = None;
        std::fs::remove_file(&bad).unwrap();
        let _socket = UnixListener::bind(bad).unwrap();
        let mut restored = workspace();
        restored.restore_window(&window, &documents).unwrap();
        assert_eq!(restored.editor().text.to_string(), "good");
        assert!(restored.prepare_sheet(0).is_ok());
        assert!(restored.tabs[0].load_failed);
        assert!(restored.tabs[0].editor.read_only);
        assert!(restored.messages.is_empty());
        assert!(restored.tabs[0].lazy.is_none());
        assert!(restored.tabs[0].editor.read_only);
        assert!(restored.messages.is_empty());
        assert!(restored.prepare_sheet(0).is_ok());
        let sheets = window.workbench.groups[0].sheets.clone();
        window.workbench.groups[0].selected.extend(sheets);
        let mut visible = workspace();
        visible.restore_window(&window, &documents).unwrap();
        visible.poll();
        assert!(visible.messages.is_empty());
        assert_eq!(
            visible
                .resource_reports
                .iter()
                .filter(|m| m.contains("failed.txt"))
                .count(),
            1
        );
        visible.select(0);
        visible.editor_mut();
        visible.poll();
        assert!(visible.messages.is_empty());
        assert_eq!(
            visible
                .resource_reports
                .iter()
                .filter(|m| m.contains("failed.txt"))
                .count(),
            1
        );
    }
    #[test]
    fn legacy_hot_exit_false_and_recent_encoding_commands_are_wired() {
        let mut w = workspace();
        w.settings.set(Layer::User, "hot_exit", json!(false));
        assert_eq!(w.hot_exit(), vel_session::HotExit::Disabled);
        let key = vel_config::keymap::Key::parse("super+shift+t").unwrap();
        assert!(
            matches!(w.editor_mut().resolve_key(&key), vel_config::keymap::Resolution::Command(ref command) if command.command == "reopen_last_file")
        );
        w.recent_files.push("/tmp/a.txt".into());
        w.recent_folders.push("/tmp/folder".into());
        w.recent_projects
            .push("/tmp/project.sublime-project".into());
        assert!(w.refresh_file_menus());
        assert!(!w.refresh_file_menus());
        let file = w.menus.iter().find(|m| m.id == "file").unwrap();
        let recent = file
            .children
            .iter()
            .find(|m| m.id == "open_recent")
            .unwrap();
        assert_eq!(
            recent
                .children
                .iter()
                .map(|row| row.caption.as_str())
                .collect::<Vec<_>>(),
            ["File", "Folder", "Project", "-", "Clear Items"]
        );
        for command in ["reopen_encoding", "save_encoding"] {
            let submenu = file
                .children
                .iter()
                .find(|m| {
                    m.children
                        .first()
                        .is_some_and(|c| c.command.as_deref() == Some(command))
                })
                .unwrap();
            assert_eq!(submenu.children.len(), vel_fileio::ENCODINGS.len());
        }
        w.recent_files.push("/tmp/b.txt".into());
        assert!(w.refresh_file_menus());
        w.recent_files.pop();
        assert!(w.refresh_file_menus(), "a shorter list is not its prefix");
        let file = w.menus.iter().find(|menu| menu.id == "file").unwrap();
        let recent = file
            .children
            .iter()
            .find(|item| item.id == "open_recent")
            .unwrap();
        assert_eq!(recent.children[0].children.len(), 1);
        assert!(!w.refresh_file_menus());
    }
}

#[cfg(test)]
mod review15_tests {
    use super::*;
    use vel_config::json;
    fn workspace(watch: bool) -> Workspace {
        Workspace::new(Text::new(), None, None, watch).unwrap()
    }
    fn location(path: &Path) -> FileLocation {
        FileLocation {
            path: path.into(),
            line: 1,
            column: 1,
        }
    }
    fn until(w: &mut Workspace, condition: impl Fn(&Workspace) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !condition(w) {
            w.poll();
            assert!(Instant::now() < deadline, "{:?}", w.messages);
            std::thread::park_timeout(Duration::from_millis(2));
        }
    }
    #[test]
    fn empty_window_previews_never_leak_sheets_or_break_all_window_hot_exit() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "first").unwrap();
        std::fs::write(&b, "second").unwrap();
        let mut w = workspace(false);
        w.close(0).unwrap();
        let hidden = w.tabs[0].sheet;
        w.preview(location(&a)).unwrap();
        assert_eq!(w.tabs.len(), 1);
        assert_ne!(w.tabs[0].sheet, hidden);
        w.preview(location(&b)).unwrap(); // Replacing the only transient creates another empty adapter.
        assert_eq!(w.tabs.len(), 1);
        assert_eq!(w.workbench.groups[0].sheets, vec![w.tabs[0].sheet]);
        let mut other = workspace(false);
        other
            .editor_mut()
            .run("insert", &json!({"characters":"unsaved in another window"}))
            .unwrap();
        let mut sessions = crate::session::Sessions::new(Some(dir.path()));
        for window in [&mut w, &mut other] {
            let id = sessions.register();
            let (snapshot, documents) = window.snapshot().unwrap();
            sessions
                .publish(
                    id,
                    snapshot,
                    documents,
                    Default::default(),
                    crate::session::Policy {
                        hot: vel_session::HotExit::Always,
                        quitting: true,
                        closing: false,
                        project_hot_exit: true,
                        remember_layout: true,
                        remember: true,
                    },
                )
                .unwrap();
        }
        sessions.flush().unwrap();
        let restored = vel_session::load(&dir.path().join("Sessions/Session.sublime-session"));
        assert!(restored.messages.is_empty(), "{:?}", restored.messages);
        restored.session.validate().unwrap();
        assert_eq!(restored.session.windows.len(), 2);
        assert!(
            restored
                .session
                .documents
                .iter()
                .any(|d| d.history.as_ref().is_some_and(|h| h
                    .current_text()
                    .is_ok_and(|t| t == "unsaved in another window")))
        );
        w.close(w.active).unwrap(); // Escape's close, then a stale select of the old origin.
        w.select(0);
        assert!(w.is_empty());
        assert!(w.snapshot().unwrap().0.sheets.is_empty());
        w.editor_mut()
            .run("insert", &json!({"characters":"visible input"}))
            .unwrap();
        w.materialize_input();
        assert!(!w.is_empty());
        assert_eq!(w.workbench.groups[0].sheets, vec![w.tabs[0].sheet]);
        assert_eq!(w.tabs.len(), 1);
        assert!(w.closed_files.is_empty());
    }
    #[test]
    fn cancelled_goto_in_an_empty_window_leaves_no_ghost_untitled() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("preview.txt");
        std::fs::write(&file, "preview").unwrap();
        let mut w = workspace(false);
        w.close(0).unwrap();
        let mut panels = crate::panels::Panels::new(&w, false);
        panels
            .action(&mut w, "show_overlay", &json!({"overlay":"goto"}), 0..0)
            .unwrap();
        let original = w.tabs[0].sheet;
        w.preview_goto(location(&file), 0).unwrap();
        assert!(w.is_empty());
        assert!(w.workbench.groups[0].sheets.is_empty());
        panels
            .key(
                &mut w,
                &vel_config::keymap::Key::parse("escape").unwrap(),
                0..7,
            )
            .unwrap();
        assert!(w.is_empty());
        assert_eq!(w.tabs[0].sheet, original);
        assert!(w.workbench.groups.iter().all(|g| g.sheets.is_empty()));
        assert!(w.snapshot().unwrap().1.is_empty());
        assert!(w.closed_files.is_empty());
    }
    #[test]
    fn r41_retarget_reuses_sheet_and_only_committed_files_are_watched() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let mut w = workspace(false);
        w.watcher = Some(Watcher::new().unwrap());
        w.settings
            .set(Layer::User, "large_file.load_bytes", serde_json::json!(16));
        let origin = w.workbench.groups[0].sheets.clone();
        let mut hidden = None;
        for index in 0..12 {
            let file = root.join(format!("preview-{index}.txt"));
            std::fs::write(&file, "background read candidate\n").unwrap();
            w.preview_goto(location(&file), 0).unwrap();
            let tab = &w.tabs[w.active];
            assert_eq!(*hidden.get_or_insert(tab.sheet), tab.sheet);
            assert!(
                tab.loading.is_some(),
                "large-file policy still queues a load"
            );
            assert_eq!(w.tabs.len(), 2);
            assert_eq!(w.workbench.groups[0].sheets, origin);
            std::fs::write(&file, "updated candidate (different size)\n").unwrap();
        }
        w.discard_goto_preview();
        assert!(w.watcher.as_ref().unwrap().changed().is_empty());
        assert!(w.recent_files.is_empty());
        let committed = root.join("commit.txt");
        std::fs::write(&committed, "accepted").unwrap();
        w.preview_goto(location(&committed), 0).unwrap();
        w.commit_goto_preview();
        std::fs::write(&committed, "different size after acceptance").unwrap();
        assert!(
            w.watcher
                .as_ref()
                .unwrap()
                .changed()
                .into_iter()
                .flatten()
                .flatten()
                .any(|path| path == committed)
        );
        assert_eq!(w.recent_files, vec![committed]);
    }
    #[test]
    fn recreated_files_clear_deletion_and_reload_only_without_real_edits() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("checkout.txt");
        std::fs::write(&file, "original").unwrap();
        let mut w = workspace(true);
        w.open(location(&file)).unwrap();
        w.clone_file().unwrap();
        for replacement in ["original", "changed checkout"] {
            std::fs::remove_file(&file).unwrap();
            until(&mut w, |w| w.tabs.iter().all(|t| t.deleted));
            std::fs::write(&file, replacement).unwrap();
            until(&mut w, |w| {
                w.tabs.iter().all(|t| !t.deleted) && w.editor().text == replacement
            });
            assert!(w.tabs.iter().all(|t| !t.dirty() && t.external.is_none()));
        }
        std::fs::remove_file(&file).unwrap();
        until(&mut w, |w| w.tabs.iter().all(|t| t.deleted));
        w.editor_mut()
            .run("insert", &json!({"characters":"local edit "}))
            .unwrap();
        let local = w.editor().text.clone();
        std::fs::write(&file, "new disk text").unwrap();
        until(&mut w, |w| w.tabs[w.active].external.is_some());
        assert!(!w.tabs[w.active].deleted);
        assert!(w.tabs[w.active].dirty());
        assert_eq!(w.editor().text, local);
    }
    #[test]
    fn reopen_skips_preview_closes_and_missing_entries_but_keeps_real_view_state() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("real.txt");
        let b = dir.path().join("missing.txt");
        let c = dir.path().join("preview.txt");
        for path in [&a, &b, &c] {
            std::fs::write(path, "abcdef").unwrap();
        }
        let mut w = workspace(false);
        w.open(location(&a)).unwrap();
        w.editor_mut()
            .set_selection(Selection::from_regions([Region::new(5, 2)]));
        w.tabs[0].scroll = (2., 50.);
        w.close(0).unwrap();
        w.open(location(&b)).unwrap();
        w.close(0).unwrap();
        std::fs::remove_file(&b).unwrap();
        w.preview(location(&c)).unwrap();
        w.close(0).unwrap();
        assert_eq!(w.closed_files.len(), 2);
        w.reopen_last_file().unwrap();
        assert_eq!(
            w.tabs[w.active].path.as_deref(),
            Some(a.canonicalize().unwrap().as_path())
        );
        assert_eq!(w.editor().regions(), vec![Region::new(5, 2)]);
        assert_eq!(w.tabs[w.active].scroll, (2., 50.));
        assert!(w.closed_files.is_empty());
        w.close(w.active).unwrap();
        std::fs::remove_file(a).unwrap();
        w.reopen_last_file().unwrap();
        assert!(w.is_empty());
        assert!(w.closed_files.is_empty());
    }
    #[test]
    fn hidden_input_cannot_be_saved_as_an_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unintended.txt");
        let mut w = workspace(false);
        w.close(0).unwrap();
        assert!(w.save(0, Some(path.clone())).is_err());
        assert!(!path.exists());
        w.editor_mut()
            .run("insert", &json!({"characters":"intended"}))
            .unwrap();
        w.save(0, Some(path.clone())).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "intended");
    }
}

#[cfg(test)]
mod review15_indentation_tests {
    use super::*;
    use vel_config::json;
    #[test]
    fn untitled_and_cloned_manual_indentation_is_not_redetected_without_a_load() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = Workspace::new(Text::new(), None, None, false).unwrap();
        for empty in [false, true] {
            if empty {
                while !w.is_empty() {
                    w.close(w.active).unwrap();
                }
            } else {
                w.new_file().unwrap();
            }
            w.editor_mut()
                .settings
                .set(Layer::View, "tab_size", json!(8));
            w.editor_mut()
                .run(
                    "insert",
                    &json!({"characters":"fn main() {\n  one();\n  two();\n}\n"}),
                )
                .unwrap();
            w.materialize_input();
            w.clone_file().unwrap();
            w.set_appearance(true).unwrap();
            w.poll();
            w.save(
                w.active,
                Some(dir.path().join(format!("manual-{empty}.rs"))),
            )
            .unwrap();
            assert_eq!(w.editor().settings.usize("tab_size", 0), 8);
            let path = w.tabs[w.active].path.clone().unwrap();
            std::fs::write(&path, "fn main() {\n    changed();\n    again();\n}\n").unwrap();
            w.tabs[w.active].external = Some(fileio::load(&path, "windows-1252", |_| {}).unwrap());
            w.reload_external(w.active, true);
            w.poll();
            assert_eq!(w.editor().settings.usize("tab_size", 0), 8);
        }
    }
}

#[cfg(test)]
mod resource_log_tests {
    use super::*;
    #[test]
    fn resource_log_is_timestamped_and_rotates_past_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("Velocity.log");
        let mut reports = Vec::new();
        write_resource_log(Some(dir.path()), &["first warning".into()], &mut reports);
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.starts_with('[') && text.contains("1 resource warning(s)\nfirst warning\n"));
        std::fs::write(&log, vec![b'x'; RESOURCE_LOG_LIMIT as usize + 1]).unwrap();
        write_resource_log(Some(dir.path()), &["second warning".into()], &mut reports);
        assert_eq!(
            std::fs::metadata(dir.path().join("Velocity.log.1"))
                .unwrap()
                .len(),
            RESOURCE_LOG_LIMIT + 1
        );
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("second warning") && !text.contains('x'));
        assert!(reports.is_empty());
    }
}

/// Records a measurement only when telemetry is (or may yet be) enabled. Normal editing
/// has none: every file open used to build and keep a JSON record with its path, and a
/// window that never drew a frame never drained them. Until the App decides, records
/// are capped so they cannot grow without bound.
fn record_telemetry(
    telemetry: &mut Vec<Value>,
    enabled: Option<bool>,
    record: impl FnOnce() -> Value,
) {
    const CAP: usize = 1024;
    if enabled != Some(false) && telemetry.len() < CAP {
        telemetry.push(record());
    }
}
