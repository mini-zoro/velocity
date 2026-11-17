//! CPU scenes and shared wrap/fold/IME geometry. This module never initializes a device.
use crate::{
    editor::Editor,
    language::{Language, rgba},
    layout::{self, Options},
};
use std::{
    collections::{BTreeSet, HashMap},
    hash::{Hash, Hasher},
    ops::Range,
    sync::{Arc, OnceLock},
    time::Instant,
};
use vel_color_scheme::{ColorScheme, DecorationOptions, FontStyle as Decoration};
use vel_platform::{Appearance, Point, Rect, Size, Utf16Range};
use vel_render::scene::*;
/// Start/end of the visual row, followed by logical smart-BOL/EOL on repetition.
/// Hard line commands bypass this function and retain the buffer's logical behavior.
pub fn visual_line_boundary(editor: &Editor, byte: usize, end: bool, repeated: bool) -> usize {
    editor.synchronize_layout();
    let layout = editor.layout.borrow();
    let (visual, segment) = layout.visual_at(&editor.text, byte);
    // A fold end can belong to a hidden physical line while its caret is drawn
    // after the header marker. Segment byte ranges belong to the projected row.
    let row = layout.locate(visual).0;
    let start = editor.text.line_to_byte(row);
    if end {
        if let Some(end) = layout.folded_segment_end(&editor.text, row, visual, &segment) {
            return end;
        }
        if repeated {
            start + layout::body_len(&editor.text, row)
        } else {
            let end = start + segment.bytes.end;
            if segment.bytes.end < layout::body_len(&editor.text, row) {
                // Keep the caret on this visual row; a wrap boundary belongs to the next row.
                editor.text.prev_grapheme_boundary(end)
            } else {
                end
            }
        }
    } else if segment.bytes.start > 0 && byte != start + segment.bytes.start && !repeated {
        start + segment.bytes.start
    } else {
        let first = start
            + editor
                .text
                .line(row)
                .chars()
                .take_while(|c| matches!(c, ' ' | '\t'))
                .count();
        if byte == first { start } else { first }
    }
}
/// Resolve wrap settings against the last viewport even when input precedes a frame.
pub(crate) fn motion_layout_options(e: &Editor) -> Options {
    let auto = e
        .language
        .borrow()
        .as_ref()
        .is_none_or(|l| l.root.build_string().starts_with("text"));
    let wrap = e.features.word_wrap
        && e.settings
            .get("word_wrap")
            .and_then(vel_config::Value::as_bool)
            .unwrap_or(auto);
    let viewport = match e.layout.borrow().viewport_width {
        0 => 80,
        width => width,
    };
    let configured = e.settings.usize("wrap_width", 0);
    let width = effective_wrap_columns(
        configured,
        viewport,
        e.settings.string("wrap_width_style", "auto") == "fixed",
    );
    Options {
        width: if wrap { width.max(1) } else { 0 },
        tab: e.preferences.tab_size,
        indent: e.settings.bool("indent_subsequent_lines", true),
    }
}
impl Editor {
    pub(crate) fn synchronize_layout(&self) {
        use vel_buffer::VerticalLayout;
        crate::layout::MotionLayout {
            layout: &self.layout,
            annotations: &self.annotations,
            options: motion_layout_options(self),
        }
        .synchronize(&self.buffer, self.id);
    }
}
pub const TAB_HEIGHT: f64 = 30.;
#[derive(Debug)]
struct CachedUnit {
    bytes: Range<usize>,
    width: usize,
    marker: bool,
}
impl CachedUnit {
    fn text<'a>(&self, line: &'a str) -> &'a str {
        if self.marker {
            "…"
        } else {
            &line[self.bytes.clone()]
        }
    }
}
#[derive(Debug)]
struct CachedLine {
    text: String,
    hidden: Vec<Range<usize>>,
    tab: usize,
    units: Arc<[CachedUnit]>,
}

