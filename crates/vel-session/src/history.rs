//! Native, owned Hot Exit history; encoding and checked import belong on session workers.
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use vel_buffer::{Buffer, HistorySnapshot, Region, Selection, ViewId};
use vel_text::Text;

/// Directed UTF-8 region, including the vertical movement goal.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Position {
    /// Anchor byte.
    pub a: usize,
    /// Caret byte.
    pub b: usize,
    /// Optional preferred visual column.
    pub xpos: Option<f64>,
}
impl From<Region> for Position {
    fn from(r: Region) -> Self {
        Self {
            a: r.a,
            b: r.b,
            xpos: r.xpos,
        }
    }
}
/// View-local session data. All byte positions are clipped to valid boundaries on restore.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ViewState {
    /// Original attached view identity, used only to remap native history.
    #[serde(default)]
    pub history_view: Option<ViewId>,
    /// Sorted directed selections.
    pub selections: Vec<Position>,
    /// Primary selection index.
    pub primary: usize,
    /// Horizontal and vertical logical scroll offsets.
    pub scroll: (f64, f64),
    /// Top visual row plus fractional row offset. Absent in legacy pixel sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_line: Option<f64>,
    /// Folded regions.
    pub folds: Vec<Position>,
    /// Bookmark and mark regions, retained even when native undo history is discarded.
    #[serde(default)]
    pub named_regions: BTreeMap<String, Vec<Position>>,
}
impl Default for ViewState {
    fn default() -> Self {
        Self {
            history_view: None,
            selections: vec![Position::default()],
            primary: 0,
            scroll: (0., 0.),
            scroll_line: None,
            folds: vec![],
            named_regions: BTreeMap::new(),
        }
    }
}
impl ViewState {
    /// Converts a captured view to font-independent vertical coordinates. The legacy
    /// field stays in its historical 20 px grid for compatibility with older readers.
    pub fn with_line_height(mut self, height: f64) -> Self {
        let line = self.scroll.1 / height.max(1.);
        self.scroll_line = Some(line);
        self.scroll.1 = line * 20.;
        self
    }
    /// Migrates pixel sessions with the old line height computed from the view's
    /// effective font size and padding. Current row coordinates are already normalized.
    pub fn top_visual_line(&self, legacy_line_height: f64) -> f64 {
        self.scroll_line
            .unwrap_or(self.scroll.1 / legacy_line_height.max(1.))
            .max(0.)
    }

