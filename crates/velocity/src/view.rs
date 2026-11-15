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
use vel_color_scheme::{ColorScheme, FontStyle as Decoration};
use vel_platform::{Appearance, Point, Rect, Size, Utf16Range};
use vel_render::scene::*;
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

#[derive(Debug)]
pub struct View {
    units: HashMap<u64, CachedLine>,
    pub find_matches: Vec<Range<usize>>,
    pub size: Size,
    pub cell: f64,
    pub ascent: f64,
    pub line_height: f64,
    pub scroll_x: f64,
    pub scroll_y: f64,
    pub appearance: Appearance,
    pub focused: bool,
    pub minimap_hover: bool,
    pub minimap_drag: bool,
    pub gutter_hover: bool,
    pub tab_height: f64,
    base_cell: f64,
    base_ascent: f64,
    started: Instant,
}
impl View {
    pub fn reset_caret(&mut self) {
        self.started = Instant::now();
    }
    pub fn new(size: Size, cell: f64, ascent: f64) -> Self {
        Self {
            units: HashMap::new(),
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
            gutter_hover: false,
            tab_height: TAB_HEIGHT,
            base_cell: cell,
            base_ascent: ascent,
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
        if e.settings.bool("show_minimap", true) {
            100f64.min(self.size.width / 4.)
        } else {
            0.
        }
    }
    pub fn right(&self, e: &Editor) -> f64 {
        self.size.width - self.minimap_width(e)
    }
    fn margin(&self, e: &Editor) -> f64 {
        let margin = e.settings.number("margin", 8.).max(0.);
        if e.settings.bool("draw_centered", false) {
            let columns = e.settings.usize("wrap_width", 80).max(1);
            margin.max((self.right(e) - self.gutter(e) - columns as f64 * self.cell) / 2.)
        } else {
            margin
        }
    }
    pub fn gutter(&self, e: &Editor) -> f64 {
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
        let scale = e.preferences.font_size / 13.;
        self.cell = self.base_cell * scale;
        self.ascent = self.base_ascent * scale;
        self.line_height = (20. * scale
            + e.settings.number("line_padding_top", 0.).max(0.)
            + e.settings.number("line_padding_bottom", 0.).max(0.))
        .max(1.);
        let width = ((self.right(e) - self.gutter(e) - self.margin(e) * 2.).max(self.cell)
            / self.cell)
            .floor() as usize;
        let language = e.language.borrow();
        let auto = language
            .as_ref()
            .is_none_or(|l| l.root.build_string().starts_with("text"));
        let wrap = e
            .settings
            .get("word_wrap")
            .and_then(vel_config::Value::as_bool)
            .unwrap_or(auto);
        let configured = e.settings.usize("wrap_width", 0);
        let wrap_width = if configured == 0 {
            width
        } else if e.settings.string("wrap_width_style", "auto") == "fixed" {
            configured
        } else {
            configured.min(width)
        };
        e.layout.borrow_mut().configure(
            &e.text,
            (e.buffer.id(), vel_buffer::ChangeId(e.revision)),
            Options {
                width: if wrap { wrap_width.max(1) } else { 0 },
                tab: e.preferences.tab_size,
                indent: e.settings.bool("indent_subsequent_lines", true),
            },
            &e.folded(),
        );
        e.layout.borrow_mut().page = self.rows();
        if wrap && wrap_width <= width {
            self.scroll_x = 0.;
        }
    }
    pub fn clamp_scroll(&mut self, e: &Editor) {
        self.apply_style(e);
        let past = if e.preferences.scroll_past_end {
            (self.size.height - self.tab_height - self.line_height).max(0.)
        } else {
            0.
        };
        self.scroll_y = self.scroll_y.clamp(
            0.,
            (e.layout.borrow().total() as f64 * self.line_height - self.size.height
                + self.tab_height
                + 8.
                + past)
                .max(0.),
        );
        self.scroll_x = self.scroll_x.max(0.);
    }
    pub fn scroll(&mut self, e: &Editor, x: f64, y: f64) {
        self.scroll_x += x;
        self.scroll_y += y;
        self.clamp_scroll(e);
    }
    pub fn rect_at(&self, e: &Editor, byte: usize) -> Rect {
        let layout = e.layout.borrow();
        let (visual, _) = layout.visual_at(&e.text, byte);
        Rect {
            x: self.gutter(e) + self.margin(e) + layout.column(&e.text, byte) as f64 * self.cell
                - self.scroll_x,
            y: self.tab_height + 4. + visual as f64 * self.line_height - self.scroll_y,
            width: 1.,
            height: self.line_height,
        }
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
        let row = ((p.y - self.tab_height - 4. + self.scroll_y) / self.line_height)
            .floor()
            .max(0.) as usize;
        let col = ((p.x - self.gutter(e) - self.margin(e) + self.scroll_x) / self.cell).max(0.);
        e.layout.borrow().offset(&e.text, row, col)
    }
    pub fn range_rect(&self, e: &Editor, range: Utf16Range) -> (Rect, Utf16Range) {
        let bytes = range.bytes(&e.text);
        let row = e.text.byte_to_line(bytes.start);
        let (_, segment) = e.layout.borrow().visual_at(&e.text, bytes.start);
        let end = bytes.end.min(e.text.line_to_byte(row) + segment.bytes.end);
        let mut r = self.rect_at(e, bytes.start);
        let layout = e.layout.borrow();
        let line = e.text.line(row).to_string();
        let end_col = segment.indent
            + layout::units(&line, layout.hidden(row), e.preferences.tab_size)
                .iter()
                .filter(|u| {
                    u.bytes.start >= segment.bytes.start
                        && u.bytes.end <= end - e.text.line_to_byte(row)
                })
                .map(|u| u.width)
                .sum::<usize>();
        r.width =
            ((end_col as f64 - layout.column(&e.text, bytes.start) as f64) * self.cell).max(1.);
        (r, Utf16Range::from_bytes(&e.text, bytes.start..end))
    }
    pub fn viewport(&self, e: &Editor) -> Range<usize> {
        e.layout.borrow().visible_bytes(
            &e.text,
            (self.scroll_y / self.line_height).floor().max(0.) as usize,
            self.rows() + 2,
        )
    }
    pub fn exact(&self, e: &Editor) -> bool {
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
        self.minimap_hover = point.x >= self.right(e) && point.y >= self.tab_height;
        self.gutter_hover = point.x >= 0. && point.x < self.gutter(e);
        if up {
            let consumed = self.minimap_drag;
            self.minimap_drag = false;
            if consumed {
                return Ok(true);
            }
        }
        if (down && self.minimap_hover) || (dragging && self.minimap_drag) {
            self.minimap_drag = true;
            let fraction = ((point.y - self.tab_height)
                / (self.size.height - self.tab_height).max(1.))
            .clamp(0., 1.);
            let row = (fraction * e.text.len_lines() as f64) as usize;
            let byte = e.text.line_to_byte(row.min(e.text.len_lines() - 1));
            self.scroll_y = (e.layout.borrow().visual_at(&e.text, byte).0 as f64
                * self.line_height
                - self.size.height / 2.)
                .max(0.);
            self.clamp_scroll(e);
            return Ok(true);
        }
        if down
            && self.gutter_hover
            && e.settings.bool("fold_buttons", true)
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
                e.buffer.add_regions(
                    e.id,
                    layout::FOLDS,
                    existing.iter().filter(|r| *r != fold).copied().collect(),
                    true,
                );
                e.layout.borrow_mut().invalidate();
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
                folds.sort_by_key(|r| r.begin());
                e.buffer.add_regions(e.id, layout::FOLDS, folds, true);
                e.layout.borrow_mut().invalidate();
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub fn scene(&mut self, e: &Editor) -> Scene {
        self.apply_style(e);
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
                l.request_view(e.id, ranges);
            }
        }
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
        let mut layer = Layer::default();
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
        let caret_row = e.text.byte_to_line(e.caret);
        let folded = e.folded();
        let layout = e.layout.borrow();
        let caret_visuals: BTreeSet<_> = regions
            .iter()
            .map(|r| layout.visual_at(&e.text, r.b).0)
            .collect();
        for column in &e.preferences.rulers {
            fill(
                &mut layer,
                Rect {
                    x: left + *column as f64 * self.cell - self.scroll_x,
                    y: self.tab_height,
                    width: 1.,
                    height: clip.height,
                },
                clip,
                global("rulers"),
            );
        }
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
        for visual in first..last {
            let (row, subrow) = layout.locate(visual);
            let segment = layout.segment(&e.text, row, subrow);
            let line = e.text.line(row).to_string();
            let start = e.text.line_to_byte(row);
            let y = self.tab_height + 4. + visual as f64 * self.line_height - self.scroll_y;
            let baseline = y + self.ascent + 3. + e.settings.number("line_padding_top", 0.).max(0.);
            if e.preferences.highlight_line && caret_visuals.contains(&visual) {
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
                        text_run(
                            &mut layer,
                            if is_folded { "▸" } else { "▾" },
                            gutter - 10.,
                            baseline,
                            e.preferences.font_size,
                            global("fold_marker"),
                            FontStyle::default(),
                            all,
                        );
                    }
                }
            }
            let display = self.line_units(&line, layout.hidden(row), e.preferences.tab_size);
            let relevant: Vec<_> = display
                .iter()
                .filter(|u| {
                    u.bytes.start >= segment.bytes.start && u.bytes.end <= segment.bytes.end
                })
                .collect();
            let mut column = segment.indent;
            let range_start = start + segment.bytes.start;
            let range_end = start + segment.bytes.end;
            for selection in regions
                .iter()
                .filter(|r| !r.empty() && r.begin() <= range_end && r.end() > range_start)
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
                fill(
                    &mut layer,
                    rect,
                    clip,
                    global(if self.focused {
                        "selection"
                    } else {
                        "inactive_selection"
                    }),
                );
                border(&mut layer, rect, clip, global("selection_border"));
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
            if e.settings.bool("draw_indent_guides", true) {
                let leading = line.len() - line.trim_start_matches([' ', '\t']).len();
                let columns =
                    crate::editor::cells_with_tab(&line[..leading], e.preferences.tab_size);
                let caret_line = e.text.line(caret_row).to_string();
                let caret_indent =
                    vel_syntax::indent::indentation(&caret_line, e.preferences.tab_size);
                let guide_options = e
                    .settings
                    .get("indent_guide_options")
                    .and_then(vel_config::Value::as_array);
                for col in (e.preferences.tab_size..=columns).step_by(e.preferences.tab_size) {
                    let active = col == caret_indent;
                    let enabled = guide_options.is_none_or(|o| {
                        o.iter()
                            .any(|s| s == if active { "draw_active" } else { "draw_normal" })
                    });
                    if enabled {
                        let name = if active {
                            "active_guide"
                        } else if col < caret_indent {
                            "stack_guide"
                        } else {
                            "guide"
                        };
                        fill(
                            &mut layer,
                            Rect {
                                x: left + col as f64 * self.cell - self.scroll_x,
                                y,
                                width: 1.,
                                height: self.line_height,
                            },
                            clip,
                            global(name),
                        );
                    }
                }
            }
            let mut run = String::new();
            let mut run_x = 0.;
            let mut run_style: Option<(Color, FontStyle, Decoration)> = None;
            for unit in relevant {
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
                let style = if e.marked.is_none() {
                    language
                        .and_then(|l| l.styled.get(&row))
                        .and_then(|line| {
                            line.spans
                                .iter()
                                .find(|s| s.range.contains(&unit.bytes.start))
                        })
                        .map(|s| s.style)
                        .unwrap_or(default)
                } else {
                    default
                };
                let fg = if selected {
                    style.selection_foreground.map(rgba).unwrap_or_else(|| {
                        global(if self.focused {
                            "selection_foreground"
                        } else {
                            "inactive_selection_foreground"
                        })
                    })
                } else if found {
                    global("find_highlight_foreground")
                } else if text == "…" {
                    global("fold_marker")
                } else {
                    rgba(style.foreground)
                };
                let font = FontStyle {
                    bold: style.font_style.contains(Decoration::BOLD) && !no_bold,
                    italic: style.font_style.contains(Decoration::ITALIC) && !no_italic,
                };
                let signature = (fg, font, style.font_style);
                let visible = x + unit.width as f64 * self.cell >= gutter && x < right;
                if (run_style != Some(signature) || text == "\t" || !visible)
                    && let Some((fg, font, decoration)) = run_style.take()
                {
                    self.flush(
                        &mut layer, &mut run, run_x, baseline, fg, font, decoration, clip, e,
                    );
                }
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
        drop(layout);
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
            if e.preferences.caret_style == "block" {
                caret.width = self.cell;
            } else if e.preferences.caret_style == "underline" {
                caret.y += caret.height - 2.;
                caret.height = 2.;
                caret.width = self.cell;
            }
            let mut color = global("caret");
            color[3] *= alpha;
            if self.focused {
                fill(&mut carets, caret, clip, color);
            }
        }
        let mut minimap = Layer {
            minimap: true,
            ..Layer::default()
        };
        self.paint_minimap(e, language, scheme, &mut minimap);
        let mut popup = Layer::default();
        e.completion.paint(e, self, &mut popup);
        let mut layers = vec![layer, carets, minimap];
        if e.completion.visible() {
            layers.push(popup);
        }
        Scene {
            background: rgba(default.background),
            layers,
        }
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
        layer.text.push(TextRun {
            text: std::mem::take(run),
            x: x as f32,
            baseline: baseline as f32,
            size: e.preferences.font_size as f32,
            color,
            style: font,
            clip,
        });
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
                let options = scheme
                    .globals
                    .value(&format!("{key}_options"))
                    .and_then(vel_config::Value::as_str)
                    .unwrap_or("underline");
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
        let bounds = Rect {
            x: self.size.width - width,
            y: self.tab_height,
            width,
            height: (self.size.height - self.tab_height).max(1.),
        };
        fill(
            layer,
            bounds,
            bounds,
            rgba(scheme.default_style().background),
        );
        if let Some(language) = language {
            for line in language.minimap.iter() {
                let y =
                    bounds.y + line.row as f64 / e.text.len_lines().max(1) as f64 * bounds.height;
                for token in &line.tokens {
                    if token.column as f64 * 0.65 >= width {
                        break;
                    }
                    fill(
                        layer,
                        Rect {
                            x: bounds.x + token.column as f64 * 0.65,
                            y,
                            width: (token.width as f64 * 0.65).max(0.65),
                            height: 1.,
                        },
                        bounds,
                        token.color,
                    );
                }
            }
        }
        if e.settings.bool("always_show_minimap_viewport", true)
            || self.minimap_hover
            || self.minimap_drag
        {
            let first = (self.scroll_y / self.line_height).max(0.) as usize;
            let layout = e.layout.borrow();
            let (row, _) = layout.locate(first);
            let (last, _) = layout.locate(first + self.rows());
            let rect = Rect {
                x: bounds.x,
                y: bounds.y + row as f64 / e.text.len_lines().max(1) as f64 * bounds.height,
                width,
                height: ((last + 1 - row) as f64 / e.text.len_lines().max(1) as f64
                    * bounds.height)
                    .max(3.),
            };
            let mut shade = color(scheme, "minimap_border");
            shade[3] *= 0.15;
            fill(layer, rect, bounds, shade);
            border(layer, rect, bounds, color(scheme, "minimap_border"));
        }
        fill(
            layer,
            Rect {
                x: bounds.x,
                y: bounds.y,
                width: 1.,
                height: bounds.height,
            },
            bounds,
            color(scheme, "minimap_border"),
        );
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
        assert!(v.scene(&e).layers[0].rects.len() >= 4);
        v.reveal(&e);
        let caret = v.rect_at(&e, e.caret).clamped(v.size);
        assert!(caret.y >= 0. && caret.y + caret.height <= v.size.height);
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