/// Optional aggregate CPU phases for the headless review profiler.
#[derive(Debug, Default, serde::Serialize)]
pub struct SceneProfile {
    frames: usize,
    total_ms: [f64; 9],
}
#[derive(Debug, PartialEq)]
struct SelectionKey {
    document: (vel_buffer::BufferId, u64, u64),
    regions: Vec<vel_buffer::Region>,
    geometry: [f64; 10],
    style: vel_color_scheme::CornerStyle,
    fill: Color,
    border: Color,
}
#[derive(Debug)]
struct SelectionCache {
    key: SelectionKey,
    contours: Vec<vel_render::selection::Contour>,
}
#[derive(Debug)]
pub struct View {
    pub profile: Option<SceneProfile>,
    units: HashMap<u64, CachedLine>,
    selection_cache: Option<SelectionCache>,
    pub find_matches: Vec<Range<usize>>,
    pub size: Size,
    pub cell: f64,
    pub ascent: f64,
    pub line_height: f64,
    pub scroll_x: f64,
    pub scroll_y: f64,
    pending_scroll_clamp: bool,
    pub appearance: Appearance,
    pub focused: bool,
    pub minimap_hover: bool,
    pub minimap_drag: bool,
    minimap_gesture: Option<MinimapDrag>,
    pub mouse_preferences: vel_platform::mouse::MousePreferences,
    pub gutter_hover: bool,
    pub tab_height: f64,
    /// Drop position while dragging selected text (`drag_text`).
    pub drop_caret: Option<usize>,
    /// Widest line drawn so far for the shown buffer (cells), bounding horizontal scrolling;
    /// `None` width once a windowed long line was drawn (no bound).
    widest: (Option<vel_buffer::BufferId>, Option<usize>),
    scroll_bar: Rect,
    scroll_puck: Rect,
    scroll_bar_drag: bool,
    scroll_bar_hover: bool,
    scroll_drag_offset: f64,
    bar_overlay: bool,
    scrolled: Option<Instant>,
    base_cell: f64,
    base_ascent: f64,
    font_metrics: Option<vel_render::typography::FontMetrics>,
    device_scale: f64,
    line_padding: (f64, f64),
    pub ligatures: bool,
    started: Instant,
}
/// A font- and display-scale-independent viewport location for navigation history.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScrollPosition {
    pub x: f64,
    /// Top visual row, including the fraction of that row above the viewport.
    pub visual_row: f64,
}
impl View {
    /// Saves the top visual row and its fractional offset, rather than pixel y.
    pub(crate) fn scroll_position(&self) -> ScrollPosition {
        ScrollPosition {
            x: self.scroll_x,
            visual_row: self.scroll_y / self.line_height.max(1.),
        }
    }
    /// Resolves a saved row using current metrics/layout and clamps changed documents.
    pub(crate) fn restore_scroll_position(&mut self, e: &Editor, position: ScrollPosition) {
        self.pending_scroll_clamp = false;
        self.scroll_x = position.x.max(0.);
        self.scroll_y = position.visual_row.max(0.) * self.line_height;
        // clamp_scroll applies current metrics and scales this old-height row before
        // clamping. A single style/layout synchronization covers both operations.
        self.clamp_scroll(e);
    }
    /// Keeps the saved row while a file's complete layout is still unavailable.
    pub(crate) fn defer_scroll_restore(&mut self, e: &Editor, position: ScrollPosition) {
        self.scroll_x = position.x.max(0.);
        self.scroll_y = position.visual_row.max(0.) * self.line_height;
        self.pending_scroll_clamp = true;
        self.apply_style(e);
    }
    pub(crate) fn finish_scroll_restore(&mut self, e: &Editor) {
        if std::mem::take(&mut self.pending_scroll_clamp) {
            self.clamp_scroll(e);
        }
    }
    pub(crate) fn pointer_gesture_active(&self) -> bool {
        self.minimap_drag || self.scroll_bar_drag
    }
    pub(crate) fn end_pointer_gesture(&mut self) {
        self.minimap_drag = false;
        self.minimap_gesture = None;
        self.scroll_bar_drag = false;
        self.drop_caret = None;
    }
    #[allow(clippy::too_many_arguments)]
    fn paint_long_line(
        &self,
        e: &Editor,
        row: usize,
        visual: usize,
        scheme: &ColorScheme,
        clip: Rect,
        layer: &mut Layer,
        occurrence: Option<&str>,
        selection_bands: &mut HashMap<usize, Vec<Rect>>,
        shaped_selection: bool,
        collect_selection: bool,
    ) {
        use unicode_segmentation::UnicodeSegmentation;
        use unicode_width::UnicodeWidthStr;
        let left = self.gutter(e) + self.margin(e);
        let first = (self.scroll_x / self.cell).floor().max(0.) as usize;
        let columns = ((self.right(e) - left) / self.cell).ceil().max(1.) as usize + 2;
        let window = e.layout.borrow().long_window(&e.text, row, first, columns);
        let start = e.text.line_to_byte(row);
        let y = self.tab_height + 4. + visual as f64 * self.line_height - self.scroll_y;
        let baseline = self.baseline(e, y);
        if e.preferences.highlight_line && e.text.byte_to_line(e.caret) == row {
            fill(
                layer,
                Rect {
                    x: clip.x,
                    y,
                    width: clip.width,
                    height: self.line_height,
                },
                clip,
                color(scheme, "line_highlight"),
            );
        }
        if e.preferences.line_numbers {
            let number = (row + 1).to_string();
            text_run(
                layer,
                &number,
                self.gutter(e) - 12. - number.len() as f64 * self.cell,
                baseline,
                e.preferences.font_size,
                color(scheme, "gutter_foreground"),
                FontStyle::default(),
                Rect {
                    x: 0.,
                    width: self.gutter(e),
                    ..clip
                },
            );
        }
        let regions = e.regions();
        let body_end = start + e.text.line_len_bytes(row);
        for (index, selection) in regions
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.empty() && r.begin() <= body_end && r.end() > start)
        {
            let a = self.rect_at(e, selection.begin().max(start));
            let b = self.rect_at(e, selection.end().min(body_end));
            let rect = Rect {
                x: a.x,
                y,
                width: (b.x - a.x).max(1.),
                height: self.line_height,
            };
            if !shaped_selection {
                fill(
                    layer,
                    rect,
                    clip,
                    color(
                        scheme,
                        if self.focused {
                            "selection"
                        } else {
                            "inactive_selection"
                        },
                    ),
                );
            }
            if collect_selection {
                selection_bands.entry(index).or_default().push(rect);
            }
        }
        let occurrences: Vec<_> = occurrence
            .map(|word| word_occurrences(&window.text, word).collect())
            .unwrap_or_default();
        let mut occurrence_index = 0;
        let mut column = window.column;
        let mut run = String::new();
        let mut run_x = left + column as f64 * self.cell - self.scroll_x;
        for (byte, grapheme) in window.text.grapheme_indices(true) {
            let width = if grapheme == "\t" {
                e.preferences.tab_size - column % e.preferences.tab_size.max(1)
            } else {
                grapheme.width()
            };
            let at = start + window.bytes.start + byte;
            let x = left + column as f64 * self.cell - self.scroll_x;
            if let Some(range) = occurrences.get(occurrence_index)
                && range.start == byte
            {
                let absolute = start + window.bytes.start + range.start
                    ..start + window.bytes.start + range.end;
                let whole = (absolute.start == 0
                    || !word_character(
                        e.text
                            .char_at(e.text.floor_char_boundary(absolute.start - 1)),
                    ))
                    && (absolute.end == e.text.len_bytes()
                        || !word_character(e.text.char_at(absolute.end)));
                if whole
                    && !regions
                        .iter()
                        .any(|r| !r.empty() && r.to_range() == absolute)
                {
                    outline(
                        layer,
                        Rect {
                            x,
                            y,
                            width: window.text[range.clone()].width() as f64 * self.cell,
                            height: self.line_height,
                        },
                        clip,
                        color(scheme, "selection_border"),
                    );
                }
                occurrence_index += 1;
            }
            if e.marked
                .as_ref()
                .is_some_and(|r| r.start <= at && at < r.end)
            {
                fill(
                    layer,
                    Rect {
                        x,
                        y: y + self.line_height - 2.,
                        width: width as f64 * self.cell,
                        height: 1.,
                    },
                    clip,
                    color(scheme, "caret"),
                );
            }
            if grapheme == "\t" {
                text_run(
                    layer,
                    &run,
                    run_x,
                    baseline,
                    e.preferences.font_size,
                    rgba(scheme.default_style().foreground),
                    FontStyle::default(),
                    clip,
                );
                run.clear();
                run_x = x + width as f64 * self.cell;
            } else {
                run.push_str(grapheme);
            }
            column += width;
        }
        text_run(
            layer,
            &run,
            run_x,
            baseline,
            e.preferences.font_size,
            rgba(scheme.default_style().foreground),
            FontStyle::default(),
            clip,
        );
    }
    fn hex_scene(&mut self, e: &Editor) -> Scene {
        self.apply_style(e);
        let snapshot = e.binary.as_ref().expect("binary tab");
        let scheme = fallback_scheme(self.appearance);
        let clip = Rect {
            x: 0.,
            y: self.tab_height,
            width: self.size.width,
            height: (self.size.height - self.tab_height).max(0.),
        };
        let first = (self.scroll_y / self.line_height).floor().max(0.) as u64;
        let end = (first + self.rows() as u64 + 2).min(snapshot.view.len_rows());
        let mut layer = Layer::default();
        for row in first..end {
            let r = snapshot.view.row(row).expect("bounded row");
            let y = self.tab_height + 4. + row as f64 * self.line_height - self.scroll_y;
            let baseline = self.baseline(e, y);
            text_run(
                &mut layer,
                &format!("{:016x}", r.offset),
                16. - self.scroll_x,
                baseline,
                e.preferences.font_size,
                color(scheme, "gutter_foreground"),
                FontStyle::default(),
                clip,
            );
            for (n, byte) in r.bytes.iter().enumerate() {
                let at = r.offset as usize + n;
                let x = 16. + (18 + n * 3) as f64 * self.cell - self.scroll_x;
                if e.selection().contains(&at) || at == e.caret {
                    fill(
                        &mut layer,
                        Rect {
                            x,
                            y,
                            width: self.cell * 2.,
                            height: self.line_height,
                        },
                        clip,
                        color(scheme, "selection"),
                    );
                    fill(
                        &mut layer,
                        Rect {
                            x: 16. + (68 + n) as f64 * self.cell - self.scroll_x,
                            y,
                            width: self.cell,
                            height: self.line_height,
                        },
                        clip,
                        color(scheme, "selection"),
                    );
                }
                text_run(
                    &mut layer,
                    &format!("{byte:02x}"),
                    x,
                    baseline,
                    e.preferences.font_size,
                    rgba(scheme.default_style().foreground),
                    FontStyle::default(),
                    clip,
                );
            }
            text_run(
                &mut layer,
                std::str::from_utf8(&r.ascii).expect("ASCII row"),
                16. + 68. * self.cell - self.scroll_x,
                baseline,
                e.preferences.font_size,
                rgba(scheme.default_style().foreground),
                FontStyle::default(),
                clip,
            );
        }
        let mut scene = Scene {
            background: rgba(scheme.default_style().background),
            layers: vec![layer],
        };
        self.paint_scrollbar(e, &mut scene);
        scene
    }
    /// Installs known primary metrics; absent/pending font state retains the last valid grid.
    pub fn set_font_metrics(
        &mut self,
        metrics: vel_render::typography::FontMetrics,
        device_scale: f64,
    ) {
        self.font_metrics = Some(metrics);
        self.base_ascent = metrics.ascent;
        self.device_scale = device_scale.max(1.);
        let height = metrics
            .line(
                13. * self.cell / self.base_cell.max(0.001),
                self.device_scale,
                self.line_padding.0,
                self.line_padding.1,
            )
            .0
            .max(1.);
        if height != self.line_height {
            self.scroll_y = self.scroll_y / self.line_height * height;
            self.line_height = height;
        }
    }
    fn baseline(&self, e: &Editor, y: f64) -> f64 {
        y + self.ascent
            + e.settings.number("line_padding_top", 0.).max(0.)
            + if self.font_metrics.is_none() { 3. } else { 0. }
    }
    pub fn reset_caret(&mut self) {
        self.started = Instant::now();
    }
    pub fn new(size: Size, cell: f64, ascent: f64) -> Self {
        Self {
            profile: std::env::var_os("VELOCITY_REVIEW_PROFILE").map(|_| SceneProfile::default()),
            units: HashMap::new(),
            selection_cache: None,
            find_matches: Vec::new(),
            size,
            cell,
            ascent,
            line_height: 20.,
            scroll_x: 0.,
            scroll_y: 0.,
            appearance: Appearance::Dark,
            focused: true,
            minimap_hover: false,
            minimap_drag: false,
            minimap_gesture: None,
            pending_scroll_clamp: false,
            mouse_preferences: Default::default(),
            widest: (None, Some(0)),
            drop_caret: None,
            gutter_hover: false,
            tab_height: TAB_HEIGHT,
            scroll_bar: Rect::default(),
            scroll_puck: Rect::default(),
            scroll_bar_drag: false,
            scroll_bar_hover: false,
            scroll_drag_offset: 0.,
            bar_overlay: true,
            scrolled: None,
            base_cell: cell,
            base_ascent: ascent,
            font_metrics: None,
            device_scale: 1.,
            line_padding: (0., 0.),
            ligatures: false,
            started: Instant::now(),
        }
    }
    fn line_units(&mut self, line: &str, hidden: &[Range<usize>], tab: usize) -> Arc<[CachedUnit]> {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        line.hash(&mut hash);
        hidden.hash(&mut hash);
        tab.hash(&mut hash);
        let key = hash.finish();
        if let Some(cached) = self.units.get(&key)
            && cached.text == line
            && cached.hidden == hidden
            && cached.tab == tab
        {
            return cached.units.clone();
        }
        let units: Arc<[_]> = layout::units(line, hidden, tab)
            .into_iter()
            .map(|u| CachedUnit {
                marker: hidden.contains(&u.bytes),
                bytes: u.bytes,
                width: u.width,
            })
            .collect();
        if line.len() <= 8192 {
            if self.units.len() >= 1024 {
                self.units.clear();
            }
            self.units.insert(
                key,
                CachedLine {
                    text: line.to_owned(),
                    hidden: hidden.to_vec(),
                    tab,
                    units: units.clone(),
                },
            );
        }
        units
    }
    pub fn minimap_width(&self, e: &Editor) -> f64 {
        if e.binary.is_some() {
            return 0.;
        }
        if e.settings.bool("show_minimap", true) {
            100f64.min(self.size.width / 4.)
        } else {
            0.
        }
    }
    pub fn right(&self, e: &Editor) -> f64 {
        self.size.width
            - self.minimap_width(e)
            - if self.bar_overlay {
                0.
            } else {
                8. * vel_ui::theme::ui_scale(&e.settings)
            }
    }
    fn margin(&self, e: &Editor) -> f64 {
        let margin = e.settings.number("margin", 8.).max(0.);
        if !e.settings.bool("draw_centered", false) {
            return margin;
        }
        let configured = e.settings.usize("wrap_width", 0);
        // 0 wraps to the window: https://www.sublimetext.com/docs/distraction_free.html
        if configured == 0 {
            return margin;
        }
        let viewport = ((self.right(e) - self.gutter(e) - margin * 2.).max(self.cell) / self.cell)
            .floor() as usize;
        let columns = effective_wrap_columns(
            configured,
            viewport,
            e.settings.string("wrap_width_style", "auto") == "fixed",
        );
        margin.max((self.right(e) - self.gutter(e) - columns as f64 * self.cell) / 2.)
    }

    pub fn gutter(&self, e: &Editor) -> f64 {
        if e.binary.is_some() {
            return 16.;
        }
        if !e.preferences.gutter {
            0.
        } else if !e.preferences.line_numbers {
            16.
        } else {
            (e.text.len_lines().max(1).ilog10() + 1) as f64 * self.cell + 24.
        }
    }
    pub fn rows(&self) -> usize {
        ((self.size.height - self.tab_height - 8.).max(1.) / self.line_height)
            .floor()
            .max(1.) as usize
    }
    pub fn apply_style(&mut self, e: &Editor) {
        self.bar_overlay = e
            .theme
            .borrow_mut()
            .style(
                e.ui_id(0xE300),
                &vel_theme::ElementPath(&[
                    vel_theme::Element::new("window", &[]),
                    vel_theme::Element::new("scroll_area_control", &[]),
                ]),
                &e.settings,
            )
            .overlay;
        let scale = e.preferences.font_size / 13.;
        self.cell = self.base_cell * scale;
        self.ascent = self.base_ascent * scale;
        let top = e.settings.number("line_padding_top", 0.).max(0.);
        let bottom = e.settings.number("line_padding_bottom", 0.).max(0.);
        self.line_padding = (top, bottom);
        let old_height = self.line_height;
        self.line_height = self
            .font_metrics
            .map_or(20. * scale + top + bottom, |m| {
                m.line(e.preferences.font_size, self.device_scale, top, bottom)
                    .0
            })
            .max(1.);
        if self.line_height != old_height {
            self.scroll_y = self.scroll_y / old_height * self.line_height;
        }
        if e.binary.is_some() {
            e.layout.borrow_mut().page = self.rows();
            return;
        }
        let width = ((self.right(e) - self.gutter(e) - self.margin(e) * 2.).max(self.cell)
            / self.cell)
            .floor() as usize;
        e.layout.borrow_mut().viewport_width = width;
        let folds = e.folded();
        let options = motion_layout_options(e);
        e.layout.borrow_mut().configure(
            &e.text,
            (e.buffer.id(), vel_buffer::ChangeId(e.revision)),
            options,
            &folds,
        );
        let annotations = e.annotations.borrow();
        let mut blocks: Vec<(usize, usize)> = annotations
            .current
            .as_ref()
            .filter(|p| p.identity.1.0 == e.revision)
            .map(|p| {
                p.hunks
                    .iter()
                    .filter(|h| annotations.inline.contains(&h.new.start))
                    .map(|h| (h.new.end, h.old.len().max(1)))
                    .collect()
            })
            .unwrap_or_default();
        let (phantoms, inline) = crate::html_ui::prepare(
            e,
            self.right(e) - self.gutter(e),
            self.cell,
            self.line_height,
        );
        blocks.extend(phantoms);
        e.layout.borrow_mut().set_phantom_inline(&e.text, inline);
        e.layout.borrow_mut().set_inline(blocks);
        e.layout.borrow_mut().page = self.rows();
        if options.width > 0 && options.width <= width {
            self.scroll_x = 0.;
        }
    }
    /// Records the width of a drawn line for the horizontal scroll bound.
    fn note_width(&mut self, e: &Editor, width: Option<usize>) {
        if self.widest.0 != Some(e.buffer.id()) {
            self.widest = (Some(e.buffer.id()), Some(0));
        }
        self.widest.1 = match (self.widest.1, width) {
            (Some(a), Some(b)) => Some(a.max(b)),
            _ => None,
        };
    }
    /// Largest horizontal scroll offset: the widest line seen (or the caret's line) fits at the
    /// right edge, like Sublime; unbounded with word wrap off only after a windowed long line.
    fn max_scroll_x(&mut self, e: &Editor) -> f64 {
        let layout = e.layout.borrow();
        if e.binary.is_some() || layout.wrapped() || layout.total() == 0 {
            return if layout.wrapped() { 0. } else { f64::INFINITY };
        }
        let row = e.text.byte_to_line(e.caret);
        let len = e.text.line_len_bytes(row);
        let caret_line = (len < crate::large::LONG_LINE)
            .then(|| layout.column(&e.text, e.text.line_to_byte(row) + len));
        drop(layout);
        self.note_width(e, caret_line);
        let Some(widest) = self.widest.1 else {
            return f64::INFINITY;
        };
        let visible = self.right(e) - self.gutter(e) - self.margin(e) * 2.;
        (widest as f64 * self.cell + self.cell - visible).max(0.)
    }
    fn max_scroll_y(&self, e: &Editor) -> f64 {
        // The empty margin leaves the last line visible, including at fraction 1.
        // https://docs.sublimetext.io/reference/settings.html#visual-settings
        let viewport = (self.size.height - self.tab_height - 8.).max(0.);
        (self.total(e) as f64 * self.line_height - viewport
            + e.preferences.scroll_past_end * (viewport - self.line_height).max(0.))
        .max(0.)
    }
    pub fn clamp_scroll(&mut self, e: &Editor) {
        self.apply_style(e);
        self.scroll_y = self.scroll_y.clamp(0., self.max_scroll_y(e));
        let max_x = self.max_scroll_x(e);
        self.scroll_x = self.scroll_x.min(max_x).max(0.);
    }
    pub fn scroll(&mut self, e: &Editor, x: f64, y: f64) {
        self.scrolled = Some(Instant::now());
        self.scroll_x += x;
        self.scroll_y += y;
        self.clamp_scroll(e);
    }
    pub fn total(&self, e: &Editor) -> usize {
        e.binary
            .as_ref()
            .map_or_else(|| e.layout.borrow().total(), |b| b.view.len_rows() as usize)
    }
    pub fn rect_at(&self, e: &Editor, byte: usize) -> Rect {
        if e.binary.is_some() {
            return Rect {
                x: 16. + (18 + (byte % 16) * 3) as f64 * self.cell - self.scroll_x,
                y: self.tab_height + 4. + (byte / 16) as f64 * self.line_height - self.scroll_y,
                width: self.cell * 2.,
                height: self.line_height,
            };
        }
        let layout = e.layout.borrow();
        let (visual, _) = layout.visual_at(&e.text, byte);
        Rect {
            x: self.gutter(e)
                + self.margin(e)
                + vel_render::typography::grid_x(layout.column(&e.text, byte), self.cell)
                - self.scroll_x,
            y: self.tab_height + 4. + visual as f64 * self.line_height - self.scroll_y,
            width: 1.,
            height: self.line_height,
        }
    }
    /// Centers a requested file location after the final pane geometry and layout are known.
    pub fn center_caret(&mut self, e: &Editor) {
        self.apply_style(e);
        let r = self.rect_at(e, e.caret);
        let top = self.tab_height + 4.;
        let middle = top + (self.size.height - top - 4.).max(self.line_height) / 2.;
        self.scroll_y += r.y + r.height / 2. - middle;
        self.clamp_scroll(e);
        // Also reveal a column beyond the right edge without disturbing the centered row.
        self.reveal(e);
    }
    pub fn reveal(&mut self, e: &Editor) {
        self.apply_style(e);
        let r = self.rect_at(e, e.caret);
        let context = e
            .settings
            .usize("scroll_context_lines", 0)
            .min(self.rows() / 2) as f64
            * self.line_height;
        let top = self.tab_height + 4. + context;
        let bottom = self.size.height - 4. - context;
        if r.y < top {
            self.scroll_y += r.y - top;
        } else if r.y + r.height > bottom {
            self.scroll_y += r.y + r.height - bottom;
        }
        let left = self.gutter(e) + self.margin(e);
        if r.x < left {
            self.scroll_x = (self.scroll_x + r.x - left).max(0.);
        } else if r.x + self.cell > self.right(e) - self.margin(e) {
            self.scroll_x += r.x + self.cell - self.right(e) + self.margin(e);
        }
        self.clamp_scroll(e);
    }
    pub fn hit_test(&self, e: &Editor, p: Point) -> usize {
        if let Some(binary) = &e.binary {
            let row = ((p.y - self.tab_height - 4. + self.scroll_y) / self.line_height)
                .floor()
                .max(0.) as usize;
            let col = ((p.x - 16. + self.scroll_x) / self.cell).max(0.);
            let cell = if col >= 68. {
                ((col - 68.) + 0.5).floor().min(16.) as usize
            } else {
                (((col - 18.).max(0.) + 0.5) / 3.).floor().min(16.) as usize
            };
            return (row.saturating_mul(16) + cell).min(binary.view.len_bytes() as usize);
        }
        let row = ((p.y - self.tab_height - 4. + self.scroll_y) / self.line_height)
            .floor()
            .max(0.) as usize;
        if let Some((boundary, _)) = e.layout.borrow().inline_at(row)
            && let Some(publication) = e.annotations.borrow().current.as_ref()
            && let Some(hunk) = publication.hunks.iter().find(|h| h.new.end == boundary)
        {
            return crate::annotation_commands::line_byte(&e.text, hunk.new.start);
        }
        let col = ((p.x - self.gutter(e) - self.margin(e) + self.scroll_x) / self.cell).max(0.);
        e.layout.borrow().offset(&e.text, row, col)
    }
    pub fn range_rect(&self, e: &Editor, range: Utf16Range) -> (Rect, Utf16Range) {
        if e.binary.is_some() {
            return (self.rect_at(e, e.caret), Utf16Range::default());
        }
        let bytes = range.bytes(&e.text);
        if bytes.is_empty() {
            return (
                self.rect_at(e, bytes.start),
                Utf16Range::from_bytes(&e.text, bytes),
            );
        }
        let row = e.text.byte_to_line(bytes.start);
        let (_, segment) = e.layout.borrow().visual_at(&e.text, bytes.start);
        let end = bytes.end.min(e.text.line_to_byte(row) + segment.bytes.end);
        let mut r = self.rect_at(e, bytes.start);
        let layout = e.layout.borrow();
        let end_col = if e.text.line_len_bytes(row) >= crate::large::LONG_LINE {
            layout.column(&e.text, end)
        } else {
            let line = e.text.line(row).to_string();
            segment.indent
                + layout::units(&line, layout.hidden(row), e.preferences.tab_size)
                    .iter()
                    .filter(|u| {
                        u.bytes.start >= segment.bytes.start
                            && u.bytes.end <= end - e.text.line_to_byte(row)
                    })
                    .map(|u| u.width)
                    .sum::<usize>()
        };
        r.width =
            ((end_col as f64 - layout.column(&e.text, bytes.start) as f64) * self.cell).max(1.);
        (r, Utf16Range::from_bytes(&e.text, bytes.start..end))
    }
    pub fn viewport(&self, e: &Editor) -> Range<usize> {
        if e.binary.is_some() {
            return 0..0;
        }
        e.layout.borrow().visible_bytes(
            &e.text,
            (self.scroll_y / self.line_height).floor().max(0.) as usize,
            self.rows() + 2,
        )
    }
    pub fn exact(&self, e: &Editor) -> bool {
        if e.binary.is_some() {
            return true;
        }
        if e.marked.is_some() {
            return false;
        }
        let language = e.language.borrow();
        let Some(language) = language.as_ref() else {
            return false;
        };
        let first = (self.scroll_y / self.line_height).floor().max(0.) as usize;
        let layout = e.layout.borrow();
        (first..(first + self.rows() + 1).min(layout.total())).all(|v| {
            language
                .styled
                .get(&layout.locate(v).0)
                .is_some_and(|line| line.exact)
        })
    }
    /// Handles minimap click/drag and gutter fold buttons before text hit testing.
    pub fn pointer(
        &mut self,
        e: &mut Editor,
        point: Point,
        down: bool,
        dragging: bool,
        up: bool,
    ) -> Result<bool, String> {
        self.pointer_with_option(e, point, down, dragging, up, false)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn pointer_with_option(
        &mut self,
        e: &mut Editor,
        point: Point,
        down: bool,
        dragging: bool,
        up: bool,
        option: bool,
    ) -> Result<bool, String> {
        if up && self.scroll_bar_drag {
            self.scroll_bar_drag = false;
            return Ok(true);
        }
        if (down && vel_ui::contains(self.scroll_bar, point)) || (dragging && self.scroll_bar_drag)
        {
            if down
                && !vel_ui::contains(self.scroll_puck, point)
                && !self.mouse_preferences.jumps(option)
            {
                let page = self.scroll_bar.height;
                self.scroll(
                    e,
                    0.,
                    if point.y < self.scroll_puck.y {
                        -page
                    } else {
                        page
                    },
                );
                return Ok(true);
            }
            if down {
                self.scroll_bar_drag = true;
                self.scroll_drag_offset = if vel_ui::contains(self.scroll_puck, point) {
                    point.y - self.scroll_puck.y
                } else {
                    self.scroll_puck.height / 2.
                };
            }
            let track = (self.scroll_bar.height - self.scroll_puck.height).max(1.);
            let fraction =
                ((point.y - self.scroll_bar.y - self.scroll_drag_offset) / track).clamp(0., 1.);
            self.scroll_y = fraction * self.max_scroll_y(e);
            self.scrolled = Some(Instant::now());
            self.clamp_scroll(e);
            return Ok(true);
        }
        self.scroll_bar_hover = point.x >= self.right(e) - 8.
            && point.x < self.right(e) + if self.bar_overlay { 0. } else { 8. }
            && point.y >= self.tab_height;
        self.minimap_hover = point.x >= self.right(e) && point.y >= self.tab_height;
        self.gutter_hover = point.x >= 0. && point.x < self.gutter(e);
        if up {
            let consumed = self.minimap_drag;
            self.minimap_drag = false;
            self.minimap_gesture = None;
            if consumed {
                return Ok(true);
            }
        }
        if (down && self.minimap_hover) || (dragging && self.minimap_drag) {
            if down {
                let map = self.minimap_geometry(e);
                let viewport = map.viewport(self.scroll_y, self.line_height);
                let gesture = if vel_ui::contains(viewport, point) {
                    MinimapDrag::Viewport {
                        grab: point.y - viewport.y,
                    }
                } else {
                    // A click targets the row actually painted beneath the pointer.
                    // Its new viewport may be far from the pointer in a panned map.
                    // Anchor subsequent movement here, with both control ends reachable.
                    let row = map.visual_at(point.y);
                    let visible = (self.size.height - self.tab_height - 8.).max(0.);
                    self.scroll_y = row * self.line_height - visible / 2.;
                    self.clamp_scroll(e);
                    MinimapDrag::Click {
                        press_y: point.y,
                        scroll: self.scroll_y,
                    }
                };
                self.minimap_gesture = Some(gesture);
                self.minimap_drag = true;
            }
            if let Some(gesture) = self.minimap_gesture {
                self.scroll_y = self.minimap_geometry(e).drag_scroll(gesture, point.y);
            }
            self.clamp_scroll(e);
            return Ok(true);
        }
        // Only the fold button column (right of the line numbers) toggles folds; a click on a
        // line number selects that line.
        if down
            && self.gutter_hover
            && e.settings.bool("fold_buttons", true)
            && point.x >= self.gutter(e) - 14.
            && point.y >= self.tab_height
        {
            let visual = ((point.y - self.tab_height - 4. + self.scroll_y) / self.line_height)
                .floor()
                .max(0.) as usize;
            let row = e.layout.borrow().locate(visual).0;
            let existing = e.folded();
            let start = e.text.line_to_byte(row);
            let end = start + e.text.line_len_bytes(row);
            if let Some(fold) = existing
                .get(existing.partition_point(|r| r.begin() < start))
                .filter(|r| r.begin() <= end)
            {
                e.set_folded(existing.iter().filter(|r| *r != fold).copied().collect());
                return Ok(true);
            }
            let fold = e
                .language
                .borrow()
                .as_ref()
                .and_then(|l| l.analysis.as_ref())
                .and_then(|a| {
                    let at = a.folds.partition_point(|f| f.range.start < start);
                    a.folds[at..]
                        .iter()
                        .take_while(|f| f.range.start <= end)
                        .find(|f| !f.attributes)
                })
                .map(|f| f.range.clone());
            if let Some(fold) = fold {
                let mut folds = existing;
                folds.push(vel_buffer::Region::from(fold));
                e.set_folded(folds);
                return Ok(true);
            }
        }
        if down && point.x >= self.gutter(e) && point.y >= self.tab_height {
            let visual = ((point.y - self.tab_height - 4. + self.scroll_y) / self.line_height)
                .floor()
                .max(0.) as usize;
            let folds = e.folded();
            let hit = folds
                .iter()
                .find(|f| {
                    let layout = e.layout.borrow();
                    if layout.visual_at(&e.text, f.begin()).0 != visual {
                        return false;
                    }
                    let x = self.gutter(e)
                        + self.margin(e)
                        + layout.column(&e.text, f.begin()) as f64 * self.cell
                        - self.scroll_x;
                    point.x >= x && point.x < x + self.cell
                })
                .copied();
            if let Some(hit) = hit {
                e.set_folded(folds.into_iter().filter(|f| *f != hit).collect());
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub fn scene(&mut self, e: &Editor) -> Scene {
        let mut phase = self.profile.as_ref().map(|_| Instant::now());
        if e.binary.is_some() {
            return self.hex_scene(e);
        }
        e.theme.borrow().metrics.poll_images();
        e.theme.borrow_mut().begin_frame(Instant::now());
        e.html.borrow_mut().begin_frame();
        if let Some(scene) = crate::html_ui::sheet(e, self) {
            return scene;
        }
        if !e.shared_shadow {
            e.annotations.borrow_mut().sync(&e.buffer, &e.settings);
        }
        e.annotations.borrow_mut().poll();
        self.apply_style(e);
        self.profile_step(&mut phase, 0);
        let first = (self.scroll_y / self.line_height).floor().max(0.) as usize;
        let last = (first + self.rows() + 2).min(e.layout.borrow().total());
        if e.marked.is_none() {
            let layout = e.layout.borrow();
            let mut ranges: Vec<Range<usize>> = Vec::new();
            for visual in first..last {
                let row = layout.locate(visual).0;
                if let Some(range) = ranges.last_mut()
                    && row <= range.end
                {
                    range.end = range.end.max(row + 1);
                } else {
                    ranges.push(row..row + 1);
                }
            }
            drop(layout);
            if let Some(l) = e.language.borrow_mut().as_mut() {
                l.poll();
                l.request_view(e.id, ranges.clone());
                if !ranges.is_empty() {
                    let visible = ranges.first().unwrap().start..ranges.last().unwrap().end;
                    let first = e.text.line_to_byte(visible.start);
                    if let Some(scopes) = l.scope_snapshot_at(first) {
                        let lines = visible.start.max(scopes.range.start)
                            ..visible.end.min(scopes.range.end);
                        e.annotations
                            .borrow_mut()
                            .check(e.annotation_view, scopes, lines);
                    }
                }
            }
        }
        self.profile_step(&mut phase, 1);
        let language = e.language.borrow();
        let language = language.as_ref();
        let scheme = language.map_or_else(|| fallback_scheme(self.appearance), |l| &*l.scheme);
        let default = scheme.default_style();
        let global = |name: &str| color(scheme, name);
        let all = Rect {
            x: 0.,
            y: 0.,
            width: self.size.width,
            height: self.size.height,
        };
        let gutter = self.gutter(e);
        let right = self.right(e);
        let left = gutter + self.margin(e);
        let clip = Rect {
            x: gutter,
            y: self.tab_height,
            width: (right - gutter).max(0.),
            height: (self.size.height - self.tab_height).max(0.),
        };
        let mut layer = Layer {
            font_options: vel_render::typography::FontOptions::parse(
                e.settings
                    .get("font_options")
                    .and_then(vel_config::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(vel_config::Value::as_str),
            ),
            ..Layer::default()
        };
        {
            let mut theme = e.theme.borrow_mut();
            let path = [
                vel_theme::Element::new("window", &[]),
                vel_theme::Element::new("sheet_contents", &[]),
                vel_theme::Element::new(
                    "text_area_control",
                    if self.focused { &["focused"] } else { &[] },
                ),
            ];
            let style = theme.style(e.ui_id(0xE000), &vel_theme::ElementPath(&path), &e.settings);
            theme.surface(&mut layer, &style, clip, all);
        }
        let mut carets = Layer::default();
        // Each split paints its own scheme background before the shared scene is merged.
        fill(
            &mut layer,
            Rect {
                x: 0.,
                y: self.tab_height,
                width: self.size.width,
                height: clip.height,
            },
            all,
            rgba(default.background),
        );
        fill(
            &mut layer,
            Rect {
                x: 0.,
                y: self.tab_height,
                width: gutter,
                height: clip.height,
            },
            all,
            global("gutter"),
        );
        let regions = e.regions();
        let bracket_glyphs = self.bracket_glyph_styles(e, language, scheme);
        let mut selection_border = scheme
            .globals
            .color("selection_border")
            .map(rgba)
            .unwrap_or([0.; 4]);
        let selection_fill = global(if self.focused {
            "selection"
        } else {
            "inactive_selection"
        });
        let selection_style = scheme.selection_corner_style();
        let selection_radius = scheme.selection_corner_radius();
        // Corner treatment applies to the fill independently of a selection border.
        // https://www.sublimetext.com/docs/color_schemes.html#selection
        let shaped_selection =
            selection_radius > 0. && selection_style != vel_color_scheme::CornerStyle::Square;
        let selection_width = scheme
            .globals
            .number("selection_border_width")
            .unwrap_or(1.)
            .max(0.);
        if selection_width == 0. {
            selection_border[3] = 0.;
        }
        let selection_key =
            (shaped_selection && regions.iter().any(|r| !r.empty())).then(|| SelectionKey {
                document: (
                    e.buffer.id(),
                    e.revision,
                    e.layout.borrow().geometry_revision,
                ),
                regions: regions.clone(),
                geometry: [
                    self.scroll_x,
                    self.scroll_y,
                    self.cell,
                    self.line_height,
                    left,
                    clip.width,
                    clip.height,
                    clip.y,
                    selection_radius,
                    selection_width,
                ],
                style: selection_style,
                fill: selection_fill,
                border: selection_border,
            });
        let selection_cached = selection_key.as_ref().is_some_and(|key| {
            self.selection_cache
                .as_ref()
                .is_some_and(|cache| cache.key == *key)
        });
        let collect_selection = !selection_cached && (shaped_selection || selection_border[3] > 0.);
        let mut selection_bands: HashMap<usize, Vec<Rect>> = HashMap::new();
        let caret_row = e.text.byte_to_line(e.caret);
        let folded = e.folded();
        let layout = e.layout.borrow();
        let caret_visuals: BTreeSet<_> = regions
            .iter()
            .map(|r| layout.visual_at(&e.text, r.b).0)
            .collect();
        for ruler in &e.preferences.ruler_entries {
            use vel_config::settings::RulerStyle;
            let rect = Rect {
                x: left + ruler.column as f64 * self.cell - self.scroll_x,
                y: self.tab_height,
                width: e.settings.number("ruler_width", 1.).clamp(0.1, 16.),
                height: clip.height,
            };
            let style =
                ruler
                    .style
                    .unwrap_or_else(|| match e.settings.string("ruler_style", "solid") {
                        "dotted" => RulerStyle::Dotted,
                        "stippled" => RulerStyle::Stippled,
                        _ => RulerStyle::Solid,
                    });
            match style {
                RulerStyle::Solid => fill(&mut layer, rect, clip, global("rulers")),
                RulerStyle::Dotted | RulerStyle::Stippled => {
                    layer.rects.push(SolidRect::patterned_ruler(
                        rect,
                        clip,
                        global("rulers"),
                        style == RulerStyle::Stippled,
                    ))
                }
            }
        }
        let whitespace_selection_only = e.settings.get("draw_white_space").is_some_and(|v| {
            v == "selection"
                || v.as_array()
                    .is_some_and(|a| a.len() == 1 && a[0] == "selection")
        });
        let draw_whitespace = e.settings.string("draw_white_space", "none") != "none"
            || e.settings
                .get("draw_white_space")
                .is_some_and(|v| v.is_array());
        let font_options = e
            .settings
            .get("font_options")
            .and_then(vel_config::Value::as_array);
        let no_bold = font_options.is_some_and(|a| a.iter().any(|s| s == "no_bold"));
        let no_italic = font_options.is_some_and(|a| a.iter().any(|s| s == "no_italic"));
        let indent_guides = e.settings.bool("draw_indent_guides", true);
        let guide_options = e
            .settings
            .get("indent_guide_options")
            .and_then(vel_config::Value::as_array);
        let guides = language.map(|l| &l.guides);
        let active = if guide_options.is_some_and(|o| o.iter().any(|s| s == "draw_active")) {
            guides.and_then(|g| g.active(caret_row, e.preferences.tab_size))
        } else {
            None
        };
        let solid_guides = guide_options.is_some_and(|o| o.iter().any(|s| s == "solid"));
        let normal_guides = guide_options.is_none_or(|o| o.iter().any(|s| s == "draw_normal"));
        let occurrence = selected_word(e, &regions);
        let bookmarks = if e.bookmarks().is_empty() {
            &[]
        } else {
            let visible = layout.visible_bytes(&e.text, first, last.saturating_sub(first));
            visible_bookmarks(e, visible)
        };
        let annotations = e.annotations.borrow();
        let publication = annotations
            .current
            .as_ref()
            .filter(|p| p.identity.1.0 == e.revision && e.marked.is_none());
        self.profile_step(&mut phase, 2);
        for visual in first..last {
            if let Some((boundary, offset)) = layout.inline_at(visual) {
                if let Some(p) = publication
                    && let Some(hunk) = p.hunks.iter().find(|h| {
                        h.new.end == boundary && annotations.inline.contains(&h.new.start)
                    })
                {
                    let y = self.tab_height + 4. + visual as f64 * self.line_height - self.scroll_y;
                    let mut bg = global("line_diff_deleted");
                    bg[3] = 0.15;
                    fill(
                        &mut layer,
                        Rect {
                            x: gutter,
                            y,
                            width: clip.width,
                            height: self.line_height,
                        },
                        clip,
                        bg,
                    );
                    let original = if hunk.old.is_empty() {
                        "(added lines)".into()
                    } else {
                        p.reference.line(hunk.old.start + offset).to_string()
                    };
                    if let Some(changes) = p.inline_spans.get(&hunk.new.start) {
                        let row = hunk.old.start + offset;
                        if row < p.reference.len_lines() {
                            let start = p.reference.line_to_byte(row);
                            let end = start + original.trim_end_matches('\n').len();
                            for change in changes
                                .iter()
                                .filter(|c| c.old.start < end && c.old.end > start)
                            {
                                let a = crate::editor::cells_with_tab(
                                    &original[..change.old.start.max(start) - start],
                                    e.preferences.tab_size,
                                );
                                let b = crate::editor::cells_with_tab(
                                    &original[..change.old.end.min(end) - start],
                                    e.preferences.tab_size,
                                );
                                let mut color = global("line_diff_deleted");
                                color[3] = 0.35;
                                fill(
                                    &mut layer,
                                    Rect {
                                        x: left + (a + 2) as f64 * self.cell - self.scroll_x,
                                        y,
                                        width: b.saturating_sub(a) as f64 * self.cell,
                                        height: self.line_height,
                                    },
                                    clip,
                                    color,
                                );
                            }
                        }
                    }
                    text_run(
                        &mut layer,
                        &format!("− {}", original.trim_end_matches('\n')),
                        left - self.scroll_x,
                        self.baseline(e, y),
                        e.preferences.font_size,
                        global("line_diff_deleted"),
                        FontStyle::default(),
                        clip,
                    );
                }
                continue;
            }
            let (row, subrow) = layout.locate(visual);
            let segment = layout.segment(&e.text, row, subrow);
            if !layout.wrapped() && e.text.line_len_bytes(row) >= crate::large::LONG_LINE {
                self.note_width(e, None);
                self.paint_long_line(
                    e,
                    row,
                    visual,
                    scheme,
                    clip,
                    &mut layer,
                    occurrence.as_deref(),
                    &mut selection_bands,
                    shaped_selection,
                    collect_selection,
                );
                continue;
            }
            let line = e.text.line(row).to_string();
            let start = e.text.line_to_byte(row);
            let y = self.tab_height + 4. + visual as f64 * self.line_height - self.scroll_y;
            let baseline = self.baseline(e, y);
            if (e.preferences.highlight_line || e.command_mode()) && caret_visuals.contains(&visual)
            {
                fill(
                    &mut layer,
                    Rect {
                        x: gutter,
                        y,
                        width: clip.width,
                        height: self.line_height,
                    },
                    clip,
                    global("line_highlight"),
                );
            }
            if subrow == 0 && e.preferences.gutter {
                if let Some(p) = publication {
                    let at = p
                        .hunks
                        .partition_point(|h| h.new.end.max(h.new.start + 1) <= row);
                    if let Some(h) = p.hunks.get(at).filter(|h| {
                        h.new.start.min(e.text.len_lines() - 1) <= row
                            && row
                                < h.new
                                    .end
                                    .max(h.new.start + 1)
                                    .max(e.text.len_lines().min(h.new.start + 1))
                    }) {
                        let name = match h.kind() {
                            vel_diff::HunkKind::Added => "line_diff_added",
                            vel_diff::HunkKind::Modified => "line_diff_modified",
                            vel_diff::HunkKind::Deleted => "line_diff_deleted",
                        };
                        let width = scheme
                            .globals
                            .number("line_diff_width")
                            .unwrap_or(3.)
                            .clamp(1., 8.);
                        let deleted = h.kind() == vel_diff::HunkKind::Deleted;
                        fill(
                            &mut layer,
                            Rect {
                                x: gutter - width,
                                y: if deleted { y - 1. } else { y },
                                width: if deleted { width + 4. } else { width },
                                height: if deleted { 2. } else { self.line_height },
                            },
                            all,
                            global(name),
                        );
                    }
                }
                if !bookmarks.is_empty()
                    && bookmarked_line(
                        bookmarks,
                        start..if row + 1 < e.text.len_lines() {
                            e.text.line_to_byte(row + 1)
                        } else {
                            e.text.len_bytes() + 1
                        },
                    )
                {
                    text_run(
                        &mut layer,
                        "◆",
                        1.,
                        baseline,
                        e.preferences.font_size,
                        global("gutter_foreground"),
                        FontStyle::default(),
                        Rect {
                            width: gutter,
                            ..all
                        },
                    );
                }
                if e.preferences.line_numbers {
                    let number =
                        if e.settings.bool("relative_line_numbers", false) && row != caret_row {
                            row.abs_diff(caret_row).to_string()
                        } else {
                            (row + 1).to_string()
                        };
                    text_run(
                        &mut layer,
                        &number,
                        gutter - 12. - number.len() as f64 * self.cell,
                        baseline,
                        e.preferences.font_size,
                        global("gutter_foreground"),
                        FontStyle::default(),
                        Rect {
                            width: gutter,
                            ..all
                        },
                    );
                }
                if e.settings.bool("fold_buttons", true)
                    && (!e.settings.bool("fade_fold_buttons", true) || self.gutter_hover)
                {
                    let header_end = start + line.trim_end_matches('\n').len();
                    let is_folded = folded
                        .get(folded.partition_point(|r| r.begin() < start))
                        .is_some_and(|r| r.begin() <= header_end);
                    let candidate = language.and_then(|l| l.analysis.as_ref()).is_some_and(|a| {
                        let at = a.folds.partition_point(|f| f.range.start < start);
                        a.folds[at..]
                            .iter()
                            .take_while(|f| f.range.start <= header_end)
                            .any(|f| !f.attributes)
                    });
                    if is_folded || candidate {
                        let mut theme = e.theme.borrow_mut();
                        let attrs = if is_folded {
                            &["expandable"][..]
                        } else {
                            &["expandable", "expanded"][..]
                        };
                        let path = [
                            vel_theme::Element::new("window", &[]),
                            vel_theme::Element::new("text_area_control", &[]),
                            vel_theme::Element::new("fold_button_control", attrs),
                        ];
                        let style = theme.style(
                            e.ui_id(0xE100_0000 + row as u32),
                            &vel_theme::ElementPath(&path),
                            &e.settings,
                        );
                        theme.surface(
                            &mut layer,
                            &style,
                            Rect {
                                x: gutter - 12.,
                                y: baseline - 10.,
                                width: 10.,
                                height: 10.,
                            },
                            all,
                        );
                    }
                }
            }
            let display = self.line_units(&line, layout.hidden(row), e.preferences.tab_size);
            if !layout.wrapped() {
                let width = segment.indent + display.iter().map(|u| u.width).sum::<usize>();
                self.note_width(e, Some(width));
            }
            let inline_phantoms = layout.has_phantom_inline();
            let column_at = |byte: usize| {
                if inline_phantoms {
                    return layout.column(&e.text, byte);
                }
                segment.indent
                    + display
                        .iter()
                        .filter(|u| {
                            u.bytes.start >= segment.bytes.start && u.bytes.end <= byte - start
                        })
                        .map(|u| u.width)
                        .sum::<usize>()
            };
            if let Some(p) = publication {
                for (&hunk, changes) in &p.inline_spans {
                    if !annotations.inline.contains(&hunk) {
                        continue;
                    }
                    let begin = start + segment.bytes.start;
                    let end = start + segment.bytes.end;
                    for change in changes
                        .iter()
                        .filter(|c| c.new.start < end && c.new.end > begin)
                    {
                        let a = column_at(change.new.start.max(begin));
                        let b = column_at(change.new.end.min(end));
                        let mut color = global("line_diff_added");
                        color[3] = 0.25;
                        fill(
                            &mut layer,
                            Rect {
                                x: left + a as f64 * self.cell - self.scroll_x,
                                y,
                                width: b.saturating_sub(a) as f64 * self.cell,
                                height: self.line_height,
                            },
                            clip,
                            color,
                        );
                    }
                }
            }
            if let Some(p) = publication
                && e.settings.bool("spell_check", false)
                && p.checked.contains(&row)
            {
                let begin = start + segment.bytes.start;
                let end = start + segment.bytes.end;
                for error in p
                    .spelling
                    .iter()
                    .filter(|m| m.range.start < end && m.range.end > begin)
                {
                    let a = column_at(error.range.start.max(begin));
                    let b = column_at(error.range.end.min(end));
                    let x = left + a as f64 * self.cell - self.scroll_x;
                    let width = b.saturating_sub(a) as f64 * self.cell;
                    layer.underlines.push(vel_render::scene::Underline {
                        rect: Rect {
                            x,
                            y: y + self.line_height - 3.,
                            width,
                            height: 2.,
                        },
                        thickness: 1.,
                        squiggle: true,
                        stippled: false,
                        color: global("misspelling"),
                        clip,
                    });
                }
            }
            let relevant: Vec<_> = display
                .iter()
                .filter(|u| {
                    u.bytes.start >= segment.bytes.start && u.bytes.end <= segment.bytes.end
                })
                .collect();
            let mut column = segment.indent;
            let range_start = start + segment.bytes.start;
            let range_end = start + segment.bytes.end;
            for (selection_index, selection) in regions
                .iter()
                .enumerate()
                .filter(|(_, r)| !r.empty() && r.begin() <= range_end && r.end() > range_start)
            {
                let a = relevant
                    .iter()
                    .take_while(|u| start + u.bytes.end <= selection.begin())
                    .map(|u| u.width)
                    .sum::<usize>();
                let b = relevant
                    .iter()
                    .take_while(|u| start + u.bytes.start < selection.end())
                    .map(|u| u.width)
                    .sum::<usize>();
                let rect = Rect {
                    x: left + (segment.indent + a) as f64 * self.cell - self.scroll_x,
                    y,
                    width: ((b - a) as f64 * self.cell).max(1.),
                    height: self.line_height,
                };
                if !shaped_selection {
                    fill(&mut layer, rect, clip, selection_fill);
                }
                if collect_selection {
                    selection_bands
                        .entry(selection_index)
                        .or_default()
                        .push(rect);
                }
            }
            if let Some(fields) = &e.fields {
                for field in fields
                    .fields()
                    .iter()
                    .filter(|f| f.range.start <= range_end && f.range.end > range_start)
                {
                    let a = relevant
                        .iter()
                        .take_while(|u| start + u.bytes.end <= field.range.start)
                        .map(|u| u.width)
                        .sum::<usize>();
                    let b = relevant
                        .iter()
                        .take_while(|u| start + u.bytes.start < field.range.end)
                        .map(|u| u.width)
                        .sum::<usize>();
                    border(
                        &mut layer,
                        Rect {
                            x: left + (segment.indent + a) as f64 * self.cell - self.scroll_x,
                            y,
                            width: ((b - a) as f64 * self.cell).max(2.),
                            height: self.line_height,
                        },
                        clip,
                        if field.mirror {
                            [0.45, 0.56, 0.65, 0.8]
                        } else {
                            [0.55, 0.75, 0.95, 1.]
                        },
                    );
                }
            }
            if indent_guides {
                let leading = line.len() - line.trim_start_matches([' ', '\t']).len();
                let columns = guides.and_then(|g| g.columns(row)).unwrap_or_else(|| {
                    crate::editor::cells_with_tab(&line[..leading], e.preferences.tab_size)
                });
                for col in vel_render::guides::columns(columns, e.preferences.tab_size) {
                    let active = active
                        .as_ref()
                        .is_some_and(|a| a.column == col && a.rows.contains(&row));
                    if active || normal_guides {
                        let guide = Rect {
                            x: left + col as f64 * self.cell - self.scroll_x,
                            y,
                            width: 1.,
                            height: self.line_height,
                        };
                        paint_guide(
                            &mut layer,
                            guide,
                            clip,
                            global(if active { "active_guide" } else { "guide" }),
                            solid_guides,
                        );
                    }
                }
            }
            if let Some(word) = &occurrence {
                // Matches and display units advance together: repeated short words
                // on a long physical line must not cause quadratic geometry scans.
                let mut units = relevant.iter().copied().peekable();
                let mut col = segment.indent;
                let width = |u: &CachedUnit| {
                    u.width
                        + if inline_phantoms {
                            layout.phantom_gap(start + u.bytes.start)
                        } else {
                            0
                        }
                };
                for range in word_occurrences(&line, word) {
                    let absolute = start + range.start..start + range.end;
                    if absolute.end <= range_start {
                        continue;
                    }
                    if absolute.start >= range_end {
                        break;
                    }
                    while units.peek().is_some_and(|u| u.bytes.end <= range.start) {
                        col += width(units.next().expect("peeked unit"));
                    }
                    let a = col;
                    while units.peek().is_some_and(|u| u.bytes.start < range.end) {
                        col += width(units.next().expect("peeked unit"));
                    }
                    let x = left + a as f64 * self.cell - self.scroll_x;
                    if x >= right {
                        break;
                    }
                    if x + (col - a) as f64 * self.cell < gutter {
                        continue;
                    }
                    if regions
                        .get(regions.partition_point(|r| r.end() < absolute.end))
                        .is_some_and(|r| !r.empty() && r.to_range() == absolute)
                        || layout
                            .hidden(row)
                            .iter()
                            .any(|r| r.start < range.end && range.start < r.end)
                    {
                        continue;
                    }
                    outline(
                        &mut layer,
                        Rect {
                            x,
                            y,
                            width: ((col - a) as f64 * self.cell).max(1.),
                            height: self.line_height,
                        },
                        clip,
                        global("selection_border"),
                    );
                }
            }
            let mut run = String::new();
            let mut run_x = 0.;
            let mut run_style: Option<(Color, FontStyle, Decoration)> = None;
            // Styled spans and display units are ordered. Walk them together instead of
            // restarting the span search for every grapheme while painting themed scenes.
            let spans = if e.marked.is_none() {
                language
                    .and_then(|l| l.styled.get(&row))
                    .map(|line| line.spans.as_ref())
                    .unwrap_or(&[])
            } else {
                &[]
            };
            debug_assert!(
                spans
                    .windows(2)
                    .all(|pair| pair[0].range.end <= pair[1].range.start)
            );
            let mut previous_span = None;
            let mut span = spans.partition_point(|s| s.range.end <= segment.bytes.start);
            for unit in relevant {
                let gap = if inline_phantoms {
                    layout.phantom_gap(start + unit.bytes.start)
                } else {
                    0
                };
                if gap > 0 {
                    if let Some((fg, font, decoration)) = run_style.take() {
                        self.flush(
                            &mut layer, &mut run, run_x, baseline, fg, font, decoration, clip, e,
                        );
                    }
                    column += gap;
                }
                let text = unit.text(&line);
                let x = left + column as f64 * self.cell - self.scroll_x;
                let at = start + unit.bytes.start;
                let selected = regions
                    .get(regions.partition_point(|r| r.end() <= at))
                    .is_some_and(|r| !r.empty() && r.begin() <= at && at < r.end());
                let found = self
                    .find_matches
                    .get(self.find_matches.partition_point(|r| r.end <= at))
                    .is_some_and(|r| r.contains(&at));
                while spans
                    .get(span)
                    .is_some_and(|s| s.range.end <= unit.bytes.start)
                {
                    span += 1;
                }
                let style = spans
                    .get(span)
                    .filter(|s| s.range.contains(&unit.bytes.start))
                    .map_or(default, |s| s.style);
                let bracket = if bracket_glyphs.is_empty() {
                    None
                } else {
                    bracket_glyphs
                        .iter()
                        .find(|(range, _, _)| range.contains(&at))
                };
                let fg = if selected {
                    style.selection_foreground.map(rgba).unwrap_or_else(|| {
                        global(if self.focused {
                            "selection_foreground"
                        } else {
                            "inactive_selection_foreground"
                        })
                    })
                } else if let Some((_, fg, options)) =
                    bracket.filter(|(_, _, o)| o.contains(DecorationOptions::FOREGROUND))
                {
                    let _ = options;
                    *fg
                } else if found {
                    global("find_highlight_foreground")
                } else if text == "…" {
                    global("fold_marker")
                } else {
                    rgba(style.foreground)
                };
                let font = FontStyle {
                    bold: (style.font_style.contains(Decoration::BOLD)
                        || bracket.is_some_and(|(_, _, o)| o.contains(DecorationOptions::BOLD)))
                        && !no_bold,
                    italic: (style.font_style.contains(Decoration::ITALIC)
                        || bracket.is_some_and(|(_, _, o)| o.contains(DecorationOptions::ITALIC)))
                        && !no_italic,
                };
                let signature = (fg, font, style.font_style);
                let visible = x + unit.width as f64 * self.cell >= gutter && x < right;
                if (run_style != Some(signature)
                    || (self.ligatures && previous_span != Some(span))
                    || text == "\t"
                    || !visible)
                    && let Some((fg, font, decoration)) = run_style.take()
                {
                    self.flush(
                        &mut layer, &mut run, run_x, baseline, fg, font, decoration, clip, e,
                    );
                }
                previous_span = Some(span);
                if visible && text != "\t" {
                    if run.is_empty() {
                        run_x = x;
                        run_style = Some(signature);
                    }
                    run.push_str(text);
                }
                if visible && (found || style.background != default.background) && !selected {
                    fill(
                        &mut layer,
                        Rect {
                            x,
                            y,
                            width: unit.width as f64 * self.cell,
                            height: self.line_height,
                        },
                        clip,
                        if found {
                            global("find_highlight")
                        } else {
                            rgba(style.background)
                        },
                    );
                }
                if visible
                    && matches!(text, " " | "\t")
                    && draw_whitespace
                    && (!whitespace_selection_only || selected)
                    && whitespace(e, &line, unit.bytes.start, selected)
                {
                    text_run(
                        &mut layer,
                        if text == "\t" { "→" } else { "·" },
                        x,
                        baseline,
                        e.preferences.font_size,
                        global("invisibles"),
                        FontStyle::default(),
                        clip,
                    );
                }
                column += unit.width;
            }
            if let Some((fg, font, decoration)) = run_style {
                self.flush(
                    &mut layer, &mut run, run_x, baseline, fg, font, decoration, clip, e,
                );
            }
            if let Some(marked) = &e.marked {
                let a = marked.start.max(range_start);
                let b = marked.end.min(range_end);
                if a < b {
                    let x = left + layout.column(&e.text, a) as f64 * self.cell - self.scroll_x;
                    let width =
                        (layout.column(&e.text, b) - layout.column(&e.text, a)) as f64 * self.cell;
                    underline(
                        &mut layer,
                        Rect {
                            x,
                            y: y + self.line_height - 2.,
                            width,
                            height: 1.5,
                        },
                        rgba(default.foreground),
                        "underline",
                        clip,
                    );
                }
            }
        }
        let contour_start = layer.contours.len();
        if selection_cached {
            layer.contours.extend(
                self.selection_cache
                    .as_ref()
                    .unwrap()
                    .contours
                    .iter()
                    .cloned(),
            );
        }
        for bands in selection_bands.into_values() {
            for points in vel_render::selection::outlines(&bands) {
                if shaped_selection {
                    let mut contour = vel_render::selection::Contour::new(
                        &points,
                        corner_style(selection_style),
                        selection_radius,
                        selection_fill,
                        selection_border,
                        clip,
                    );
                    contour.border_width = selection_width;
                    layer.contours.push(contour);
                } else {
                    // Square/zero-radius borders need only the exposed union edges; no
                    // rounded geometry or extra primitives at internal line boundaries.
                    for (a, b) in points
                        .iter()
                        .zip(points.iter().cycle().skip(1))
                        .take(points.len())
                    {
                        fill(
                            &mut layer,
                            Rect {
                                x: a.x.min(b.x),
                                y: a.y.min(b.y),
                                width: (b.x - a.x).abs().max(selection_width),
                                height: (b.y - a.y).abs().max(selection_width),
                            },
                            clip,
                            selection_border,
                        );
                    }
                }
            }
        }
        if !selection_cached {
            self.selection_cache = selection_key.map(|key| SelectionCache {
                key,
                contours: layer.contours[contour_start..].to_vec(),
            });
        }
        drop(layout);
        self.profile_step(&mut phase, 3);
        self.paint_brackets(e, language, scheme, clip, &mut layer);
        let alpha = caret_alpha(
            e.settings.string("caret_style", "solid"),
            self.started.elapsed().as_secs_f64(),
        );
        for r in regions {
            let visual = e.layout.borrow().visual_at(&e.text, r.b).0;
            if !(first..last).contains(&visual) {
                continue;
            }
            let mut caret = self.rect_at(e, r.b);
            caret.width += e.settings.number("caret_extra_width", 0.).max(0.);
            if e.command_mode() || e.preferences.caret_style == "block" {
                caret.width = self.cell;
            } else if e.preferences.caret_style == "underline" {
                caret.y += caret.height - 2.;
                caret.height = 2.;
                caret.width = self.cell;
            }
            let mut color = global("caret");
            color[3] *= alpha * if e.command_mode() { 0.45 } else { 1. };
            if self.focused {
                if (e.command_mode() || e.preferences.caret_style == "block")
                    && scheme.block_caret_corner_radius() > 0.
                    && scheme.block_caret_corner_style() != vel_color_scheme::CornerStyle::Square
                {
                    if scheme.block_caret_corner_style() == vel_color_scheme::CornerStyle::Round {
                        carets.rects.push(SolidRect {
                            rect: caret,
                            clip,
                            color,
                            radius: scheme.block_caret_corner_radius() as f32,
                        });
                    } else {
                        carets.contours.push(vel_render::selection::Contour::new(
                            &vel_render::selection::outlines(&[caret])[0],
                            corner_style(scheme.block_caret_corner_style()),
                            scheme.block_caret_corner_radius(),
                            color,
                            [0.; 4],
                            clip,
                        ));
                    }
                } else {
                    fill(&mut carets, caret, clip, color);
                }
            }
        }
        if let Some(at) = self.drop_caret {
            let mut caret = self.rect_at(e, at);
            caret.width = 2.;
            fill(&mut carets, caret, clip, global("caret"));
        }
        let mut minimap = Layer {
            minimap: true,
            ..Layer::default()
        };
        self.profile_step(&mut phase, 4);
        self.paint_minimap(e, language, scheme, &mut minimap);
        self.profile_step(&mut phase, 5);
        let layers = vec![layer, carets, minimap];
        let mut scene = Scene {
            background: rgba(default.background),
            layers,
        };
        drop(annotations);
        self.paint_scrollbar(e, &mut scene);
        self.profile_step(&mut phase, 6);
        e.completion.paint(e, self, &mut scene);
        self.profile_step(&mut phase, 7);
        crate::html_ui::paint(e, self, &mut scene);
        e.annotations.borrow_mut().flush();
        self.profile_step(&mut phase, 8);
        if let Some(profile) = &mut self.profile {
            profile.frames += 1;
        }
        scene
    }
    fn profile_step(&mut self, phase: &mut Option<Instant>, index: usize) {
        if let Some(at) = phase {
            let now = Instant::now();
            self.profile.as_mut().expect("profiling enabled").total_ms[index] +=
                now.duration_since(*at).as_secs_f64() * 1000.;
            *at = now;
        }
    }
    #[cfg(test)]
    pub(crate) fn scrollbar_rects(&self) -> (Rect, Rect) {
        (self.scroll_bar, self.scroll_puck)
    }
    #[cfg(test)]
    pub(crate) fn minimap_rects(&self, e: &Editor) -> (Rect, Rect) {
        let map = self.minimap_geometry(e);
        (map.bounds, map.viewport(self.scroll_y, self.line_height))
    }
    #[cfg(test)]
    pub(crate) fn minimap_visual_at(&self, e: &Editor, y: f64) -> f64 {
        self.minimap_geometry(e).visual_at(y)
    }
    fn paint_scrollbar(&mut self, e: &Editor, scene: &mut Scene) {
        let ui_scale = vel_ui::theme::ui_scale(&e.settings);
        let height = (self.size.height - self.tab_height).max(1.);
        let content = height + self.max_scroll_y(e);
        if content <= height {
            self.scroll_bar = Rect::default();
            return;
        }
        let elapsed = self
            .scrolled
            .map_or(f32::INFINITY, |at| at.elapsed().as_secs_f32());
        let hovered = self.scroll_bar_hover || self.scroll_bar_drag;
        if self.bar_overlay && !hovered && elapsed >= 0.8 {
            self.scroll_bar = Rect::default();
            return;
        }
        let bounds = Rect {
            x: if self.bar_overlay {
                self.right(e) - 8. * ui_scale
            } else {
                self.right(e)
            },
            y: self.tab_height,
            width: 8. * ui_scale,
            height,
        };
        self.scroll_bar = bounds;
        let mut theme = e.theme.borrow_mut();
        let path = [
            vel_theme::Element::new("window", &[]),
            vel_theme::Element::new("scroll_area_control", &[]),
            vel_theme::Element::new("scroll_bar_control", &["vertical"]),
        ];
        let style = theme.style(e.ui_id(0xE301), &vel_theme::ElementPath(&path), &e.settings);
        let mut layer = Layer::default();
        theme.surface(&mut layer, &style, bounds, bounds);
        let track_path = [
            path[0],
            path[1],
            path[2],
            vel_theme::Element::new("scroll_track_control", &[]),
        ];
        let track = theme.style(
            e.ui_id(0xE302),
            &vel_theme::ElementPath(&track_path),
            &e.settings,
        );
        theme.surface(&mut layer, &track, bounds, bounds);
        let puck_path = [
            path[0],
            path[1],
            path[2],
            vel_theme::Element::new("puck_control", if hovered { &["hover"] } else { &[] }),
        ];
        let mut puck = theme.style(
            e.ui_id(0xE303),
            &vel_theme::ElementPath(&puck_path),
            &e.settings,
        );
        let puck_height = (height * height / content)
            .max(f64::from(puck.min_size[1]).max(24. * ui_scale))
            .min(height);
        let rect = Rect {
            x: bounds.x + ui_scale,
            y: bounds.y
                + (self.scroll_y / (content - height)).clamp(0., 1.) * (height - puck_height),
            width: (bounds.width - 2. * ui_scale).max(1.),
            height: puck_height,
        };
        self.scroll_puck = rect;
        if self.bar_overlay && !hovered {
            puck.multiply_opacity(((0.8 - elapsed) / 0.2).clamp(0., 1.));
            if e.settings.bool("animation_enabled", true) {
                theme.request_animation();
            }
        }
        theme.surface(&mut layer, &puck, rect, bounds);
        scene.layers.push(layer);
    }
    #[allow(clippy::too_many_arguments)]
    fn flush(
        &self,
        layer: &mut Layer,
        run: &mut String,
        x: f64,
        baseline: f64,
        color: Color,
        font: FontStyle,
        decoration: Decoration,
        clip: Rect,
        e: &Editor,
    ) {
        if run.is_empty() {
            return;
        }
        if decoration.contains(Decoration::UNDERLINE)
            || decoration.contains(Decoration::STIPPLED_UNDERLINE)
            || decoration.contains(Decoration::SQUIGGLY_UNDERLINE)
        {
            let options = if decoration.contains(Decoration::SQUIGGLY_UNDERLINE) {
                "squiggly_underline"
            } else if decoration.contains(Decoration::STIPPLED_UNDERLINE) {
                "stippled_underline"
            } else {
                "underline"
            };
            underline(
                layer,
                Rect {
                    x,
                    y: baseline + 2.,
                    width: crate::editor::cells_with_tab(run, e.preferences.tab_size) as f64
                        * self.cell,
                    height: 3.,
                },
                color,
                options,
                clip,
            );
        }
        let text = TextRun {
            family: None,
            text: std::mem::take(run),
            x: x as f32,
            baseline: baseline as f32,
            size: e.preferences.font_size as f32,
            color,
            style: font,
            clip,
        };
        // https://www.sublimetext.com/docs/color_schemes.html#style-rules
        if decoration.contains(Decoration::GLOW) {
            layer.glow.push(text.clone());
        }
        layer.text.push(text);
    }
    fn bracket_glyph_styles(
        &self,
        e: &Editor,
        language: Option<&Language>,
        scheme: &ColorScheme,
    ) -> Vec<(Range<usize>, Color, DecorationOptions)> {
        let mut result = Vec::new();
        if ![
            scheme.brackets_options(),
            scheme.bracket_contents_options(),
            scheme.tags_options(),
        ]
        .iter()
        .flatten()
        .any(|o| {
            o.contains(DecorationOptions::FOREGROUND)
                || o.contains(DecorationOptions::BOLD)
                || o.contains(DecorationOptions::ITALIC)
        }) {
            return result;
        }
        let Some(analysis) = language.and_then(|l| l.analysis.as_ref()) else {
            return result;
        };
        for region in e.regions() {
            if let Some(pair) = analysis.pair_at(
                region.b,
                e.settings.bool("match_brackets_content", true),
                |kind| match kind {
                    't' => e.settings.bool("match_tags", true),
                    _ if !e.settings.bool("match_brackets", true) => false,
                    '[' => e.settings.bool("match_brackets_square", true),
                    '{' => e.settings.bool("match_brackets_braces", true),
                    '<' => e.settings.bool("match_brackets_angle", false),
                    _ => true,
                },
            ) {
                let (key, options) = if pair.kind == 't' {
                    ("tags_foreground", scheme.tags_options())
                } else if pair.open.end < region.b && region.b < pair.close.start {
                    (
                        "bracket_contents_foreground",
                        scheme.bracket_contents_options(),
                    )
                } else {
                    ("brackets_foreground", scheme.brackets_options())
                };
                if let Some(options) = options {
                    for range in [&pair.open, &pair.close] {
                        result.push((range.clone(), color(scheme, key), options));
                    }
                }
            }
        }
        result
    }
    fn paint_brackets(
        &self,
        e: &Editor,
        language: Option<&Language>,
        scheme: &ColorScheme,
        clip: Rect,
        layer: &mut Layer,
    ) {
        if !e.settings.bool("match_brackets", true) && !e.settings.bool("match_tags", true) {
            return;
        }
        let Some(analysis) = language.and_then(|l| l.analysis.as_ref()) else {
            return;
        };
        let content = e.settings.bool("match_brackets_content", true);
        for region in e.regions() {
            let pair = analysis.pair_at(region.b, content, |kind| match kind {
                kind if kind != 't' && !e.settings.bool("match_brackets", true) => false,
                '[' => e.settings.bool("match_brackets_square", true),
                '{' => e.settings.bool("match_brackets_braces", true),
                '<' => e.settings.bool("match_brackets_angle", false),
                't' => e.settings.bool("match_tags", true),
                _ => true,
            });
            if let Some(pair) = pair {
                let key = if pair.kind == 't' {
                    "tags"
                } else if pair.open.end < region.b && region.b < pair.close.start {
                    "bracket_contents"
                } else {
                    "brackets"
                };
                let fg = color(scheme, &format!("{key}_foreground"));
                let options = match key {
                    "tags" => scheme.tags_options(),
                    "bracket_contents" => scheme.bracket_contents_options(),
                    _ => scheme.brackets_options(),
                }
                .unwrap_or(DecorationOptions::UNDERLINE);
                let Some(options) = decoration_underline(options) else {
                    continue;
                };
                for range in [&pair.open, &pair.close] {
                    let a = self.rect_at(e, range.start);
                    let b = self.rect_at(e, range.end);
                    if a.y >= self.tab_height - self.line_height && a.y <= self.size.height {
                        underline(
                            layer,
                            Rect {
                                x: a.x,
                                y: a.y + self.line_height - 3.,
                                width: (b.x - a.x).max(self.cell),
                                height: 3.,
                            },
                            fg,
                            options,
                            clip,
                        );
                    }
                }
            }
        }
    }
    fn minimap_geometry(&self, e: &Editor) -> MinimapGeometry {
        MinimapGeometry::new(
            Rect {
                x: self.size.width - self.minimap_width(e),
                y: self.tab_height,
                width: self.minimap_width(e),
                height: (self.size.height - self.tab_height).max(0.),
            },
            self.total(e),
            self.line_height,
            self.scroll_y,
            self.max_scroll_y(e),
        )
    }
    fn paint_minimap(
        &self,
        e: &Editor,
        language: Option<&Language>,
        scheme: &ColorScheme,
        layer: &mut Layer,
    ) {
        let width = self.minimap_width(e);
        if width == 0. {
            return;
        }
        let map = self.minimap_geometry(e);
        let bounds = map.bounds;
        fill(
            layer,
            bounds,
            bounds,
            rgba(scheme.default_style().background),
        );
        {
            let mut theme = e.theme.borrow_mut();
            let path = [
                vel_theme::Element::new("window", &[]),
                vel_theme::Element::new(
                    "minimap_control",
                    if self.minimap_hover { &["hover"] } else { &[] },
                ),
            ];
            let style = theme.style(e.ui_id(0xE200), &vel_theme::ElementPath(&path), &e.settings);
            theme.surface(layer, &style, bounds, bounds);
        }
        let layout = e.layout.borrow();
        // Build 4142: https://www.sublimetext.com/download. Horizontal panning is opt-in.
        let horizontal = if e.settings.bool("minimap_horizontal_scrolling", false) {
            self.scroll_x / self.cell * 0.65
        } else {
            0.
        };
        // Bound all work by visible minimap rows, including wrapped/folded projections.
        let fallback = (e.features.highlighting == vel_largefile::HighlightMode::Viewport
            || !e.features.indexing)
            && language.is_none_or(|l| l.minimap.is_empty());
        let samples = language.is_some_and(|l| !l.minimap.is_empty());
        if fallback || samples {
            let stride = if fallback {
                usize::try_from(e.features.minimap_stride)
                    .unwrap_or(usize::MAX)
                    .max(1)
            } else {
                1
            };
            for visual in map.visible_rows().step_by(stride) {
                if layout.inline_at(visual).is_some() {
                    continue;
                }
                let (row, subrow) = layout.locate(visual);
                let y = map.y(visual as f64);
                let styled = language
                    .and_then(|l| l.minimap.get(l.minimap.partition_point(|l| l.row < row)))
                    .filter(|l| l.row == row);
                if styled.is_none() && !fallback {
                    continue;
                }
                let segment = layout.segment(&e.text, row, subrow);
                let segment_column = segment.source_column;
                let segment_columns = segment.columns;
                if let Some(line) = styled {
                    for token in &line.tokens {
                        let from = token.column.max(segment_column);
                        let to = (token.column + token.width)
                            .min(segment_column.saturating_add(segment_columns));
                        if to <= from {
                            continue;
                        }
                        let x = bounds.x + (segment.indent + from - segment_column) as f64 * 0.65
                            - horizontal;
                        if x >= bounds.x + width {
                            break;
                        }
                        if x + (to - from) as f64 * 0.65 < bounds.x {
                            continue;
                        }
                        fill(
                            layer,
                            Rect {
                                x,
                                y,
                                width: (to - from) as f64 * 0.65,
                                height: map.pitch.min(1.5),
                            },
                            bounds,
                            token.color,
                        );
                    }
                } else {
                    let start = e.text.line_to_byte(row) + segment.bytes.start;
                    let end = e.text.floor_char_boundary(
                        (start + 160).min(e.text.line_to_byte(row) + segment.bytes.end),
                    );
                    let sample = e.text.slice(start..end).to_string();
                    let cells = crate::editor::cells_with_tab(
                        sample.trim_end_matches('\n'),
                        e.preferences.tab_size,
                    );
                    fill(
                        layer,
                        Rect {
                            x: bounds.x + segment.indent as f64 * 0.65 - horizontal,
                            y,
                            width: (cells as f64 * 0.65).min(width + horizontal),
                            height: map.pitch.min(1.5),
                        },
                        bounds,
                        rgba(scheme.default_style().foreground),
                    );
                }
            }
        }
        // Documented default: https://docs.sublimetext.io/reference/settings.html#visual-settings
        if e.settings.bool("always_show_minimap_viewport", false)
            || self.minimap_hover
            || self.minimap_drag
        {
            let rect = map.viewport(self.scroll_y, self.line_height);
            let mut theme = e.theme.borrow_mut();
            let path = [
                vel_theme::Element::new("window", &[]),
                vel_theme::Element::new(
                    "minimap_control",
                    if self.minimap_hover { &["hover"] } else { &[] },
                ),
            ];
            let style = theme.style(e.ui_id(0xE200), &vel_theme::ElementPath(&path), &e.settings);
            let mut shade = style
                .viewport_color
                .unwrap_or_else(|| color(scheme, "minimap_border"));
            shade[3] *= style.viewport_opacity;
            fill(layer, rect, bounds, shade);
        }
        // This setting borders the visible-text region independently of hover:
        // https://docs.sublimetext.io/reference/settings.html#visual-settings
        if e.settings.bool("draw_minimap_border", false) {
            border(
                layer,
                map.viewport(self.scroll_y, self.line_height),
                bounds,
                color(scheme, "minimap_border"),
            );
        }
    }
}
fn fallback_scheme(appearance: Appearance) -> &'static ColorScheme {
    static DARK: OnceLock<ColorScheme> = OnceLock::new();
    static LIGHT: OnceLock<ColorScheme> = OnceLock::new();
    if appearance == Appearance::Light {
        LIGHT.get_or_init(ColorScheme::velocity_light)
    } else {
        DARK.get_or_init(ColorScheme::velocity_dark)
    }
}
pub fn color(scheme: &ColorScheme, name: &str) -> Color {
    rgba(
        scheme
            .globals
            .color(name)
            .unwrap_or_else(|| scheme.default_style().foreground),
    )
}
pub fn fill(layer: &mut Layer, rect: Rect, clip: Rect, color: Color) {
    layer.rects.push(SolidRect {
        rect,
        radius: 0.,
        color,
        clip,
    });
}
fn outline(layer: &mut Layer, rect: Rect, clip: Rect, color: Color) {
    layer.rects.push(SolidRect::outline(rect, clip, color));
}
fn border(layer: &mut Layer, rect: Rect, clip: Rect, color: Color) {
    for r in [
        Rect { height: 1., ..rect },
        Rect {
            y: rect.y + rect.height - 1.,
            height: 1.,
            ..rect
        },
        Rect { width: 1., ..rect },
        Rect {
            x: rect.x + rect.width - 1.,
            width: 1.,
            ..rect
        },
    ] {
        fill(layer, r, clip, color);
    }
}
#[allow(clippy::too_many_arguments)]
fn text_run(
    layer: &mut Layer,
    text: &str,
    x: f64,
    baseline: f64,
    size: f64,
    color: Color,
    style: FontStyle,
    clip: Rect,
) {
    layer.text.push(TextRun {
        family: None,
        text: text.to_owned(),
        x: x as f32,
        baseline: baseline as f32,
        size: size as f32,
        color,
        style,
        clip,
    });
}
fn underline(layer: &mut Layer, rect: Rect, color: Color, options: &str, clip: Rect) {
    if options.contains("none") {
        return;
    }
    if options.contains("outline") {
        border(layer, rect, clip, color);
    } else {
        layer.underlines.push(Underline {
            rect,
            color,
            thickness: 1.,
            squiggle: options.contains("squiggly"),
            stippled: options.contains("stippled"),
            clip,
        });
    }
}
fn whitespace(e: &Editor, line: &str, byte: usize, selected: bool) -> bool {
    let leading = || line[..byte].trim().is_empty();
    let trailing = || line[byte..].trim().is_empty();
    match e.settings.get("draw_white_space") {
        Some(vel_config::Value::Array(options)) => options.iter().any(|s| {
            s == "all"
                || (s == "selection" && selected)
                || (s == "leading" && leading())
                || (s == "trailing" && trailing())
        }),
        _ => match e.settings.string("draw_white_space", "none") {
            "all" => true,
            "selection" => selected,
            "leading" => leading(),
            "trailing" => trailing(),
            _ => false,
        },
    }
}
pub fn caret_alpha(style: &str, seconds: f64) -> f32 {
    let phase = seconds.rem_euclid(1.);
    match style {
        "blink" => f32::from(phase < 0.5),
        "phase" => (1. - phase * 2.).clamp(0., 1.) as f32,
        "smooth" => ((seconds * std::f64::consts::TAU).cos() * 0.5 + 0.5) as f32,
        _ => 1.,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vel_text::Text;
    #[test]
    fn horizontal_scroll_stops_at_the_widest_line() {
        let size = Size {
            width: 400.,
            height: 200.,
        };
        // Every line fits: a horizontal swipe must not move the text.
        let mut e = Editor::new(Text::from("short\nlines\n"));
        e.settings.set(
            vel_config::settings::Layer::View,
            "word_wrap",
            vel_config::json!(false),
        );
        e.sync();
        let mut v = View::new(size, 8., 12.);
        v.scene(&e);
        v.scroll(&e, 300., 0.);
        assert_eq!(v.scroll_x, 0.);
        // A 200-column line bounds the offset so its end reaches the right edge.
        let long = format!("{}\nshort\n", "x".repeat(200));
        let mut e = Editor::new(Text::from(long.as_str()));
        e.settings.set(
            vel_config::settings::Layer::View,
            "word_wrap",
            vel_config::json!(false),
        );
        e.sync();
        let mut v = View::new(size, 8., 12.);
        v.scene(&e);
        v.scroll(&e, 10_000., 0.);
        let visible = v.right(&e) - v.gutter(&e) - v.margin(&e) * 2.;
        assert!(
            (v.scroll_x - (201. * 8. - visible)).abs() < 0.01,
            "{}",
            v.scroll_x
        );
        v.scroll(&e, -20_000., 0.);
        assert_eq!(v.scroll_x, 0.);
    }
    #[test]
    fn utf16_geometry_hit_testing_scroll_and_retina_independence() {
        let mut e = Editor::new(Text::from("a🦀你\nnext"));
        let mut v = View::new(
            Size {
                width: 400.,
                height: 200.,
            },
            8.,
            12.,
        );
        v.apply_style(&e);
        let (rect, actual) = v.range_rect(&e, Utf16Range::new(1, 2));
        assert_eq!(actual, Utf16Range::new(1, 2));
        assert_eq!(rect.width, 16.);
        assert_eq!(
            v.hit_test(
                &e,
                Point {
                    x: rect.x + 16.,
                    y: rect.y
                }
            ),
            5
        );
        v.scroll_x = 0.25;
        assert_eq!(v.rect_at(&e, 1).x, rect.x - 0.25);
        e.text_input(vel_platform::TextInput::Marked {
            text: "你好".into(),
            selected: Utf16Range::new(2, 0),
            replacement: Some(Utf16Range::new(1, 2)),
        });
        assert_eq!(v.scene(&e).layers[0].underlines.len(), 1);
    }
    #[test]
    fn selection_across_newlines_and_visibility_are_bounded() {
        let mut e = Editor::new(Text::from("a\n你🦀\nthird"));
        e.set_selection(vel_buffer::Selection::from_regions([
            vel_buffer::Region::new(0, e.text.len_bytes()),
        ]));
        let mut v = View::new(
            Size {
                width: 100.,
                height: 40.,
            },
            8.,
            12.,
        );
        let scene = v.scene(&e);
        assert!(
            scene.layers[0]
                .contours
                .iter()
                .any(|c| !c.triangles.is_empty() && c.fill[3] > 0.)
        );
        v.reveal(&e);
        let caret = v.rect_at(&e, e.caret).clamped(v.size);
        assert!(caret.y >= 0. && caret.y + caret.height <= v.size.height);
    }
    #[test]
    fn requested_location_centers_after_stale_scroll_resize_and_wrapping() {
        let mut e = Editor::new(Text::from(
            "a long source line with words\n".repeat(1000).as_str(),
        ));
        let mut v = View::new(
            Size {
                width: 800.,
                height: 600.,
            },
            8.,
            12.,
        );
        e.set_caret(e.text.line_to_byte(119) + 9, false);
        for (height, stale) in [(600., 7000.), (360., 0.), (480., 12000.)] {
            v.size.height = height;
            v.scroll_y = stale;
            v.center_caret(&e);
            let r = v.rect_at(&e, e.caret);
            let expected = (v.tab_height + v.size.height) / 2.;
            assert!((r.y + r.height / 2. - expected).abs() < 0.01);
        }
        e.settings.set(
            vel_config::settings::Layer::View,
            "word_wrap",
            vel_config::json!(true),
        );
        v.size.width = 130.;
        v.center_caret(&e);
        let r = v.rect_at(&e, e.caret);
        assert!((r.y + r.height / 2. - (v.tab_height + v.size.height) / 2.).abs() < 0.01);
        e.set_caret(0, false);
        v.center_caret(&e);
        assert_eq!(v.scroll_y, 0.);
        e.set_caret(e.text.len_bytes(), false);
        v.center_caret(&e);
        let r = v.rect_at(&e, e.caret);
        assert!(r.y >= v.tab_height && r.y + r.height <= v.size.height);
    }
    #[test]
    fn caret_modes_and_whitespace_selection_have_distinct_output() {
        assert_eq!(caret_alpha("solid", 0.75), 1.);
        assert_eq!(caret_alpha("blink", 0.75), 0.);
        assert!(caret_alpha("smooth", 0.5) < 0.01);
        let mut e = Editor::new(Text::from("  x \n"));
        e.settings.set(
            vel_config::settings::Layer::View,
            "draw_white_space",
            vel_config::json!(["leading", "trailing"]),
        );
        assert!(whitespace(&e, "  x \n", 0, false));
        assert!(whitespace(&e, "  x \n", 3, false));
        assert!(!whitespace(&e, "x x", 1, false));
    }
}

// Public settings reference confirms match_selection defaults to true:
// https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/reference/settings.html#visual-settings
fn selected_word(e: &Editor, regions: &[vel_buffer::Region]) -> Option<String> {
    if !e.settings.bool("match_selection", true) || e.marked.is_some() {
        return None;
    }
    let r = regions.first()?;
    if r.empty() || r.size() > crate::large::LONG_LINE {
        return None;
    }
    let word = e.text.slice(r.to_range()).to_string();
    if !word.chars().all(word_character)
        || !word.chars().any(char::is_alphanumeric)
        || (r.begin() > 0
            && word_character(e.text.char_at(e.text.floor_char_boundary(r.begin() - 1))))
        || (r.end() < e.text.len_bytes() && word_character(e.text.char_at(r.end())))
        || regions
            .iter()
            .any(|r| r.empty() || e.text.slice(r.to_range()).to_string() != word)
    {
        return None;
    }
    Some(word)
}
fn word_character(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || unicode_width::UnicodeWidthChar::width(c) == Some(0)
}
fn word_occurrences<'a>(line: &'a str, word: &'a str) -> impl Iterator<Item = Range<usize>> + 'a {
    line.match_indices(word).filter_map(move |(at, _)| {
        let end = at + word.len();
        let before = line[..at].chars().next_back();
        let after = line[end..].chars().next();
        (!before.is_some_and(word_character) && !after.is_some_and(word_character))
            .then_some(at..end)
    })
}
fn paint_guide(layer: &mut Layer, rect: Rect, clip: Rect, color: Color, solid: bool) {
    if solid {
        fill(layer, rect, clip, color);
    } else {
        layer
            .rects
            .push(SolidRect::stippled_guide(rect, clip, color));
    }
}
fn visible_bookmarks(e: &Editor, visible: Range<usize>) -> &[vel_buffer::Region] {
    let regions = e.bookmarks();
    // Toggle Bookmark retains disjoint regions; monotone edit mapping preserves
    // their order, including collapsed carets. No per-frame region/row allocation.
    let first = regions
        .partition_point(|r| r.end() < visible.start || (r.end() == visible.start && !r.empty()));
    let last = regions.partition_point(|r| r.begin() <= visible.end);
    &regions[first.min(last)..last]
}
fn bookmarked_line(regions: &[vel_buffer::Region], line: Range<usize>) -> bool {
    let at =
        regions.partition_point(|r| r.end() < line.start || (r.end() == line.start && !r.empty()));
    regions.get(at).is_some_and(|r| r.begin() < line.end)
}

#[cfg(test)]
#[path = "round16_perf.rs"]
mod round16_perf;
#[cfg(test)]
#[path = "round16_tests.rs"]
mod round16_tests;

#[cfg(test)]
#[path = "round16_review_tests.rs"]
mod round16_review_tests;

#[cfg(test)]
#[path = "round16_review_perf.rs"]
mod round16_review_perf;

/// Shared by centred margins, frame layout and motion between frames.
fn effective_wrap_columns(configured: usize, viewport: usize, fixed: bool) -> usize {
    if configured == 0 {
        viewport.max(1)
    } else if fixed {
        configured
    } else {
        configured.min(viewport.max(1))
    }
}
#[derive(Clone, Copy, Debug)]
enum MinimapDrag {
    Viewport { grab: f64 },
    Click { press_y: f64, scroll: f64 },
}
#[derive(Clone, Copy, Debug)]
struct MinimapGeometry {
    bounds: Rect,
    pitch: f64,
    offset: f64,
    total: usize,
    travel: f64,
    max_scroll: f64,
}
impl MinimapGeometry {
    fn new(bounds: Rect, total: usize, line_height: f64, scroll: f64, max_scroll: f64) -> Self {
        let pitch = (line_height / 8.).max(1.);
        let viewport_lines = bounds.height / line_height.max(1.);
        let box_height = (viewport_lines * pitch).min(bounds.height);
        let travel = (bounds.height - box_height)
            .min(max_scroll / line_height.max(1.) * pitch)
            .max(0.);
        let fraction = if max_scroll > 0. {
            (scroll / max_scroll).clamp(0., 1.)
        } else {
            0.
        };
        // Short documents retain their fixed pitch at the top. Long documents pan
        // so that the viewport travels over the whole control, including past-end space.
        let offset = (scroll / line_height.max(1.) * pitch - fraction * travel).max(0.);
        Self {
            bounds,
            pitch,
            offset,
            total,
            travel,
            max_scroll,
        }
    }
    fn scroll_at(self, viewport_y: f64) -> f64 {
        if self.travel > 0. {
            ((viewport_y - self.bounds.y) / self.travel).clamp(0., 1.) * self.max_scroll
        } else {
            0.
        }
    }
    fn drag_scroll(self, gesture: MinimapDrag, y: f64) -> f64 {
        match gesture {
            MinimapDrag::Viewport { grab } => self.scroll_at(y - grab),
            MinimapDrag::Click { press_y, scroll } => {
                let anchor = scroll.clamp(0., self.max_scroll);
                if y == press_y {
                    anchor
                } else if y < press_y {
                    let travel = (press_y - self.bounds.y).max(0.);
                    if travel > 0. {
                        anchor * (1. - ((press_y - y) / travel).clamp(0., 1.))
                    } else {
                        0.
                    }
                } else {
                    let travel = (self.bounds.y + self.bounds.height - press_y).max(0.);
                    if travel > 0. {
                        anchor + (self.max_scroll - anchor) * ((y - press_y) / travel).clamp(0., 1.)
                    } else {
                        self.max_scroll
                    }
                }
            }
        }
    }
    fn y(self, visual: f64) -> f64 {
        self.bounds.y + visual * self.pitch - self.offset
    }
    fn visual_at(self, y: f64) -> f64 {
        ((y - self.bounds.y + self.offset) / self.pitch)
            .clamp(0., self.total.saturating_sub(1) as f64)
    }
    fn visible_rows(self) -> Range<usize> {
        let first = (self.offset / self.pitch).floor() as usize;
        first.min(self.total)
            ..((self.offset + self.bounds.height) / self.pitch)
                .ceil()
                .min(self.total as f64) as usize
    }
    fn viewport(self, scroll: f64, line_height: f64) -> Rect {
        Rect {
            y: self.y(scroll / line_height.max(1.)),
            height: (self.bounds.height / line_height.max(1.) * self.pitch).min(self.bounds.height),
            ..self.bounds
        }
    }
}

fn decoration_underline(options: DecorationOptions) -> Option<&'static str> {
    if options.contains(DecorationOptions::SQUIGGLY_UNDERLINE) {
        Some("squiggly_underline")
    } else if options.contains(DecorationOptions::STIPPLED_UNDERLINE) {
        Some("stippled_underline")
    } else if options.contains(DecorationOptions::UNDERLINE) {
        Some("underline")
    } else {
        None
    }
}

fn corner_style(style: vel_color_scheme::CornerStyle) -> vel_render::selection::Corners {
    match style {
        vel_color_scheme::CornerStyle::Round => vel_render::selection::Corners::Round,
        vel_color_scheme::CornerStyle::Cut => vel_render::selection::Corners::Cut,
        vel_color_scheme::CornerStyle::Square => vel_render::selection::Corners::Square,
    }
}

#[cfg(test)]
mod lane_b_round17_tests {
    use super::*;
    use vel_config::{json, settings::Layer as SettingLayer};
    use vel_text::Text;
    #[test]
    fn minimap_viewport_default_and_borders_are_independent() {
        let mut editor = Editor::new(Text::from("line\n".repeat(50).as_str()));
        let mut view = View::new(
            Size {
                width: 400.,
                height: 300.,
            },
            8.,
            12.,
        );
        editor
            .settings
            .replace(SettingLayer::View, json!({"show_minimap":true}))
            .unwrap();
        editor.sync();
        view.apply_style(&editor);
        let map = view.minimap_geometry(&editor);
        for (hover, border_enabled) in [(false, false), (true, false), (false, true), (true, true)]
        {
            view.minimap_hover = hover;
            editor.settings.set(
                SettingLayer::View,
                "draw_minimap_border",
                json!(border_enabled),
            );
            editor.sync();
            let mut layer = Layer::default();
            view.paint_minimap(&editor, None, &ColorScheme::velocity_dark(), &mut layer);
            let viewport = map.viewport(view.scroll_y, view.line_height);
            let border = layer.rects.iter().any(|r| {
                r.rect
                    == Rect {
                        height: 1.,
                        ..viewport
                    }
            });
            assert_eq!(border, border_enabled);
            assert_eq!(layer.rects.iter().any(|r| r.rect == viewport), hover);
            if !hover && !border_enabled {
                assert_eq!(layer.rects.len(), 1);
            }
        }
    }
    #[test]
    fn minimap_mapping_table_short_long_wrapped_and_scrolled() {
        let bounds = Rect {
            x: 300.,
            y: 30.,
            width: 100.,
            height: 400.,
        };
        for (total, scroll, first, last) in [
            (10, 0., 0, 10),
            (10_000, 0., 0, 160),
            (10_000, 98_000., 4831, 4992),
            (10_000, 199_600., 9840, 10_000),
        ] {
            let map = MinimapGeometry::new(
                bounds,
                total,
                20.,
                scroll,
                (total as f64 * 20. - bounds.height).max(0.),
            );
            let visible = map.visible_rows();
            assert_eq!(visible.start, first, "{total} {scroll}");
            assert_eq!(visible.end, last, "{total} {scroll}");
            for row in visible {
                assert!((map.visual_at(map.y(row as f64)) - row as f64).abs() < 0.00001);
            }
            assert_eq!(map.pitch, 2.5);
        }
        let mut editor = Editor::new(Text::from("a long line wraps into several rows\nshort"));
        editor
            .settings
            .set(SettingLayer::View, "word_wrap", json!(true));
        editor
            .settings
            .set(SettingLayer::View, "wrap_width", json!(5));
        editor.sync();
        let mut view = View::new(
            Size {
                width: 400.,
                height: 120.,
            },
            8.,
            12.,
        );
        view.apply_style(&editor);
        let total = editor.layout.borrow().total();
        assert!(total > editor.text.len_lines());
        let map = view.minimap_geometry(&editor);
        assert_eq!(map.total, total);
        let row = 4.min(total - 1);
        assert!((map.visual_at(map.y(row as f64)) - row as f64).abs() < 0.00001);
        view.pointer(
            &mut editor,
            Point {
                x: map.bounds.x + 4.,
                y: map.y(row as f64),
            },
            true,
            false,
            false,
        )
        .unwrap();
        // Pressing inside the viewport grabs it without jumping; an outside click
        // centres the row drawn beneath it (the long-file table exercises that case).
        let point = Point {
            x: map.bounds.x + 4.,
            y: map.y(row as f64),
        };
        let viewport = map.viewport(0., view.line_height);
        let expected = if vel_ui::contains(viewport, point) {
            0.
        } else {
            (map.visual_at(point.y) * view.line_height
                - (view.size.height - view.tab_height - 8.).max(0.) / 2.)
                .clamp(0., view.max_scroll_y(&editor))
        };
        assert!((view.scroll_y - expected).abs() < 0.001);
    }
    #[test]
    fn zero_wrap_width_and_centered_geometry_share_columns_with_hit_testing() {
        let mut editor = Editor::new(Text::from("a ".repeat(150).as_str()));
        let mut view = View::new(
            Size {
                width: 900.,
                height: 500.,
            },
            8.,
            12.,
        );
        for fixed in [false, true] {
            for configured in [0, 1, 40, 200] {
                editor.settings.replace(SettingLayer::View,json!({"word_wrap":true,"wrap_width":configured,"wrap_width_style":if fixed {"fixed"} else {"auto"},"draw_centered":true})).unwrap();
                editor.sync();
                view.apply_style(&editor);
                if configured == 0 {
                    assert_eq!(view.margin(&editor), 8.);
                }
                let options = motion_layout_options(&editor);
                assert_eq!(
                    options.width,
                    effective_wrap_columns(
                        configured,
                        editor.layout.borrow().viewport_width,
                        fixed
                    )
                );
                for at in [0, 2, 20, 80, editor.text.len_bytes()] {
                    let rect = view.rect_at(&editor, at);
                    assert_eq!(
                        view.hit_test(
                            &editor,
                            Point {
                                x: rect.x,
                                y: rect.y + rect.height / 2.
                            }
                        ),
                        at
                    );
                }
            }
        }
    }
    #[test]
    fn font_metrics_drive_baseline_carets_gutter_scroll_and_minimap() {
        let mut editor = Editor::new(Text::from("alpha\n".repeat(200).as_str()));
        editor
            .settings
            .replace(
                SettingLayer::View,
                json!({"line_padding_top":2.,"line_padding_bottom":3.,"show_minimap":true}),
            )
            .unwrap();
        editor.sync();
        let mut view = View::new(
            Size {
                width: 400.,
                height: 220.,
            },
            8.,
            12.,
        );
        let metrics = vel_render::typography::FontMetrics {
            ascent: 12.1,
            descent: 3.2,
            leading: 0.6,
        };
        view.set_font_metrics(metrics, 2.);
        let scene = view.scene(&editor);
        assert_eq!(view.line_height, 21.);
        let rect = view.rect_at(&editor, 6);
        assert_eq!(rect.height, 21.);
        assert_eq!(
            view.hit_test(
                &editor,
                Point {
                    x: rect.x,
                    y: rect.y + 10.
                }
            ),
            6
        );
        let run = scene.layers[0]
            .text
            .iter()
            .find(|r| r.text == "alpha")
            .unwrap();
        assert!((run.baseline as f64 - view.tab_height - 4. - 14.1).abs() < 0.00001);
        let number = scene.layers[0].text.iter().find(|r| r.text == "1").unwrap();
        assert_eq!(number.baseline, run.baseline);
        assert_eq!(view.minimap_geometry(&editor).pitch, 21. / 8.);
        view.scroll_y = 1e9;
        view.clamp_scroll(&editor);
        assert_eq!(
            view.scroll_y,
            (editor.layout.borrow().total() as f64 * 21. - view.size.height + view.tab_height + 8.)
                .max(0.)
        );
    }
    #[test]
    fn randomized_wrap_fold_wide_and_tiny_geometry_stays_in_bounds() {
        let mut seed = 0x12543_u64;
        for iteration in 0..300 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let text = [
                "a你🦀 é\t/end-one.\n",
                "\n",
                "  中文한글 !!= ->\n",
                "🙂wide\n",
            ]
            .repeat(1 + (seed as usize % 12))
            .concat();
            let mut editor = Editor::new(Text::from(text.as_str()));
            let boundaries: Vec<_> = text
                .char_indices()
                .map(|(i, _)| i)
                .chain([text.len()])
                .collect();
            editor.settings.replace(SettingLayer::View,json!({"word_wrap":true,"wrap_width":seed as usize%20,"draw_centered":iteration%2==0,"gutter":iteration%3!=0})).unwrap();
            editor.sync();
            if boundaries.len() > 5 {
                let a = boundaries[2];
                let b = boundaries[boundaries.len() / 2];
                editor.set_folded(vec![vel_buffer::Region::new(a, b)]);
            }
            let mut view = View::new(
                Size {
                    width: (seed % 200) as f64,
                    height: (seed % 160) as f64,
                },
                8.,
                12.,
            );
            view.scene(&editor);
            view.scroll_y = 1e6;
            view.clamp_scroll(&editor);
            for at in boundaries.iter().step_by(3) {
                let rect = view.rect_at(&editor, *at);
                assert!(rect.x.is_finite() && rect.y.is_finite());
                let hit = view.hit_test(
                    &editor,
                    Point {
                        x: rect.x,
                        y: rect.y + rect.height / 2.,
                    },
                );
                assert!(hit <= editor.text.len_bytes() && text.is_char_boundary(hit));
                let map = view.minimap_geometry(&editor);
                assert!(map.visible_rows().end <= editor.layout.borrow().total());
            }
        }
    }
    #[test]
    fn typed_bracket_options_never_turn_foreground_or_unknown_into_underlines() {
        for (value, expected) in [
            ("foreground", None),
            ("bold italic foreground", None),
            ("unknown", None),
            ("", None),
            ("underline", Some("underline")),
            ("foreground stippled_underline", Some("stippled_underline")),
            ("squiggly_underline bold", Some("squiggly_underline")),
        ] {
            let scheme = vel_color_scheme::SchemeSource::parse(
                &json!({"globals":{"brackets_options":value}}).to_string(),
            )
            .unwrap()
            .compile()
            .unwrap();
            assert_eq!(
                decoration_underline(scheme.brackets_options().unwrap()),
                expected
            );
        }
    }
    #[test]
    fn fractional_scroll_distance_and_paging_use_the_real_viewport() {
        let mut editor = Editor::new(Text::from("line\n".repeat(500).as_str()));
        let mut view = View::new(
            Size {
                width: 400.,
                height: 240.,
            },
            8.,
            12.,
        );
        for (value, fraction) in [(json!(false), 0.), (json!(true), 1.), (json!(0.5), 0.5)] {
            editor
                .settings
                .set(SettingLayer::View, "scroll_past_end", value);
            editor.sync();
            view.scroll_y = 1e9;
            view.clamp_scroll(&editor);
            let viewport = view.size.height - view.tab_height - 8.;
            assert!(
                (view.scroll_y
                    - (view.total(&editor) as f64 * view.line_height - viewport
                        + fraction * (viewport - view.line_height).max(0.)))
                .abs()
                    < 0.001
            );
        }
        editor
            .settings
            .set(SettingLayer::View, "overlay_scroll_bars", json!("disabled"));
        editor.sync();
        for jump in [false, true] {
            for option in [false, true] {
                view.mouse_preferences.jump_to_click = jump;
                view.scroll_y = 0.;
                view.scrolled = Some(Instant::now());
                view.scene(&editor);
                let point = Point {
                    x: view.scroll_bar.x + 2.,
                    y: view.scroll_bar.y + view.scroll_bar.height - 2.,
                };
                view.pointer_with_option(&mut editor, point, true, false, false, option)
                    .unwrap();
                if jump ^ option {
                    assert!(view.scroll_y > 1000.);
                } else {
                    assert_eq!(view.scroll_y, view.scroll_bar.height);
                }
                view.pointer(&mut editor, point, false, false, true)
                    .unwrap();
            }
        }
    }
}

#[cfg(test)]
mod round17_scene_timing {
    use super::*;
    #[test]
    #[ignore = "explicit glyph-run glow staging timing; GPU pass requires native review"]
    fn round17_glow_staging_cost() {
        let editor = Editor::new(vel_text::Text::new());
        let view = View::new(
            Size {
                width: 1200.,
                height: 800.,
            },
            8.,
            12.,
        );
        let clip = Rect {
            x: 0.,
            y: 0.,
            width: 1200.,
            height: 800.,
        };
        for glow in [false, true] {
            let decoration = if glow {
                Decoration::GLOW
            } else {
                Decoration::default()
            };
            let start = Instant::now();
            for _ in 0..20_000 {
                let mut layer = Layer::default();
                let mut run = "keyword -> selected glyphs".to_owned();
                view.flush(
                    &mut layer,
                    &mut run,
                    100.,
                    100.,
                    [1.; 4],
                    FontStyle::default(),
                    decoration,
                    clip,
                    &editor,
                );
                assert_eq!(layer.glow.len(), usize::from(glow));
                std::hint::black_box(layer);
            }
            println!(
                "GLOW_STAGING glow={glow} us_per_run={}",
                start.elapsed().as_secs_f64() * 1e6 / 20_000.
            );
        }
    }
    #[test]
    #[ignore = "explicit visible-scene corner/glow timing, no concurrent builds"]
    fn round17_disabled_features_and_corner_scene_cost() {
        let mut editor = Editor::new(vel_text::Text::from(
            "text and symbols -> !=\n".repeat(1000).as_str(),
        ));
        let mut view = View::new(
            Size {
                width: 1200.,
                height: 800.,
            },
            8.,
            12.,
        );
        editor.settings.set(
            vel_config::settings::Layer::View,
            "show_minimap",
            vel_config::json!(false),
        );
        editor.sync();
        for selected in [false, true] {
            if selected {
                editor.set_selection(vel_buffer::Selection::from_regions([
                    vel_buffer::Region::new(0, editor.text.len_bytes()),
                ]));
            }
            let start = Instant::now();
            let mut rects = 0;
            let mut contours = 0;
            for _ in 0..2000 {
                let scene = std::hint::black_box(view.scene(&editor));
                rects = scene.layers[0].rects.len();
                contours = scene.layers[0].contours.len();
            }
            println!(
                "SCENE_TIMING selected={selected} us={} rects={rects} contours={contours}",
                start.elapsed().as_secs_f64() * 1e6 / 2000.
            );
        }
    }
}

#[cfg(test)]
mod minimap_drag_paint_regression {
    use super::*;
    #[test]
    fn minimap_drag_paint_and_hit_testing_share_the_live_inverse() {
        let mut editor = Editor::new(vel_text::Text::from("line\n".repeat(10_000).as_str()));
        editor
            .settings
            .replace(
                vel_config::settings::Layer::View,
                vel_config::json!({
                    "word_wrap":false,"show_minimap":true,"always_show_minimap_viewport":true
                }),
            )
            .unwrap();
        editor.sync();
        let mut view = View::new(
            Size {
                width: 800.,
                height: 600.,
            },
            8.,
            12.,
        );
        view.apply_style(&editor);
        let map = view.minimap_geometry(&editor);
        let before = map.viewport(view.scroll_y, view.line_height);
        let grab = before.height / 2.;
        let press = Point {
            x: map.bounds.x + 20.,
            y: before.y + grab,
        };
        view.pointer(&mut editor, press, true, false, false)
            .unwrap();
        for y in [press.y + 20., map.bounds.y + map.bounds.height - grab] {
            let point = Point { y, ..press };
            view.pointer(&mut editor, point, false, true, false)
                .unwrap();
            let drawing = view.minimap_geometry(&editor);
            let viewport = drawing.viewport(view.scroll_y, view.line_height);
            assert!((viewport.y + grab - y).abs() < 1e-6);
            assert!((drawing.scroll_at(viewport.y) - view.scroll_y).abs() < 1e-6);
            let mut layer = Layer::default();
            view.paint_minimap(&editor, None, &ColorScheme::velocity_dark(), &mut layer);
            assert!(layer.rects.iter().any(|r| r.rect == viewport));
            view.pointer(&mut editor, point, false, false, true)
                .unwrap();
            assert_eq!(
                view.minimap_geometry(&editor)
                    .viewport(view.scroll_y, view.line_height),
                viewport
            );
            view.pointer(&mut editor, point, true, false, false)
                .unwrap();
        }
        assert!((view.scroll_y - view.max_scroll_y(&editor)).abs() < 1e-6);
        view.end_pointer_gesture();
    }
}