    /// Copies selections, folds, bookmarks and marks, retaining no document contents.
    pub fn capture(buffer: &Buffer, id: ViewId, scroll: (f64, f64)) -> Self {
        let sel = buffer.view(id).sel();
        Self {
            named_regions: [
                vel_buffer::ops::BOOKMARKS,
                vel_buffer::ops::MARK,
                "velocity.mark.primary",
            ]
            .into_iter()
            .filter_map(|name| {
                buffer
                    .view(id)
                    .get_regions(name)
                    .or_else(|| {
                        (name == vel_buffer::ops::MARK)
                            .then(|| buffer.view(id).get_regions("velocity.mark"))
                            .flatten()
                    })
                    .map(|set| {
                        (
                            name.into(),
                            set.regions.iter().copied().map(Position::from).collect(),
                        )
                    })
            })
            .collect(),
            history_view: Some(id),
            selections: sel.iter().map(Position::from).collect(),
            primary: sel.primary_index(),
            scroll,
            scroll_line: None,
            folds: buffer
                .view(id)
                .get_regions("velocity.folds")
                .map(|s| s.regions.iter().copied().map(Position::from).collect())
                .unwrap_or_default(),
        }
    }
    /// Rejects invalid floating point values or oversized selection arrays.
    pub fn validate(&self) -> Result<(), String> {
        if self.selections.is_empty()
            || self.selections.len() > 1_000_000
            || self.primary >= self.selections.len()
            || self.folds.len() > 1_000_000
            || self.named_regions.len() > 3
            || self.named_regions.values().map(Vec::len).sum::<usize>() > 1_000_000
            || [self.scroll.0, self.scroll.1]
                .iter()
                .any(|n| !n.is_finite() || *n < 0.)
            || self.scroll_line.is_some_and(|n| !n.is_finite() || n < 0.)
            || self
                .selections
                .iter()
                .chain(&self.folds)
                .chain(self.named_regions.values().flatten())
                .any(|p| p.xpos.is_some_and(|n| !n.is_finite()))
        {
            return Err("invalid session view state".into());
        }
        Ok(())
    }
    /// Applies view state through the buffer's public selection/region APIs.
    pub fn apply(&self, buffer: &mut Buffer, id: ViewId) {
        let selection = selection(&self.selections, self.primary, buffer.text());
        buffer.set_selection(id, selection);
        let text = buffer.text();
        let folds = self
            .folds
            .iter()
            .map(|p| {
                Region::new(
                    text.floor_char_boundary(p.a.min(text.len_bytes())),
                    text.floor_char_boundary(p.b.min(text.len_bytes())),
                )
            })
            .filter(|r| !r.empty())
            .collect();
        buffer.add_regions(id, "velocity.folds", folds, true);
        for name in [
            vel_buffer::ops::BOOKMARKS,
            vel_buffer::ops::MARK,
            "velocity.mark.primary",
        ] {
            if let Some(positions) = self.named_regions.get(name) {
                let regions = positions
                    .iter()
                    .map(|p| Region {
                        a: buffer
                            .text()
                            .floor_char_boundary(p.a.min(buffer.text().len_bytes())),
                        b: buffer
                            .text()
                            .floor_char_boundary(p.b.min(buffer.text().len_bytes())),
                        xpos: p.xpos,
                    })
                    .collect();
                buffer.add_regions(id, name, regions, false);
            }
        }
    }
}
pub(super) fn selection(positions: &[Position], primary: usize, text: &Text) -> Selection {
    let mut regions: Vec<_> = positions
        .iter()
        .map(|p| Region {
            a: text.floor_char_boundary(p.a.min(text.len_bytes())),
            b: text.floor_char_boundary(p.b.min(text.len_bytes())),
            xpos: p.xpos,
        })
        .collect();
    // Selection::from_regions makes the last inserted region primary.
    if primary < regions.len() {
        let r = regions.remove(primary);
        regions.push(r);
    }
    Selection::from_regions(regions)
}
/// Complete native history and its separately supplied current rope snapshot.
/// Capture shares text/steps; serialization extracts deltas on the session writer.
#[derive(Clone, Debug)]
pub struct History {
    text: Option<Text>,
    disk: Option<(std::path::PathBuf, String)>,
    snapshot: HistorySnapshot,
}
impl History {
    /// Captures groups, saved point, typing continuation and all attached views.
    pub fn capture(buffer: &Buffer) -> Self {
        Self {
            text: Some(buffer.text().clone()),
            disk: None,
            snapshot: buffer.export_history(),
        }
    }
    /// Captures clean files without duplicating their current disk contents in session JSON.
    /// Native history still retains selection events and any saved text undo/redo steps.
    pub fn capture_saved(buffer: &Buffer, path: std::path::PathBuf, encoding: String) -> Self {
        let mut history = Self::capture(buffer);
        history.disk = Some((path, encoding));
        history
    }
    /// Supplies the current text, lazily reading clean files only on activation.
    pub fn current_text(&self) -> Result<Text, String> {
        if let Some((path, encoding)) = &self.disk {
            return vel_fileio::load(path, encoding, |_| {}).map(|loaded| loaded.text);
        }
        self.text
            .clone()
            .ok_or_else(|| "missing session contents".into())
    }
    /// Whether current contents are retained independently of the clean disk file.
    pub fn retains_text(&self) -> bool {
        self.disk.is_none()
    }
    /// Attached original view IDs. Closed historical owners remain internal to the payload.
    pub fn view_ids(&self) -> Vec<ViewId> {
        self.snapshot.view_ids()
    }
    /// Imports checked history and returns a complete old-to-fresh view mapping.
    /// On disk mismatch, contents survive but text and soft undo stacks are discarded.
    pub fn restore_views(
        &self,
        keep_history: bool,
        dirty: bool,
    ) -> Result<(Buffer, BTreeMap<ViewId, ViewId>), String> {
        let mut map: BTreeMap<_, _> = self
            .view_ids()
            .into_iter()
            .map(|old| (old, ViewId::fresh()))
            .collect();
        let text = self.current_text()?;
        if keep_history {
            return Buffer::restore_history(text, self.snapshot.clone(), &map)
                .map(|b| (b, map))
                .map_err(|e| e.to_string());
        }
        let mut buffer = Buffer::new(text);
        for new in map.values_mut() {
            *new = buffer.new_view();
        }
        if dirty {
            buffer.mark_unsaved();
        }
        Ok((buffer, map))
    }
    /// Convenience import for single-view clients. Multi-view clients use `restore_views`.
    pub fn restore(&self, keep_history: bool, dirty: bool) -> Result<(Buffer, ViewId), String> {
        let (mut buffer, map) = self.restore_views(keep_history, dirty)?;
        let id = map
            .values()
            .next()
            .copied()
            .unwrap_or_else(|| buffer.new_view());
        Ok((buffer, id))
    }
}
impl Serialize for History {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Wire<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            text: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            disk: &'a Option<(std::path::PathBuf, String)>,
            snapshot: &'a HistorySnapshot,
        }
        Wire {
            text: if self.disk.is_none() {
                Some(
                    self.text
                        .as_ref()
                        .ok_or_else(|| serde::ser::Error::custom("missing session contents"))?
                        .to_string(),
                )
            } else {
                None
            },
            disk: &self.disk,
            snapshot: &self.snapshot,
        }
        .serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for History {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Native {
                #[serde(default)]
                text: Option<String>,
                #[serde(default)]
                disk: Option<(std::path::PathBuf, String)>,
                snapshot: HistorySnapshot,
            },
            Legacy(crate::legacy::History),
        }
        match Wire::deserialize(deserializer)? {
            Wire::Native {
                text,
                disk,
                snapshot,
            } => {
                if text.is_none() && disk.is_none() {
                    return Err(serde::de::Error::custom("missing session contents"));
                }
                Ok(Self {
                    text: text.map(|s| Text::from(s.as_str())),
                    disk,
                    snapshot,
                })
            }
            Wire::Legacy(history) => {
                let (buffer, _) = history.restore(true, true);
                Ok(Self::capture(&buffer))
            }
        }
    }
}
