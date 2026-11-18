//! Shared document properties and checked view attachment at the tab boundary.
use crate::workspace::{Tab, Workspace};
use vel_buffer::{Buffer, ViewId};
use vel_session::ViewState;
use vel_text::Text;

pub(crate) fn attached(buffer: &Buffer, view: ViewId) -> bool {
    buffer.views().any(|id| id == view)
}
impl Tab {
    /// Document state follows the buffer; selections, folds, settings and scroll are per view.
    pub(crate) fn inherit_document(&mut self, source: &Self) {
        self.session_document = source.session_document;
        self.document = source.document;
        self.restored_document = source.restored_document;
        self.deleted = source.deleted;
        self.restored_format_dirty = source.restored_format_dirty;
        self.load_failed = source.load_failed;
        self.shared_dirty = source.shared_dirty || source.editor.buffer.is_dirty();
        self.path = source.path.clone();
        self.format = source.format.clone();
        self.saved_format = source.saved_format.clone();
        self.tracker = source.tracker.clone();
        self.disk_saved = source.disk_saved.clone();
        self.disk_content = source.disk_content;
        self.saved_in_effective_encoding = source.saved_in_effective_encoding;
        self.legacy_stamp_matches = source.legacy_stamp_matches;
        self.legacy_saved_text = source.legacy_saved_text.clone();
        self.legacy_text_fingerprint = source.legacy_text_fingerprint.clone();
        self.encoding_chosen = source.encoding_chosen;
        self.preferred_encoding = source.preferred_encoding.clone();
        self.restore_history = source.restore_history.clone();
        self.overwrite_allowed = source.overwrite_allowed;
        self.disk_stamp = source.disk_stamp;
        self.needs_detection = source.needs_detection;
        self.indentation_initialized = source.indentation_initialized;
        self.editor.language = source.editor.language.clone();
        self.editor.layout_edits = source.editor.layout_edits.clone();
        self.editor.annotations = source.editor.annotations.clone();
        self.inherit_document_mode(source);
    }
    pub(crate) fn inherit_document_mode(&mut self, source: &Self) {
        self.binary_document = source.binary_document;
        self.editor.binary = source.editor.binary.clone();
        self.editor.read_only = source.editor.read_only
            || (source.loading.is_some() && !source.external_load)
            || source.binary_loading.is_some()
            || source.editor.binary.is_some();
        self.editor.features = source.editor.features;
        self.editor.document_metrics = source.editor.document_metrics;
        self.editor.source_bytes = source.editor.source_bytes;
    }
}
impl Workspace {
    pub(crate) fn save_conflict(
        &self,
        index: usize,
        chosen: Option<&std::path::Path>,
    ) -> Result<bool, String> {
        let tab = &self.tabs[index];
        let path = chosen
            .or(tab.path.as_deref())
            .ok_or("save requires a path")?;
        let current = vel_fileio::content_identity(path)?;
        let inplace = chosen.is_none()
            || tab.path.as_deref().is_some_and(|p| {
                p == path
                    || crate::workspace::absolute(p).ok() == crate::workspace::absolute(path).ok()
            });
        if !inplace || current.is_none() || current == tab.overwrite_allowed {
            return Ok(false);
        }
        if tab.disk_content.is_some() {
            return Ok(current != tab.disk_content);
        }
        // Legacy sessions have no raw digest. Their saved text/format can prove
        // equality, but a matching stamp alone must not authorize an overwrite.
        let mut options = tab.decode_options();
        options.legacy_stamp_matches = false;
        Ok(!vel_fileio::revision_unchanged(path, &options)?)
    }
    pub(crate) fn acknowledge_overwrite(
        &mut self,
        index: usize,
        chosen: Option<&std::path::Path>,
    ) -> Result<(), String> {
        let path = chosen
            .or(self.tabs[index].path.as_deref())
            .ok_or("save requires a path")?;
        self.tabs[index].overwrite_allowed = vel_fileio::content_identity(path)?;
        Ok(())
    }
    pub(crate) fn retry_failed_document(&mut self, index: usize) -> Result<(), String> {
        let identity = self.tabs[index].session_document;
        let document = vel_session::Document {
            id: identity,
            path: self.tabs[index].path.clone(),
            disk: self.tabs[index].disk_saved.clone(),
            disk_content: self.tabs[index].disk_content,
            saved_in_effective_encoding: self.tabs[index].saved_in_effective_encoding,
            effective_encoding: Some(self.tabs[index].saved_format.encoding.clone()),
            encoding_chosen: self.tabs[index].encoding_chosen,
            preferred_encoding: self.tabs[index].preferred_encoding.clone(),
            encoding_display_name: self.tabs[index].format.display_name.clone(),
            encoding: self.tabs[index].format.encoding.clone(),
            bom: self.tabs[index].format.bom,
            line_ending: self.tabs[index].format.line_ending.name().into(),
            dirty: false,
            history: self.tabs[index]
                .restore_history
                .as_ref()
                .map(|(history, _)| history.clone()),
        };
        for sibling in 0..self.tabs.len() {
            if self.tabs[sibling].session_document != identity {
                continue;
            }
            let state = self.saved_sheet_state(sibling);
            let tab = &mut self.tabs[sibling];
            tab.lazy = Some((document.clone(), state));
            tab.load_failed = false;
            tab.deleted = false;
            tab.binary_document = false;
            tab.editor.binary = None;
            tab.editor.read_only = true;
        }
        self.prepare_sheet(index)
    }
    pub(crate) fn require_editor_view(&self, index: usize) -> Result<(), String> {
        let tab = self.tabs.get(index).ok_or("unknown sheet")?;
        if !attached(&tab.editor.buffer, tab.editor.id) {
            return Err("document view is not attached".into());
        }
        Ok(())
    }
    pub(crate) fn copy_document_properties(&mut self, owner: usize, target: usize) {
        if owner == target {
            return;
        }
        if owner < target {
            let (a, b) = self.tabs.split_at_mut(target);
            b[0].inherit_document(&a[owner]);
        } else {
            let (a, b) = self.tabs.split_at_mut(owner);
            a[target].inherit_document(&b[0]);
        }
    }
    pub(crate) fn saved_sheet_state(&self, index: usize) -> ViewState {
        let tab = &self.tabs[index];
        let mut state = tab
            .pending_restore
            .as_ref()
            .or_else(|| tab.lazy.as_ref().map(|(_, view)| view))
            .cloned()
            .unwrap_or_else(|| {
                if attached(&tab.editor.buffer, tab.editor.id) {
                    ViewState::capture(&tab.editor.buffer, tab.editor.id, tab.scroll)
                } else {
                    ViewState::default()
                }
            });
        if tab.load_failed
            && let Some((history, _)) = self
                .tabs
                .iter()
                .filter(|owner| owner.session_document == tab.session_document)
                .find_map(|owner| owner.restore_history.as_ref())
            && state
                .history_view
                .is_some_and(|id| !history.view_ids().contains(&id))
        {
            // Runtime placeholder/clone IDs do not belong to the retained native
            // history. Keep the selection/folds, and attach a fresh view on retry.
            state.history_view = None;
        }
        state
    }
    /// Creates a missing view before any capture/adoption. Never dereference an untrusted ID.
    pub(crate) fn ensure_shared_view(
        &mut self,
        owner: usize,
        target: usize,
    ) -> Result<ViewId, String> {
        self.require_editor_view(owner)?;
        if owner == target {
            self.tabs[owner].shared_view = self.tabs[owner].editor.id;
        }
        let view = self.tabs[target].shared_view;
        if attached(&self.tabs[owner].editor.buffer, view) {
            return Ok(view);
        }
        let state = self.saved_sheet_state(target);
        state.validate()?;
        let view = self.tabs[owner].editor.buffer.new_view();
        state.apply(&mut self.tabs[owner].editor.buffer, view);
        self.tabs[owner]
            .editor
            .restore_folded_view_state(&state, view);
        self.tabs[target].shared_view = view;
        Ok(view)
    }
    /// A failed load still publishes one consistent, read-only document to every sibling.
    pub(crate) fn fail_shared_document(&mut self, index: usize) -> Result<(), String> {
        let identity = self
            .tabs
            .get(index)
            .ok_or("unknown sheet")?
            .session_document;
        let siblings = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| t.session_document == identity)
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        let text = siblings
            .iter()
            .find_map(|i| {
                let t = &self.tabs[*i];
                if !t.editor.buffer.text().is_empty() {
                    Some(t.editor.buffer.text().clone())
                } else {
                    t.lazy
                        .as_ref()
                        .and_then(|(d, _)| d.history.as_ref())
                        .filter(|h| h.retains_text())
                        .and_then(|h| h.current_text().ok())
                }
            })
            .unwrap_or_else(Text::new);
        let dirty = siblings
            .iter()
            .any(|i| self.document_contents_dirty(&self.tabs[*i]));
        let recovered_name = format!("Recovered {}", self.tabs[index].name());
        let states = siblings
            .iter()
            .map(|i| {
                let state = self.saved_sheet_state(*i);
                if state.validate().is_ok() {
                    state
                } else {
                    ViewState::default()
                }
            })
            .collect::<Vec<_>>();
        // Retry checkpoints retain the original native history. Every sibling
        // must therefore retain its original history view, even if that lazy
        // sibling had not started loading when the shared owner failed.
        let retains_history = siblings
            .iter()
            .any(|i| self.tabs[*i].restore_history.is_some());
        let mut buffer = Buffer::new(text.clone());
        if dirty {
            buffer.mark_unsaved();
        }
        let views = states
            .iter()
            .map(|state| {
                let view = buffer.new_view();
                state.apply(&mut buffer, view);
                view
            })
            .collect::<Vec<_>>();
        let document = buffer.id();
        let owner_at = siblings
            .iter()
            .position(|i| *i == index)
            .ok_or("missing error sheet")?;
        for (at, sibling) in siblings.iter().copied().enumerate() {
            let tab = &mut self.tabs[sibling];
            tab.loading = None;
            tab.binary_loading = None;
            tab.previous_document = None;
            if retains_history {
                tab.pending_restore = Some(states[at].clone());
            } else {
                tab.pending_restore = None;
            }
            tab.lazy = None;
            tab.document = document;
            tab.shared_view = views[at];
            tab.editor.buffer = Buffer::new(text.clone());
            tab.editor.id = tab.editor.buffer.new_view();
            states[at].apply(&mut tab.editor.buffer, tab.editor.id);
            tab.editor.shared_shadow = sibling != index;
            tab.shared_dirty = dirty;
            tab.editor.read_only = true;
            tab.load_failed = true;
            if dirty {
                // Actual retained edits are an unsaved recovery document, not a
                // clean error placeholder. They must still prompt and allow Save As.
                tab.path = None;
                tab.suggested_name = Some(recovered_name.clone());
                tab.deleted = false;
                tab.disk_saved = None;
                tab.disk_content = None;
                tab.overwrite_allowed = None;
                tab.binary_document = false;
                tab.editor.binary = None;
                tab.editor.read_only = false;
                tab.load_failed = false;
            }
        }
        self.tabs[index].editor.buffer = buffer;
        self.tabs[index].editor.id = views[owner_at];
        for sibling in siblings {
            self.copy_document_properties(index, sibling);
            let tab = &mut self.tabs[sibling];
            tab.editor.sync();
        }
        Ok(())
    }
    pub(crate) fn validate_document_views(&self) -> Result<(), String> {
        for (index, tab) in self.tabs.iter().enumerate() {
            self.require_editor_view(index)?;
            if let Some(owner) = self.tabs.iter().find(|t| {
                t.document == tab.document && t.lazy.is_none() && t.editor.buffer.id() == t.document
            }) {
                if !attached(&owner.editor.buffer, tab.shared_view) {
                    return Err("shared view is not attached".into());
                }
            } else if tab.lazy.is_none() {
                return Err("shared document has no owner".into());
            }
        }
        Ok(())
    }
}
