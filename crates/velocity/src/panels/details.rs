//! Find/overlay transitions and status presentation shared by native and headless routes.
use super::*;
use std::time::Duration;

/// Find-panel action area: 28x24 icon toggles, 56–88x24 buttons, a message slot that keeps
/// at least `MESSAGE_MIN` DIPs, and 6 DIPs between wrapped rows.
const TOGGLE_WIDTH: f64 = 28.;
const TOGGLE_GAP: f64 = 4.;
const ACTION_HEIGHT: f64 = 24.;
const ACTION_ROW_GAP: f64 = 6.;
const BUTTON_WIDTH: f64 = 88.;
const BUTTON_MIN: f64 = 56.;
const BUTTON_GAP: f64 = 6.;
const MESSAGE_MIN: f64 = 96.;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct FindSizing {
    pub(super) caption: f64,
    button: f64,
}

/// Right-hand status-bar items, in placement priority (right to left).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StatusItem {
    Syntax,
    Binary,
    Indentation,
    Git,
    Indexing,
}
/// Laid-out find-panel controls; toggles follow [`Panels::find_toggle_options`] order.
#[derive(Clone, Debug, Default)]
pub(crate) struct FindControls {
    pub toggles: Vec<Rect>,
    pub buttons: Vec<Rect>,
    pub message: Rect,
    pub rows: usize,
}

impl Panels {
    pub(super) fn panel_buttons(panel: Panel) -> &'static [(usize, &'static str)] {
        match panel {
            Panel::Find | Panel::Incremental => {
                &[(0, "find_next"), (1, "find_prev"), (2, "find_all")]
            }
            Panel::Replace => &[(0, "find_next"), (3, "replace_next"), (4, "replace_all")],
            Panel::Files => &[(0, "find_in_files_start"), (3, "replace_in_files")],
            _ => &[],
        }
    }
    /// Indices into FIND_OPTIONS. Context and Use Buffer are specific to Find in Files.
    pub(crate) fn find_toggle_options(panel: Panel) -> &'static [usize] {
        match panel {
            Panel::Files => &[0, 1, 2, 5, 6, 7],
            Panel::Replace => &[0, 1, 2, 3, 4, 5],
            Panel::Find | Panel::Incremental => &[0, 1, 2, 3, 4],
            _ => &[],
        }
    }
    pub(super) fn find_fields(panel: Panel) -> usize {
        match panel {
            Panel::Replace => 2,
            Panel::Files => 3,
            Panel::Find | Panel::Incremental | Panel::Input => 1,
            _ => 0,
        }
    }
    /// Width (DIPs) left of the inputs for their labels: 80, less when the input would get
    /// narrower than 120 DIPs, and 8 (no labels) below 64. `close` is the close button's
    /// reserve in DIPs (zero for panels without one).
    pub(super) fn find_label_column(width: f64, s: f64, close: f64, advance: f64) -> f64 {
        let room = width / s - 12. - close - 120.;
        if room >= 64. {
            (advance.ceil() + 16.).max(80.).min(room)
        } else {
            8.
        }
    }
    /// Width (DIPs) left of the input panel's field for its caption. Unlike the find panels'
    /// one-word labels the caption is free text from the command ("New name for this entry",
    /// a translation, a Vintage prompt), so the column follows the caption's measured width
    /// (`caption`, DIPs) plus 8 on each side: at least the find panels' 80 so short captions
    /// line up with them, at most half the panel so the field keeps the other half (a longer
    /// caption ends in an ellipsis), and 8 (no caption) below 64. Before the caption has been
    /// measured, the find panels' column.
    pub(super) fn input_label_column(width: f64, s: f64, caption: Option<f64>) -> f64 {
        let Some(caption) = caption else {
            return Self::find_label_column(width, s, 0., 0.);
        };
        let label = (caption + 16.).max(80.).min((width / s - 12.) / 2.);
        if label >= 64. { label } else { 8. }
    }
    /// Measures the input panel's caption with the style it is painted with. When that changed
    /// (a new caption, theme, font or UI scale) the panel is laid out again before it is
    /// painted, so the field never starts under its caption, not even on the first frame.
    pub(super) fn fit_input_caption(&mut self, workspace: &Workspace) -> bool {
        if self.panel != Some(Panel::Input) {
            return false;
        }
        let advance = {
            let mut theme = workspace.themes.ui.borrow_mut();
            let style = self.tree.style(
                2,
                Some("label_control"),
                &mut theme,
                &workspace.editor().settings,
            );
            theme.advance(&style, &self.input_caption) / self.ui_scale
        };
        if self.input_caption_advance != Some(advance) {
            self.input_caption_advance = Some(advance);
            self.layout(self.size);
            return true;
        }
        false
    }
    pub(super) fn measure_find_controls(&mut self, workspace: &Workspace) -> bool {
        let Some(panel) = self
            .panel
            .filter(|p| !Self::find_toggle_options(*p).is_empty())
        else {
            return false;
        };
        let settings = &workspace.editor().settings;
        let mut theme = workspace.themes.ui.borrow_mut();
        let key = (panel, self.ui_scale, workspace.ui.revision, theme.revision);
        if self.find_measurement == Some(key) {
            return false;
        }
        self.find_measurement = Some(key);
        let label = self
            .tree
            .style(2, Some("label_control"), &mut theme, settings);
        let caption = workspace.ui.find_fields[..Self::find_fields(panel)]
            .iter()
            .map(|name| theme.advance(&label, name) / self.ui_scale)
            .fold(0., f64::max);
        let button = Self::panel_buttons(panel)
            .iter()
            .enumerate()
            .map(|(i, (caption, _))| {
                let style = self.tree.style(50 + i as u64, None, &mut theme, settings);
                theme.advance(&style, &workspace.ui.find_buttons[*caption]) / self.ui_scale + 8.
            })
            .fold(BUTTON_MIN, f64::max);
        let sizing = FindSizing {
            caption,
            button: button.ceil(),
        };
        if self.find_sizing != sizing {
            self.find_sizing = sizing;
            self.find_layout = None;
            self.layout(self.size);
            return true;
        }
        false
    }
    /// Height a find panel needs at its natural size, in DIPs, for a panel of `width` pixels.
    fn find_panel_height(panel: Panel, width: f64, s: f64, sizing: FindSizing) -> f64 {
        let rows = if Self::find_toggle_options(panel).is_empty() {
            0
        } else {
            Self::find_controls_at(panel, 0., width, s, s, 0., sizing).rows
        };
        let actions = if rows == 0 {
            0.
        } else {
            rows as f64 * (ACTION_HEIGHT + ACTION_ROW_GAP) - ACTION_ROW_GAP
        };
        if panel == Panel::Input {
            // One row: the caption and its field, with the find panels' 8-DIP margins.
            return 8. + 28. + 8.;
        }
        8. + Self::find_fields(panel) as f64 * PANEL_ROW + actions + 8.
    }
    /// Vertical scale of the open find panel: the UI scale, or less when a short window
    /// clamps the panel below its natural height (everything then shrinks to fit).
    pub(super) fn find_vertical_scale(&self) -> f64 {
        let Some(panel) = self.panel else {
            return self.ui_scale;
        };
        let natural = self.find_panel_natural_height(panel, self.panel_bounds.width);
        (self.panel_bounds.height / natural.max(1.)).min(self.ui_scale)
    }
    pub(super) fn find_input_extras(&self, panel: Panel) -> [f64; 3] {
        if panel == Panel::Input {
            return [0.; 3];
        }
        [
            18. * self.query.natural_rows().saturating_sub(1) as f64,
            if matches!(panel, Panel::Replace | Panel::Files) {
                18. * self.replace.natural_rows().saturating_sub(1) as f64
            } else {
                0.
            },
            0.,
        ]
    }
    pub(super) fn find_panel_natural_height(&self, panel: Panel, width: f64) -> f64 {
        Self::find_panel_height(panel, width, self.ui_scale, self.find_sizing)
            + self.find_input_extras(panel).iter().sum::<f64>()
    }
    /// Toggles, buttons and message slot of the open find panel, as laid out and hit-tested.
    /// Cached by geometry: painting asks for it several times a frame.
    pub(crate) fn find_controls(&mut self) -> Option<FindControls> {
        let panel = self.panel?;
        let vertical = self.find_vertical_scale();
        if let Some((p, bounds, scale, v, controls)) = &self.find_layout
            && *p == panel
            && *bounds == self.panel_bounds
            && *scale == self.ui_scale
            && *v == vertical
        {
            return Some(controls.clone());
        }
        let controls = self.compute_find_controls()?;
        self.find_layout = Some((
            panel,
            self.panel_bounds,
            self.ui_scale,
            vertical,
            controls.clone(),
        ));
        Some(controls)
    }
    fn compute_find_controls(&self) -> Option<FindControls> {
        let panel = self.panel?;
        if Self::find_toggle_options(panel).is_empty() {
            return None;
        }
        let v = self.find_vertical_scale();
        let top = self.panel_bounds.y
            + (8.
                + Self::find_fields(panel) as f64 * PANEL_ROW
                + self.find_input_extras(panel).iter().sum::<f64>())
                * v;
        Some(Self::find_controls_at(
            panel,
            self.panel_bounds.x,
            self.panel_bounds.width,
            self.ui_scale,
            v,
            top,
            self.find_sizing,
        ))
    }
    /// Flow layout of the action area. At normal widths it is one row aligned with the inputs:
    /// toggles on the left, the message in the middle, buttons on the right. When that does
    /// not fit, buttons move to their own row (and toggles or buttons wrap further in very
    /// narrow panels), so controls never overlap and the message keeps a readable width.
    fn find_controls_at(
        panel: Panel,
        x: f64,
        width: f64,
        s: f64,
        v: f64,
        top: f64,
        sizing: FindSizing,
    ) -> FindControls {
        let toggles = Self::find_toggle_options(panel).len();
        let buttons = Self::panel_buttons(panel).len();
        // Inputs span [x + label, x + width - 12 - reserve] (DIPs): 40 beside the close
        // button, 66 when Find in Files also shows its Where button. The buttons end with the
        // inputs, not under the edge buttons. The full area keeps an 8-DIP margin.
        let reserve = Self::find_edge_reserve(panel, width, s);
        let label = Self::find_label_column(width, s, reserve, sizing.caption);
        let aligned = (x + label * s, x + width - (12. + reserve) * s);
        let full = (x + 8. * s, x + width - 8. * s);
        let span = |r: (f64, f64)| ((r.1 - r.0) / s).max(0.);
        let run = |n: usize, item: f64, gap: f64| {
            if n == 0 {
                0.
            } else {
                n as f64 * item + (n - 1) as f64 * gap
            }
        };
        let toggle_run = run(toggles, TOGGLE_WIDTH, TOGGLE_GAP);
        let min_button_width = sizing.button.max(BUTTON_MIN);
        let max_button_width = min_button_width.max(BUTTON_WIDTH);
        let button_min = run(buttons, min_button_width, BUTTON_GAP);
        let pitch = (ACTION_HEIGHT + ACTION_ROW_GAP) * v;
        let height = ACTION_HEIGHT * v;
        let row_y = |row: usize| top + row as f64 * pitch;
        let mut out = FindControls::default();
        let a = span(aligned);
        if toggle_run + 8. + MESSAGE_MIN + 8. + button_min <= a {
            let room = a - toggle_run - 16. - MESSAGE_MIN - run(buttons, 0., BUTTON_GAP);
            let bw = (room / buttons.max(1) as f64).clamp(min_button_width, max_button_width);
            for i in 0..toggles {
                out.toggles.push(Rect {
                    x: aligned.0 + i as f64 * (TOGGLE_WIDTH + TOGGLE_GAP) * s,
                    y: top,
                    width: TOGGLE_WIDTH * s,
                    height,
                });
            }
            let first = aligned.1 - run(buttons, bw, BUTTON_GAP) * s;
            for i in 0..buttons {
                out.buttons.push(Rect {
                    x: first + i as f64 * (bw + BUTTON_GAP) * s,
                    y: top,
                    width: bw * s,
                    height,
                });
            }
            let left = aligned.0 + (toggle_run + 8.) * s;
            out.message = Rect {
                x: left,
                y: top,
                width: (first - 8. * s - left).max(0.),
                height,
            };
            out.rows = 1;
            return out;
        }
        let region = if toggle_run.max(button_min) + 8. + MESSAGE_MIN <= a {
            aligned
        } else {
            full
        };
        let w = span(region);
        let mut row = 0;
        // Toggles, wrapping when even one row of them does not fit.
        let toggle_w = TOGGLE_WIDTH.min(w);
        let per = (((w + TOGGLE_GAP) / (toggle_w + TOGGLE_GAP)).floor() as usize).max(1);
        let mut last_end = region.0;
        for i in 0..toggles {
            let (r, c) = (i / per, i % per);
            let rect = Rect {
                x: region.0 + c as f64 * (toggle_w + TOGGLE_GAP) * s,
                y: row_y(row + r),
                width: toggle_w * s,
                height,
            };
            last_end = rect.x + rect.width;
            out.toggles.push(rect);
        }
        let toggle_rows = toggles.div_ceil(per);
        let message_on_toggles = (region.1 - last_end) / s - 8. >= MESSAGE_MIN;
        if message_on_toggles && toggle_rows > 0 {
            out.message = Rect {
                x: last_end + 8. * s,
                y: row_y(row + toggle_rows - 1),
                width: region.1 - last_end - 8. * s,
                height,
            };
        }
        row += toggle_rows;
        // Buttons, right-aligned on the following row(s).
        let per =
            (((w + BUTTON_GAP) / (min_button_width.min(w) + BUTTON_GAP)).floor() as usize).max(1);
        let in_row = buttons.min(per).max(1);
        let bw = ((w - run(in_row, 0., BUTTON_GAP)) / in_row as f64)
            .clamp(min_button_width.min(w), max_button_width.min(w));
        let first_row_buttons = buttons.min(per);
        let first_row_left = region.1 - run(first_row_buttons, bw, BUTTON_GAP) * s;
        let message_on_buttons =
            !message_on_toggles && (first_row_left - region.0) / s - 8. >= MESSAGE_MIN;
        if !message_on_toggles && !message_on_buttons {
            // A dedicated, full-width message row.
            out.message = Rect {
                x: region.0,
                y: row_y(row),
                width: region.1 - region.0,
                height,
            };
            row += 1;
        }
        for i in 0..buttons {
            let (r, c) = (i / per, i % per);
            let count = (buttons - r * per).min(per);
            let left = region.1 - run(count, bw, BUTTON_GAP) * s;
            out.buttons.push(Rect {
                x: left + c as f64 * (bw + BUTTON_GAP) * s,
                y: row_y(row + r),
                width: bw * s,
                height,
            });
        }
        if message_on_buttons {
            out.message = Rect {
                x: region.0,
                y: row_y(row),
                width: first_row_left - 8. * s - region.0,
                height,
            };
        }
        out.rows = row + buttons.div_ceil(per);
        out
    }
    pub(super) fn find_edge_reserve(panel: Panel, width: f64, s: f64) -> f64 {
        CLOSE_BUTTON
            + 6.
            + if panel == Panel::Files && width >= 272. * s {
                CLOSE_BUTTON + 4.
            } else {
                0.
            }
    }
    /// Quick-panel row height in DIPs. Command lists (palette, syntax and snippet choices)
    /// show only a caption and key binding; other overlays have a details line.
    /// Lists without any details line (commands, symbols) use compact rows too.
    pub(super) fn quick_row_height(&self) -> f64 {
        let no_details =
            !self.quick.items.is_empty() && self.quick.items.iter().all(|i| i.details.is_empty());
        if self.quick_compact() || no_details {
            32.
        } else {
            44.
        }
    }
    /// Overlay header (input) and footer heights in DIPs.
    pub(super) fn quick_chrome(&self) -> (f64, f64) {
        if self.quick_tight {
            (40., 0.)
        } else {
            (46., 26.)
        }
    }
    /// Top of the overlay's row list.
    pub(crate) fn quick_list_top(&self) -> f64 {
        self.quick_bounds.y + self.quick_chrome().0 * self.ui_scale
    }
    pub(super) fn quick_compact(&self) -> bool {
        matches!(
            self.overlay,
            Some(Overlay::Palette | Overlay::Syntax | Overlay::Choices)
        )
    }
    /// The find panel's status message slot (match counts, errors).
    pub(super) fn panel_message_clip(&self) -> Rect {
        match &self.find_layout {
            Some((panel, bounds, scale, vertical, controls))
                if Some(*panel) == self.panel
                    && *bounds == self.panel_bounds
                    && *scale == self.ui_scale
                    && *vertical == self.find_vertical_scale() =>
            {
                controls.message
            }
            _ => self
                .compute_find_controls()
                .map(|c| c.message)
                .unwrap_or_default(),
        }
    }
    /// Whether a find panel shows the status message itself: its slot must be tall enough
    /// for the 12-point label (very short windows shrink the panel); otherwise the status bar
    /// shows it.
    pub(crate) fn message_in_find_panel(&self) -> bool {
        self.find_panel_visible() && {
            let slot = self.panel_message_clip();
            slot.height >= 16. * self.ui_scale && slot.width >= 24. * self.ui_scale
        }
    }
    pub(super) fn paint_find_edge_buttons(
        &self,
        theme: &mut vel_ui::theme::ThemeUi,
        settings: &vel_config::settings::Settings,
        layer: &mut Layer,
        clip: Rect,
    ) {
        for (index, (id, class)) in [
            (CLOSE_ID, "panel_close_button"),
            (WHERE_ID, "panel_close_button panel_where_button"),
        ]
        .into_iter()
        .enumerate()
        {
            let Some(node) = self.tree.nodes.get(&id).filter(|n| n.visible) else {
                continue;
            };
            let attributes: Vec<_> = node.attributes.iter().copied().collect();
            let style = theme.style(
                StyleId::FindClose.at(index as u64),
                &vel_theme::ElementPath(&[
                    vel_theme::Element::new("window", &[]),
                    vel_theme::Element::new("panel_control", &[]),
                    vel_theme::Element::new(class, &attributes),
                ]),
                settings,
            );
            theme.icon_surface(layer, &style, node.rect, clip);
        }
    }
    /// Plain-language name and shortcut of the find-panel control under the pointer.
    pub(super) fn panel_tooltip(&self, workspace: &Workspace, point: Point) -> Option<String> {
        if !self.find_panel_visible() {
            return None;
        }
        let id = self.tree.hit_test(point)?;
        let (name, invocation) = match id {
            WHERE_ID => {
                return self
                    .hit_find_options(point)
                    .then(|| workspace.ui.where_menu.to_string());
            }
            30..=37 => {
                let i = (id - 30) as usize;
                let option = FIND_OPTIONS[i];
                (
                    if option == "use_buffer" {
                        workspace.ui.show_results_in_tab.as_ref()
                    } else {
                        workspace.ui.find_options[i].as_ref()
                    },
                    Invocation {
                        command: "toggle_find_option".into(),
                        args: json!({ "option": option }),
                    },
                )
            }
            CLOSE_ID => (
                workspace.ui.close.as_ref(),
                Invocation {
                    command: "hide_panel".into(),
                    args: json!({}),
                },
            ),
            _ => {
                let (caption, command) = self
                    .panel
                    .and_then(|p| Self::panel_buttons(p).get(id.checked_sub(50)? as usize))?;
                (
                    workspace.ui.find_buttons[*caption].as_ref(),
                    Invocation {
                        command: (*command).into(),
                        args: json!({}),
                    },
                )
            }
        };
        let tr = &workspace.translations;
        let name = match id {
            30..=35 => {
                let on = [
                    self.options.regex,
                    self.options.case_sensitive,
                    self.options.whole_word,
                    self.options.wrap,
                    self.in_selection,
                    self.options.preserve_case,
                ][(id - 30) as usize];
                if on {
                    tr.tr_fmt("{name}: on", &[("name", name.to_owned())])
                } else {
                    tr.tr_fmt("{name}: off", &[("name", name.to_owned())])
                }
            }
            _ => name.to_owned(),
        };
        Some(match workspace.editor().keymap.hint(&invocation) {
            Some(keys) => format!("{name} ({keys})"),
            None => name,
        })
    }
    pub(crate) fn set_message(&mut self, message: impl Into<UiText>) {
        self.message = message.into();
        self.message_seen.clone_from(&self.message);
        self.message_until = Some(Instant::now() + Duration::from_secs(5));
    }
    pub(crate) fn refresh_locale(&mut self, workspace: &Workspace) {
        let tr = &workspace.translations;
        self.message.refresh(tr);
        self.message_seen.refresh(tr);
        self.find_status.refresh(tr);
        if let Some(error) = &mut self.find_error {
            error.refresh(tr);
        }
        self.result_status.refresh(tr);
        self.input_caption.refresh(tr);
        self.refresh_output_locale(tr);
    }
    pub(super) fn panel_status_message(&self, workspace: &Workspace) -> &str {
        if self.document_find_visible()
            && !self.find_status.is_empty()
            && self
                .search_identity
                .as_ref()
                .is_some_and(|key| key.snapshot == FindSnapshot::current(workspace))
        {
            &self.find_status
        } else {
            &self.message
        }
    }
    pub(super) fn status_message(&self, workspace: &Workspace) -> &str {
        if self.message.is_empty() {
            self.panel_status_message(workspace)
        } else {
            &self.message
        }
    }
    /// Next native wake for feedback expiry or a bounded, slow hover-scope retry.
    pub fn message_deadline(&self) -> Option<Instant> {
        let hover = (!self.hover_term.is_empty()
            && !self.hover_requested
            && self.hover_since.elapsed() < Duration::from_secs(2))
        .then_some(self.hover_retry)
        .flatten();
        self.message_until.into_iter().chain(hover).min()
    }
    pub(super) fn expire_message(&mut self, now: Instant) -> bool {
        if self.message != self.message_seen {
            self.message_seen.clone_from(&self.message);
            self.message_until = (!self.message.is_empty()).then_some(now + Duration::from_secs(5));
        }
        if self.message_until.is_some_and(|until| now >= until) {
            self.message.clear();
            self.message_seen.clear();
            self.message_until = None;
            return true;
        }
        false
    }
    pub(super) fn cancel_incremental(&mut self, workspace: &mut Workspace) {
        let Some((origin, buffer, version)) = self.incremental_origin.take() else {
            return;
        };
        let _navigation = crate::editor::navigation::Guard::suppress(&workspace.jump_history);
        self.find_jump_origin = None;

        if let Some(index) = workspace
            .tabs
            .iter()
            .position(|t| t.sheet == origin.sheet && t.editor.buffer.id() == buffer)
        {
            workspace.select(index);
            let editor = workspace.editor_mut();
            let regions: Option<Vec<_>> = origin
                .selection
                .iter()
                .map(|r| editor.buffer.transform_region_from(r, version))
                .collect();
            if let Some(mut regions) = regions {
                let primary = regions.remove(origin.selection.primary_index());
                regions.push(primary);
                editor.set_selection(Selection::from_regions(regions));
            }
            workspace.tabs[index].scroll = origin.scroll;
            self.restore_scroll = Some(origin.scroll);
            self.preserve_scroll = true;
            self.reveal = false;
        }
        workspace
            .jump_history
            .borrow_mut()
            .reset_current(crate::intelligence::capture(workspace));
    }
    pub(super) fn preview_find(&mut self, workspace: &mut Workspace, origin: usize) {
        let _navigation = crate::editor::navigation::Guard::suppress(&workspace.jump_history);
        if self.in_selection {
            return;
        }
        let hit = self
            .highlights
            .get(self.highlights.partition_point(|r| r.start < origin))
            .or_else(|| self.options.wrap.then(|| self.highlights.first()).flatten())
            .cloned();
        if let Some(hit) = hit {
            workspace
                .editor_mut()
                .set_selection(Selection::from_regions([Region::from(hit)]));
            self.remember_find_selection(workspace);
            self.reveal = true;
        }
        workspace
            .jump_history
            .borrow_mut()
            .reset_current(crate::intelligence::capture(workspace));
    }
    /// A cancelled preview restores its exact viewport; skip the event's generic caret reveal.
    pub fn should_reveal(&mut self, requested: bool) -> bool {
        let preserve = std::mem::take(&mut self.preserve_scroll);
        requested && !preserve
    }
    pub(super) fn walk_history(
        &mut self,
        workspace: &mut Workspace,
        backwards: bool,
        viewport: Range<usize>,
    ) {
        let field = self.focus_input;
        if field > 1 {
            return;
        }
        let history = if field == 0 {
            &self.find_history
        } else {
            &self.replace_history
        };
        if history.is_empty() {
            return;
        }
        let cursor = self.history_cursor[field];
        if cursor.is_none() {
            if !backwards {
                return;
            }
            self.history_draft[field] = self.input().expect("focused field").value();
        }
        let next = if backwards {
            Some(cursor.map_or(history.len() - 1, |i| i.saturating_sub(1)))
        } else {
            cursor.and_then(|i| (i + 1 < history.len()).then_some(i + 1))
        };
        let value = next.map_or_else(|| self.history_draft[field].clone(), |i| history[i].clone());
        self.input_mut().expect("focused field").set(&value);
        self.input_changed(workspace, viewport);
        self.history_cursor[field] = next;
    }
    /// Makes builds the navigation source while any hidden file search continues.
    pub fn select_build_results(&mut self) {
        self.search_results_active = false;
    }
    /// Whether F4 can navigate the last search's match lines (context rows are skipped).
    pub fn has_search_results(&self) -> bool {
        self.result_view
            .rows
            .iter()
            .any(|r| matches!(r, files::Row::Line(line) if line.match_count > 0))
    }
    /// Walks search results through the same jump path used by Enter and the mouse.
    pub fn navigate_result(
        &mut self,
        workspace: &mut Workspace,
        forward: bool,
    ) -> Result<(), String> {
        let rows = &self.result_view.rows;
        let is_hit =
            |i: &usize| matches!(rows.get(*i), Some(files::Row::Line(l)) if l.match_count > 0);
        let cursor = self.result_cursor;
        let next = if forward {
            let first = cursor.map_or(0, |i| i + 1);
            (first..rows.len()).chain(0..first).find(is_hit)
        } else {
            let first = cursor.unwrap_or(rows.len());
            (0..first)
                .rev()
                .chain((first..rows.len()).rev())
                .find(is_hit)
        };
        if let Some(index) = next {
            self.result_selected = index;
            self.result_navigated = true;
            self.result_scroll.reveal(index);
            self.jump_result(workspace)?;
        }
        Ok(())
    }
    pub(super) fn overlay_footer<'a>(&self, workspace: &'a Workspace) -> &'a str {
        let ui = &workspace.ui;
        match self.overlay {
            Some(Overlay::Palette) => &ui.palette_footer,
            Some(Overlay::Appearance) => &ui.appearance_footer,
            Some(Overlay::Syntax | Overlay::Choices) => &ui.choice_footer,
            Some(Overlay::ProjectSymbols | Overlay::Locations) => &ui.jump_footer,
            Some(Overlay::Goto) => &ui.goto_footer,
            None => "",
        }
    }
    pub(super) fn overlay_prompt<'a>(&self, workspace: &'a Workspace) -> &'a str {
        let ui = &workspace.ui;
        match self.overlay {
            Some(Overlay::Palette) => &ui.palette_prompt,
            Some(Overlay::Goto) => match self.quick_input.buffer.text().chars().next() {
                Some(':') => &ui.line_prompt,
                Some('@') => &ui.symbol_prompt,
                _ => &ui.goto_prompt,
            },
            Some(Overlay::Syntax) => &ui.syntax_prompt,
            Some(Overlay::Appearance) => &ui.appearance_prompt,
            Some(Overlay::ProjectSymbols) => &ui.project_symbol_prompt,
            Some(Overlay::Locations) => &ui.location_prompt,
            Some(Overlay::Choices) | None => &ui.choice_prompt,
        }
    }
    /// Whether the native command dispatcher is validating the accepted palette row.
    pub fn palette_accept_pending(&self, name: &str, args: &Value) -> bool {
        self.palette_accept
            .as_ref()
            .is_some_and(|i| i.command == name && &i.args == args)
    }
    /// Completes acceptance only after the app rechecks this row against the current context.
    pub fn finish_palette_accept(
        &mut self,
        workspace: &mut Workspace,
        name: &str,
        args: &Value,
        enabled: bool,
    ) {
        if !self.palette_accept_pending(name, args) {
            return;
        }
        let invocation = self.palette_accept.take().expect("pending palette row");
        if !enabled || self.overlay != Some(Overlay::Palette) {
            return;
        }
        self.uses += 1;
        self.palette_recent.insert(
            format!("{}:{}", invocation.command, invocation.args),
            self.uses,
        );
        self.overlay = None;
        if let Some(source) = self.preview.take()
            && let Some(tab) = workspace.tabs.iter_mut().find(|t| t.sheet == source.sheet)
        {
            tab.transient = source.transient;
        }
        self.prepared.clear();
        self.focus.show(Surface::Overlay, false);
    }
    /// Supplies the native app's `command_enabled` results without a second enablement policy.
    pub fn set_palette_enabled(&mut self, enabled: Vec<bool>) {
        if self.palette_enabled != enabled {
            self.palette_enabled = enabled;
            if self.overlay == Some(Overlay::Palette) {
                self.prepared.clear();
            }
        }
    }
    pub(super) fn palette_command_enabled(&self, workspace: &Workspace, index: usize) -> bool {
        self.palette_enabled.get(index).copied().unwrap_or_else(|| {
            let command = &workspace.palette[index].invocation;
            workspace.editor().registry.is_enabled(
                &command.command,
                &command.args,
                workspace.editor(),
            )
        })
    }
    /// Where the last paint placed a status-bar item, as its full-height click target (None
    /// before the first paint, and when it did not fit or is absent).
    pub(crate) fn status_item(&self, item: StatusItem) -> Option<Rect> {
        self.status_items
            .as_ref()?
            .iter()
            .find(|(i, _)| *i == item)
            .map(|(_, rect)| *rect)
    }
    /// The indentation item's click target (empty when it is not shown).
    pub(super) fn indentation_rect(&self) -> Rect {
        self.status_item(StatusItem::Indentation)
            .unwrap_or_default()
    }
    pub(super) fn indentation_choices(&mut self, workspace: &mut Workspace) {
        let mut items = Vec::new();
        for (caption, args) in [
            ("Indent Using Spaces".into(), json!({"spaces":true})),
            ("Indent Using Tabs".into(), json!({"spaces":false})),
            (
                "Convert Indentation to Spaces".into(),
                json!({"spaces":true,"convert":true}),
            ),
            (
                "Convert Indentation to Tabs".into(),
                json!({"spaces":false,"convert":true}),
            ),
        ]
        .into_iter()
        .chain(
            [1, 2, 3, 4, 5, 6, 7, 8]
                .into_iter()
                .map(|width| (format!("Tab Width: {width}"), json!({"width":width}))),
        ) {
            items.push(vel_config::ui::PaletteItem {
                caption,
                invocation: Invocation {
                    command: "set_indentation".into(),
                    args,
                },
            });
        }
        self.choices(workspace, items);
    }
    pub(super) fn set_indentation(
        &mut self,
        workspace: &mut Workspace,
        args: &Value,
    ) -> Result<(), String> {
        let editor = workspace.editor_mut();
        if let Some(spaces) = args["spaces"].as_bool() {
            editor.settings.set(
                vel_config::settings::Layer::View,
                "translate_tabs_to_spaces",
                json!(spaces),
            );
        }
        if let Some(width) = args["width"].as_u64().filter(|w| (1..=32).contains(w)) {
            editor.set_view_setting("tab_size", json!(width));
        }
        editor.sync();
        if args["convert"].as_bool() == Some(true) && !editor.read_only {
            let tab = editor.preferences.tab_size.max(1);
            let spaces = editor.preferences.translate_tabs_to_spaces;
            let mut offset = 0;
            let mut edits = Vec::new();
            for line in editor.text.lines() {
                let line = line.to_string();
                let len = line
                    .bytes()
                    .take_while(|c| matches!(c, b' ' | b'\t'))
                    .count();
                let width = line[..len].bytes().fold(0, |col, c| {
                    if c == b'\t' {
                        col + tab - col % tab
                    } else {
                        col + 1
                    }
                });
                let replacement = if spaces {
                    " ".repeat(width)
                } else {
                    format!("{}{}", "\t".repeat(width / tab), " ".repeat(width % tab))
                };
                if line[..len] != replacement {
                    edits.push((offset..offset + len, replacement));
                }
                offset += line.len() + 1;
            }
            editor.apply_edits(edits, None, vel_buffer::EditKind::Other);
            editor.sync();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vel_text::Text;

    fn setup(source: &str) -> (Workspace, Panels) {
        let workspace = Workspace::new(Text::from(source), None, None, false).unwrap();
        let panels = Panels::new(&workspace, false);
        (workspace, panels)
    }
    fn action(p: &mut Panels, w: &mut Workspace, name: &str, args: Value) {
        let end = w.editor().text.len_bytes();
        assert!(p.action(w, name, &args, 0..end).unwrap(), "{name}");
    }
    fn key(p: &mut Panels, w: &mut Workspace, key: &str) {
        let end = w.editor().text.len_bytes();
        if let vel_config::keymap::Resolution::Command(invocation) =
            p.resolve_key(w, &Key::parse(key).unwrap(), 0..end).unwrap()
            && !p
                .action(w, &invocation.command, &invocation.args, 0..end)
                .unwrap()
        {
            w.editor_mut()
                .run(&invocation.command, &invocation.args)
                .unwrap();
        }
    }
    fn wait(p: &mut Panels, w: &mut Workspace, done: impl Fn(&Panels) -> bool) {
        let until = Instant::now() + Duration::from_secs(10);
        while !done(p) {
            let end = w.editor().text.len_bytes();
            p.poll(w, 0..end);
            assert!(Instant::now() < until, "panel worker did not finish");
            std::thread::park_timeout(Duration::from_millis(1));
        }
    }
    fn type_query(p: &mut Panels, w: &mut Workspace, text: &str, viewport: Range<usize>) {
        p.query.set(text);
        p.input_changed(w, viewport);
        wait(p, w, |p| p.all_matches_ready);
    }
    fn size() -> Size {
        Size {
            width: 1200.,
            height: 800.,
        }
    }

    #[test]
    fn early_enter_never_accepts_candidates_from_another_target() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("old-file.txt");
        std::fs::write(&file, "old file").unwrap();
        for target in ["@", "#word"] {
            let (mut w, mut p) = setup("word\nsecond line");
            p.file_paths = Arc::new(vec![file.clone()]);
            p.file_candidates = Arc::new(vec![fuzzy::Candidate::new(file.to_string_lossy())]);
            action(&mut p, &mut w, "show_overlay", json!({"text":"old-file"}));
            wait(&mut p, &mut w, |p| !p.quick_pending);
            assert_eq!(p.quick.items.len(), 1);
            key(&mut p, &mut w, "escape");
            assert_eq!(w.active, 0);
            action(&mut p, &mut w, "show_overlay", json!({"text":target}));
            assert!(p.quick.items.is_empty());
            key(&mut p, &mut w, "enter");
            assert_eq!(p.overlay, Some(Overlay::Goto), "{target}");
            assert_eq!(w.active, 0);
            assert_eq!(w.editor().caret, 0);
            assert!(w.tabs[w.active].path.is_none());
        }
    }

    #[test]
    fn preferred_palette_matches_survive_the_limit_and_alias_deduplication() {
        let (w, mut p) = setup("text");
        const DISTRACTORS: usize = 205;
        let primary = 0..=DISTRACTORS;
        p.overlay = Some(Overlay::Choices);
        p.quick_query = "rust".into();
        p.palette_indices = primary.clone().collect();
        p.choice_items = primary
            .clone()
            .map(|_| vel_config::ui::PaletteItem {
                caption: String::new(),
                invocation: Invocation {
                    command: "noop".into(),
                    args: json!({}),
                },
            })
            .collect();
        p.palette_primary = primary.clone().chain(primary.clone()).collect();
        p.candidates = Arc::new(
            primary
                .map(|index| match index {
                    0 => "甲Rust",
                    DISTRACTORS => "乙",
                    _ => "候选",
                })
                .chain(std::iter::repeat_n("crust", DISTRACTORS))
                .chain(["Rust"])
                .map(fuzzy::Candidate::new)
                .collect(),
        );

        // A raw fuzzy order crowded by high-scoring mid-word hits, including
        // the alias of the direct hit. Lower-scoring preferred hits must survive.
        let mut hits: Vec<_> = (0..DISTRACTORS)
            .map(|index| fuzzy::Hit {
                index: 1 + DISTRACTORS + index,
                score: 12,
                positions: vec![1, 2, 3, 4],
            })
            .collect();
        hits.extend([
            fuzzy::Hit {
                index: 0,
                score: 10,
                positions: vec![3, 4, 5, 6],
            },
            fuzzy::Hit {
                index: 2 * DISTRACTORS + 1,
                score: 9,
                positions: vec![0, 1, 2, 3],
            },
        ]);
        hits.truncate(p.palette_match_limit());
        p.publish_quick(&w, hits);
        assert_eq!(p.quick.items.len(), 100);
        assert_eq!(p.quick.items[0].caption, "甲Rust");
        assert_eq!(p.quick.items[0].highlights, [3, 4, 5, 6]);
        assert_eq!(p.quick.items[1].caption, "乙");
        assert_eq!(p.quick.items[1].highlights, [0]);
        assert!(
            p.quick.items[2..]
                .iter()
                .all(|row| row.highlights.is_empty())
        );
        assert_eq!(p.quick.items.iter().filter(|row| row.value == 0).count(), 1);

        p.overlay = Some(Overlay::Goto);
        assert_eq!(p.palette_match_limit(), 100);
    }
    #[test]
    fn target_switch_clears_rows_and_an_unavailable_symbol_source_is_not_ranked() {
        let (mut w, mut p) = setup(&"a".repeat(600));
        action(&mut p, &mut w, "show_overlay", json!({"text":"@"}));
        // A previously prepared symbol list must not survive a buffer/source change.
        p.set_local(vec![("previous".into(), 500)], "");
        wait(&mut p, &mut w, |p| !p.quick_pending);
        w.new_file().unwrap();
        w.editor_mut()
            .run("insert", &json!({"characters":"b".repeat(600)}))
            .unwrap();
        w.editor_mut().set_caret(0, false);
        p.query_quick(&mut w);
        assert!(!p.quick_ready);
        p.preview_current(&mut w).unwrap();
        assert_eq!(w.editor().caret, 0);
        key(&mut p, &mut w, "enter");
        assert_eq!(w.editor().caret, 0);
        assert_eq!(p.overlay, Some(Overlay::Goto));
        p.quick_input.set(":1");
        p.query_quick(&mut w);
        assert!(p.quick.items.is_empty());
        assert!(p.local_positions.is_empty());
        key(&mut p, &mut w, "enter");
        assert!(p.overlay.is_none());
    }
    fn click(p: &mut Panels, w: &mut Workspace, point: Point) {
        let end = w.editor().text.len_bytes();
        p.mouse(w, point, true, false, false, 0..end).unwrap();
        p.mouse(w, point, false, false, false, 0..end).unwrap();
    }
    fn center(r: Rect) -> Point {
        Point {
            x: r.x + r.width / 2.,
            y: r.y + r.height / 2.,
        }
    }
    /// Typing moves the selection to a match; "In selection" must stay available (and
    /// clickable), since it has no key binding.
    #[test]
    fn in_selection_toggle_stays_clickable_after_typing() {
        let (mut w, mut p) = setup("one two\nthree two\nfour two\nfive\n");
        let end = w.editor().text.line_to_byte(3);
        w.editor_mut()
            .set_selection(vel_buffer::Selection::from_regions([Region::new(0, end)]));
        action(&mut p, &mut w, "show_panel", json!({"panel":"find"}));
        p.layout(size());
        p.sync_widgets(&w);
        assert!(p.tree.nodes.get(&34).is_some_and(|n| n.visible));
        p.focus_input = 0;
        type_query(&mut p, &mut w, "two", 0..end);
        for panel in ["find", "incremental_find", "replace"] {
            action(&mut p, &mut w, "show_panel", json!({"panel":panel}));
            p.layout(size());
            p.sync_widgets(&w);
            for id in [33, 34] {
                assert!(
                    p.tree.nodes.get(&id).is_some_and(|n| n.visible),
                    "{panel} {id}"
                );
            }
        }
        let before = p.in_selection;
        let toggle = p.tree.nodes[&34].rect;
        click(&mut p, &mut w, center(toggle));
        assert_ne!(p.in_selection, before);
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"find_in_files"}),
        );
        p.layout(size());
        p.sync_widgets(&w);
        assert!(!p.tree.nodes.get(&34).is_some_and(|n| n.visible));
        assert!(p.tree.nodes.get(&35).is_some_and(|n| n.visible));
    }
    /// A find message whose slot is too short to read (short window, large UI scale) moves to
    /// the status bar; at normal sizes it stays in the panel.
    #[test]
    fn find_message_moves_to_the_status_bar_when_the_panel_slot_is_too_short() {
        let (mut w, mut p) = setup("text");
        action(&mut p, &mut w, "show_panel", json!({"panel":"find"}));
        let painted = |p: &mut Panels, w: &Workspace, window: Size| {
            p.message = "3 matches".into();
            p.layout(window);
            let mut scene = Scene::default();
            p.paint(w, &mut scene);
            let status_top = window.height - STATUS * p.ui_scale;
            let runs: Vec<_> = scene
                .layers
                .iter()
                .flat_map(|l| &l.text)
                .filter(|t| t.text.contains("3 matches"))
                .map(|t| f64::from(t.baseline) >= status_top)
                .collect();
            (p.message_in_find_panel(), runs)
        };
        assert_eq!(painted(&mut p, &w, size()), (true, vec![false]));
        p.ui_scale = 2.;
        let (inside, runs) = painted(
            &mut p,
            &w,
            Size {
                width: 1000.,
                height: 210.,
            },
        );
        assert!(!inside);
        assert_eq!(runs, vec![true]);
    }
    /// The footer under the list is inert, and the overlay stays inside short windows.
    #[test]
    fn overlay_footer_selects_nothing_and_overlay_fits_short_windows() {
        let (mut w, mut p) = setup("text");
        w.palette = (0..40)
            .map(|i| vel_config::ui::PaletteItem {
                caption: format!("Command {i:02}"),
                invocation: Invocation {
                    command: "new_file".into(),
                    args: json!({ "item": i }),
                },
            })
            .collect();
        action(
            &mut p,
            &mut w,
            "show_overlay",
            json!({"overlay":"command_palette"}),
        );
        wait(&mut p, &mut w, |p| !p.quick_pending);
        p.layout(size());
        p.sync_widgets(&w);
        let rows = p.quick.scroll.viewport / p.quick.scroll.row_height;
        assert!((rows - rows.round()).abs() < 1e-6, "whole rows: {rows}");
        let footer = Point {
            x: p.quick_bounds.x + 40.,
            y: p.quick_bounds.y + p.quick_bounds.height - 6.,
        };
        let selected = p.quick.selected;
        click(&mut p, &mut w, footer);
        assert_eq!(p.quick.selected, selected);
        assert!(p.pending_command.is_none());
        assert_eq!(p.overlay, Some(Overlay::Palette));
        for scale in [1., 1.5, 2.] {
            p.ui_scale = scale;
            for height in [100., 120., 150., 200., 240., 300.] {
                p.layout(Size {
                    width: 800.,
                    height,
                });
                let q = p.quick_bounds;
                assert!(
                    q.y >= 0. && q.y + q.height <= height + 1e-6,
                    "{scale} {height}"
                );
                // From the native minimum height (200) up, at least one whole row shows.
                if height >= 200. {
                    assert!(
                        p.quick.scroll.viewport >= p.quick.scroll.row_height - 1e-6,
                        "{scale} {height}: {} < {}",
                        p.quick.scroll.viewport,
                        p.quick.scroll.row_height
                    );
                    assert!(p.quick_input.bounds.height >= 24. * scale - 1e-6);
                }
            }
        }
    }
    /// Tool tips follow option changes made by shortcut and vanish with the panel.
    #[test]
    fn find_tooltip_follows_options_and_panel_close() {
        let (mut w, mut p) = setup("text");
        action(&mut p, &mut w, "show_panel", json!({"panel":"find"}));
        p.layout(size());
        p.paint(&w, &mut Scene::default());
        let toggle = center(p.tree.nodes[&31].rect);
        p.review_hover(&w, toggle);
        assert!(
            p.review
                .tooltip
                .as_ref()
                .unwrap()
                .0
                .starts_with("Match case: off")
        );
        action(
            &mut p,
            &mut w,
            "toggle_find_option",
            json!({"option":"case_sensitive"}),
        );
        p.layout(size());
        p.paint(&w, &mut Scene::default());
        assert!(
            p.review
                .tooltip
                .as_ref()
                .unwrap()
                .0
                .starts_with("Match case: on")
        );
        action(&mut p, &mut w, "hide_panel", json!({}));
        p.layout(size());
        p.paint(&w, &mut Scene::default());
        assert!(p.review.tooltip.is_none());
    }

    #[test]
    fn find_in_files_where_matches_close_ink_center_and_hover_at_both_scales() {
        use vel_render::scene::{ImageQuad, SolidRect, Surface};

        fn glyph(scene: &Scene, button: Rect) -> &ImageQuad {
            scene
                .layers
                .iter()
                .flat_map(|layer| &layer.surfaces)
                .find_map(|surface| match surface {
                    Surface::Image(image) if contains(button, center(image.rect)) => Some(image),
                    _ => None,
                })
                .expect("themed button mask")
        }

        fn ink_center_y(glyph: &ImageQuad) -> f64 {
            let mut area = 0.;
            let mut moment = 0.;
            for (i, pixel) in glyph.image.rgba.as_chunks::<4>().0.iter().enumerate() {
                let y = i / glyph.image.width as usize;
                let alpha = f64::from(pixel[3]);
                area += alpha;
                moment += (y as f64 + 0.5) * alpha;
            }
            assert!(area > 0.);
            glyph.rect.y + glyph.rect.height * moment / (area * f64::from(glyph.image.height))
        }

        fn wash(scene: &Scene, button: Rect) -> Option<&SolidRect> {
            scene
                .layers
                .iter()
                .flat_map(|layer| &layer.surfaces)
                .find_map(|surface| match surface {
                    Surface::Fill(wash) if wash.rect == button && wash.radius > 0. => Some(wash),
                    _ => None,
                })
        }

        for scale in [1., 2.] {
            let (mut w, mut p) = setup("text");
            p.ui_scale = scale;
            action(
                &mut p,
                &mut w,
                "show_panel",
                json!({"panel":"find_in_files"}),
            );
            p.layout(size());
            let mut idle = Scene::default();
            p.paint(&w, &mut idle);
            let close = p.close_button_rect();
            let more = p.find_options_rect();
            let close_ink = glyph(&idle, close);
            let where_ink = glyph(&idle, more);
            assert_eq!(more.height, CLOSE_BUTTON * scale);
            assert!(p.hit_find_options(center(more)));
            assert_ne!(close_ink.image.id, where_ink.image.id);
            assert!(
                (ink_center_y(close_ink) - ink_center_y(where_ink)).abs() < 0.1,
                "Where and Close ink must be on the same line at {scale}×"
            );
            assert!(wash(&idle, close).is_none() && wash(&idle, more).is_none());

            let mut hover = |button| {
                p.tree.pointer(center(button), None);
                p.review_hover(&w, center(button));
                let mut scene = Scene::default();
                p.paint(&w, &mut scene);
                let wash = wash(&scene, button).expect("hover wash");
                assert_ne!(glyph(&scene, button).color, glyph(&idle, button).color);
                (wash.color, wash.radius)
            };
            assert_eq!(hover(close), hover(more));
            assert_eq!(p.review.tooltip.as_ref().unwrap().0, "Where…");
            w.settings.set(
                vel_config::settings::Layer::User,
                "ui_language",
                json!("zh-Hans"),
            );
            w.sync_ui_language();
            p.paint(&w, &mut Scene::default());
            assert_eq!(p.review.tooltip.as_ref().unwrap().0, "位置…");
        }
    }
    /// The action buttons end where the inputs end. Find in Files reserves room for its Where
    /// button beside the close button; its Find and Replace buttons ran 26 DIP past the inputs,
    /// under that pair, instead of lining up with the fields above them.
    #[test]
    fn find_panel_buttons_end_with_the_inputs() {
        for name in ["find", "replace", "find_in_files"] {
            for scale in [1., 2.] {
                for width in [640., 760., 980., 1200., 1600.] {
                    let (mut w, mut p) = setup("text");
                    p.ui_scale = scale;
                    p.side_visible = false;
                    action(&mut p, &mut w, "show_panel", json!({"panel": name}));
                    p.layout(Size {
                        width: width * scale,
                        height: 800. * scale,
                    });
                    p.sync_widgets(&w);
                    let at = format!("{name} ×{scale} {width}");
                    let controls = p.find_controls().unwrap();
                    let last = *controls.buttons.last().unwrap();
                    let input = p.query.bounds;
                    assert!(
                        (last.x + last.width - (input.x + input.width)).abs() < 0.01,
                        "{at}: button {last:?} input {input:?}"
                    );
                    // Edge buttons stay to the right of the action area, on the first row.
                    let edge = if name == "find_in_files" {
                        p.find_options_rect()
                    } else {
                        p.close_button_rect()
                    };
                    assert!(edge.x >= last.x + last.width, "{at}: {edge:?} {last:?}");
                    assert!(controls.toggles[0].x == input.x, "{at}");
                }
            }
        }
    }
    #[test]
    fn every_new_palette_query_selects_its_best_hit_including_early_enter() {
        for early in [true, false] {
            let (mut w, mut p) = setup("text");
            w.palette = [("New File", "file"), ("New Window", "window")]
                .into_iter()
                .map(|(caption, item)| vel_config::ui::PaletteItem {
                    caption: caption.into(),
                    invocation: Invocation {
                        command: "new_file".into(),
                        args: json!({"item":item}),
                    },
                })
                .collect();
            action(
                &mut p,
                &mut w,
                "show_overlay",
                json!({"overlay":"command_palette","text":"n"}),
            );
            wait(&mut p, &mut w, |p| !p.quick_pending);
            p.quick.navigate(1);
            assert_eq!(p.quick.items[p.quick.selected].caption, "New Window");
            p.quick_input.set("ne");
            p.query_quick(&mut w);
            if !early {
                wait(&mut p, &mut w, |p| !p.quick_pending);
            }
            let invocation = p.accept(&mut w, false, 0..1).unwrap().unwrap();
            assert_eq!(invocation.args["item"], "file");
            p.finish_palette_accept(&mut w, &invocation.command, &invocation.args, true);
            assert!(p.overlay.is_none());
        }
    }
    #[test]
    fn query_selection_clipboard_and_cursor_commands_keep_the_current_match() {
        let (mut w, mut p) = setup("foo foo foo foo");
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"find","text":"foo"}),
        );
        wait(&mut p, &mut w, |p| p.all_matches_ready);
        for _ in 0..3 {
            key(&mut p, &mut w, "enter");
        }
        assert_eq!(w.editor().selection(), 8..11);
        for command in [
            Command::Left(false),
            Command::Right(false),
            Command::SelectAll,
            Command::Copy,
        ] {
            assert!(p.command(&mut w, command, 0..15).unwrap().0);
            p.poll(&mut w, 0..15);
            assert_eq!(w.editor().selection(), 8..11, "{command:?}");
            assert!(p.pending_navigation.is_empty());
            assert!(p.all_matches_ready);
        }
        p.query
            .run("move", &json!({"by":"characters","forward":false}))
            .unwrap();
        p.edited(&mut w, 0..15);
        assert_eq!(w.editor().selection(), 8..11);
        action(
            &mut p,
            &mut w,
            "toggle_find_option",
            json!({"option":"case_sensitive"}),
        );
        wait(&mut p, &mut w, |p| p.all_matches_ready);
        assert_eq!(w.editor().selection(), 0..3);
    }
    #[test]
    fn in_selection_typing_options_and_reopen_preserve_the_scope_and_primary() {
        let (mut w, mut p) = setup("foo\nbar\nfoo\nbaz");
        let original = Selection::from_regions([Region::new(8, 15), Region::new(0, 7)]);
        w.editor_mut().set_selection(original.clone());
        action(&mut p, &mut w, "show_panel", json!({"panel":"find"}));
        action(
            &mut p,
            &mut w,
            "toggle_find_option",
            json!({"option":"in_selection"}),
        );
        let scope = p.selection_scope.clone();
        type_query(&mut p, &mut w, "foo", 0..15);
        action(
            &mut p,
            &mut w,
            "toggle_find_option",
            json!({"option":"case_sensitive"}),
        );
        wait(&mut p, &mut w, |p| p.all_matches_ready);
        assert_eq!(w.editor().buffer.view(w.editor().id).sel(), &original);
        p.panel_focused = false;
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"find","toggle":true}),
        );
        assert_eq!(p.selection_scope, scope);
        assert_eq!(w.editor().buffer.view(w.editor().id).sel(), &original);
    }
    #[test]
    fn selected_text_previews_from_the_primary_begin_in_either_direction() {
        for region in [Region::new(4, 7), Region::new(7, 4)] {
            let (mut w, mut p) = setup("foo foo foo");
            w.editor_mut()
                .set_selection(Selection::from_regions([Region::new(8, 11), region]));
            action(&mut p, &mut w, "show_panel", json!({"panel":"find"}));
            assert_eq!(p.query.value(), "foo");
            type_query(&mut p, &mut w, "fo", 0..11);
            assert_eq!(w.editor().selection(), 4..6);
            type_query(&mut p, &mut w, "foo", 0..11);
            assert_eq!(w.editor().selection(), 4..7);
        }
    }
    #[test]
    fn unchanged_invalid_regex_reports_again_after_feedback_expires() {
        let (mut w, mut p) = setup("foo foo");
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"replace","text":"["}),
        );
        p.focus_input = 1; // Options still update while the Replace input has focus.
        action(
            &mut p,
            &mut w,
            "toggle_find_option",
            json!({"option":"regex"}),
        );
        let error = p.message.clone();
        assert!(error.contains("unclosed character class"));
        for command in [
            "find_next",
            "find_prev",
            "find_all",
            "replace_next",
            "replace_all",
        ] {
            p.expire_message(Instant::now() + Duration::from_secs(6));
            assert!(p.message.is_empty());
            action(&mut p, &mut w, command, json!({}));
            assert_eq!(p.message, error, "{command}");
            assert_eq!(w.editor().text.to_string(), "foo foo");
        }
        for command in ["enter", "shift+enter"] {
            p.expire_message(Instant::now() + Duration::from_secs(6));
            key(&mut p, &mut w, command);
            assert_eq!(p.message, error);
        }
        p.focus_input = 0;
        type_query(&mut p, &mut w, "foo", 0..7);
        assert!(p.find_error.is_none());
    }
    #[test]
    fn message_clip_stops_before_every_panels_first_button_at_each_scale() {
        let (mut w, mut p) = setup("text");
        for scale in [0.75, 1., 1.5, 2.] {
            p.ui_scale = scale;
            for name in ["find", "incremental_find", "replace", "find_in_files"] {
                action(&mut p, &mut w, "show_panel", json!({"panel":name}));
                p.layout(size());
                p.sync_widgets(&w);
                let message = p.panel_message_clip();
                let button = p.tree.nodes[&50].rect;
                // Same row: the message ends before the button; or buttons wrapped below.
                assert!(
                    message.x + message.width <= button.x || message.y + message.height <= button.y,
                    "{name} at {scale}"
                );
                assert!(message.width > 0.);
            }
        }
    }
    /// Every find panel, side bar on/off, UI scale 1/1.5/2, widths 320–1600 and heights
    /// 120–800: controls stay inside the panel (which stays inside the window above the
    /// status bar), never overlap, are hit where drawn, and the message keeps a readable slot.
    #[test]
    fn find_panel_layout_sweep_never_overlaps_and_keeps_controls_inside() {
        let (mut w, mut p) = setup("text");
        let overlap = |a: Rect, b: Rect| {
            let x = (a.x + a.width).min(b.x + b.width) - a.x.max(b.x);
            let y = (a.y + a.height).min(b.y + b.height) - a.y.max(b.y);
            x > 0.01 && y > 0.01
        };
        let inside = |r: Rect, outer: Rect| {
            r.x >= outer.x - 0.01
                && r.y >= outer.y - 0.01
                && r.x + r.width <= outer.x + outer.width + 0.01
                && r.y + r.height <= outer.y + outer.height + 0.01
        };
        let mut layouts = 0;
        for name in ["find", "incremental_find", "replace", "find_in_files"] {
            action(&mut p, &mut w, "show_panel", json!({"panel":name}));
            let panel = p.panel.unwrap();
            let fields = Panels::find_fields(panel);
            for side in [false, true] {
                p.side_visible = side;
                for scale in [1., 1.5, 2.] {
                    p.ui_scale = scale;
                    for width in (272..=1600).step_by(16) {
                        for height in [120., 160., 200., 260., 400., 600., 800.] {
                            let window = Size {
                                width: width as f64,
                                height,
                            };
                            p.layout(window);
                            p.sync_widgets(&w);
                            layouts += 1;
                            let at = format!("{name} side={side} ui={scale} {width}x{height}");
                            let bounds = p.panel_bounds;
                            assert!(bounds.y >= -0.01, "{at}");
                            assert!(
                                bounds.y + bounds.height <= height - STATUS * scale + 0.01,
                                "{at}"
                            );
                            let mut controls: Vec<(u64, Rect)> = (20..20 + fields as u64)
                                .chain(30..30 + FIND_OPTIONS.len() as u64)
                                .chain(50..53)
                                .chain([CLOSE_ID])
                                .filter_map(|id| {
                                    p.tree
                                        .nodes
                                        .get(&id)
                                        .filter(|n| n.visible)
                                        .map(|n| (id, n.rect))
                                })
                                .collect();
                            assert_eq!(
                                controls
                                    .iter()
                                    .filter(
                                        |(id, _)| (30..30 + FIND_OPTIONS.len() as u64).contains(id)
                                    )
                                    .count(),
                                Panels::find_toggle_options(panel).len(),
                                "{at}"
                            );
                            let message = p.panel_message_clip();
                            controls.push((0, message));
                            for (i, (id, rect)) in controls.iter().enumerate() {
                                assert!(inside(*rect, bounds), "{at}: {id} {rect:?} {bounds:?}");
                                for (other, second) in &controls[i + 1..] {
                                    assert!(
                                        !overlap(*rect, *second),
                                        "{at}: {id} {rect:?} overlaps {other} {second:?}"
                                    );
                                }
                                if *id != 0 && rect.width >= 2. && rect.height >= 2. {
                                    let center = Point {
                                        x: rect.x + rect.width / 2.,
                                        y: rect.y + rect.height / 2.,
                                    };
                                    assert_eq!(p.tree.hit_test(center), Some(*id), "{at}");
                                }
                            }
                            let readable = (MESSAGE_MIN * scale).min(bounds.width - 16. * scale);
                            assert!(message.width >= readable - 0.01, "{at}: {message:?}");
                            // A slot too short for the text hands the message to the status bar.
                            assert!(p.message_in_find_panel() || p.chrome, "{at}");
                            if p.message_in_find_panel() {
                                assert!(message.height >= 16. * scale - 0.01, "{at}");
                            }
                            // Inputs keep a usable width: at least 120 DIPs, or all the panel
                            // has once labels are dropped.
                            for id in 20..20 + fields as u64 {
                                let input = p.tree.nodes[&id].rect;
                                let usable = (120. * scale).min(bounds.width - 48. * scale);
                                assert!(input.width >= usable - 0.01, "{at}: input {input:?}");
                            }
                        }
                    }
                }
            }
        }
        assert!(layouts > 10_000);
    }
    #[test]
    fn file_search_survives_other_panels_and_build_source_switching() {
        let (mut w, mut p) = setup(&"foo\n".repeat(500));
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"find_in_files","text":"foo"}),
        );
        p.where_input.set("<open files>");
        action(&mut p, &mut w, "find_in_files_start", json!({}));
        assert!(p.file_task.is_some());
        action(&mut p, &mut w, "show_panel", json!({"panel":"find"}));
        assert!(p.file_task.is_some());
        action(
            &mut p,
            &mut w,
            "show_input_panel",
            json!({"caption":"/","initial_text":""}),
        );
        assert!(p.file_task.is_some());
        action(&mut p, &mut w, "hide_panel", json!({}));
        assert!(p.file_task.is_some());
        p.select_build_results();
        assert!(p.file_task.is_some());
        wait(&mut p, &mut w, |p| p.file_complete);
        assert_eq!(p.result_status, "500 matches in 1 file");
        assert!(!p.search_results_active);
        p.select_build_results();
        assert!(p.file_complete);
        p.query.set("missing");
        action(&mut p, &mut w, "find_in_files_start", json!({}));
        assert!(p.search_results_active);
        wait(&mut p, &mut w, |p| p.file_complete);
        assert_eq!(p.result_status, "0 matches in 0 files");
        assert!(!p.has_search_results());
    }

    #[test]
    fn replace_enter_and_shift_enter_find_without_editing() {
        let (mut w, mut p) = setup("cat cat cat");
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"replace","text":"cat"}),
        );
        p.replace.set("dog");
        wait(&mut p, &mut w, |p| p.all_matches_ready);
        key(&mut p, &mut w, "enter");
        assert_eq!(w.editor().selection(), 0..3);
        key(&mut p, &mut w, "enter");
        assert_eq!(w.editor().selection(), 4..7);
        key(&mut p, &mut w, "shift+enter");
        assert_eq!(w.editor().selection(), 0..3);
        assert_eq!(w.editor().text.to_string(), "cat cat cat");
        key(&mut p, &mut w, "super+alt+e");
        assert_eq!(w.editor().text.to_string(), "dog cat cat");
        assert_eq!(p.panel, Some(Panel::Replace));
    }
    #[test]
    fn incremental_enter_commits_and_cancel_restores_selection_and_scroll() {
        let (mut w, mut p) = setup("cat\nx\ncat\ny\ncat");
        w.editor_mut().set_selection(Selection::from_regions([
            Region::new(6, 7),
            Region::caret(3),
        ]));
        let original = w.editor().buffer.view(w.editor().id).sel().clone();
        w.tabs[w.active].scroll = (12., 240.);
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"incremental_find"}),
        );
        type_query(&mut p, &mut w, "cat", 0..3);
        assert_eq!(w.editor().selection(), 6..9);
        assert!(p.reveal);
        assert!(p.find_history.is_empty());
        action(&mut p, &mut w, "hide_panel", json!({"cancel":true}));
        assert_eq!(
            w.editor()
                .buffer
                .view(w.editor().id)
                .sel()
                .iter()
                .collect::<Vec<_>>(),
            original.iter().collect::<Vec<_>>()
        );
        assert_eq!(p.restore_scroll, Some((12., 240.)));
        assert!(!p.reveal);
        assert!(!p.should_reveal(true));
        assert!(p.should_reveal(true));
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"incremental_find","text":"cat"}),
        );
        type_query(&mut p, &mut w, "c", 0..3);
        type_query(&mut p, &mut w, "cat", 0..3);
        assert_eq!(w.editor().selection(), 6..9);
        key(&mut p, &mut w, "enter");
        wait(&mut p, &mut w, |p| p.panel.is_none());
        assert_eq!(w.editor().selection(), 12..15);
        assert_eq!(p.find_history, ["cat"]);
    }
    #[test]
    fn incremental_escape_invalidates_pending_preview() {
        let (mut w, mut p) = setup("cat cat");
        w.editor_mut().set_caret(4, false);
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"incremental_find"}),
        );
        p.query.set("cat");
        p.input_changed(&mut w, 0..3);
        key(&mut p, &mut w, "escape");
        wait(&mut p, &mut w, |p| p.all_matches_ready);
        assert_eq!(w.editor().selection(), 4..4);
        assert!(p.panel.is_none());
    }
    #[test]
    fn typing_find_uses_opening_caret_wraps_and_requests_reveal() {
        for panel in ["find", "incremental_find"] {
            let (mut w, mut p) = setup("cat\ncat\ncat");
            w.editor_mut().set_caret(5, false);
            action(&mut p, &mut w, "show_panel", json!({"panel":panel}));
            type_query(&mut p, &mut w, "c", 0..3);
            assert_eq!(w.editor().selection(), 8..9);
            type_query(&mut p, &mut w, "cat", 0..3);
            assert_eq!(w.editor().selection(), 8..11);
            assert!(p.reveal);
            assert!(p.find_history.is_empty());
            action(&mut p, &mut w, "hide_panel", json!({}));
            w.editor_mut().set_caret(11, false);
            action(&mut p, &mut w, "show_panel", json!({"panel":panel}));
            type_query(&mut p, &mut w, "c", 8..11);
            type_query(&mut p, &mut w, "cat", 8..11);
            assert_eq!(w.editor().selection(), 0..3);
        }
    }
    #[test]
    fn reopen_selects_fields_and_editor_focus_shortcut_focuses_before_toggling() {
        let (mut w, mut p) = setup("cat cat");
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"find","text":"old"}),
        );
        p.panel_focused = false;
        key(&mut p, &mut w, "super+f");
        assert_eq!(p.panel, Some(Panel::Find));
        assert!(p.panel_focused);
        assert_eq!(
            p.query
                .buffer
                .view(p.query.view)
                .sel()
                .primary()
                .unwrap()
                .to_range(),
            0..3
        );
        p.text_input(
            &mut w,
            TextInput::Insert {
                text: "cat".into(),
                replacement: None,
            },
            0..7,
        );
        assert_eq!(p.query.value(), "cat");
        key(&mut p, &mut w, "super+f");
        assert!(p.panel.is_none());
        p.replace.set("replacement");
        key(&mut p, &mut w, "super+alt+f");
        assert_eq!(
            p.replace
                .buffer
                .view(p.replace.view)
                .sel()
                .primary()
                .unwrap()
                .to_range(),
            0..11
        );
    }
    #[test]
    fn histories_record_only_commits_and_arrows_restore_drafts() {
        let (mut w, mut p) = setup("cat cat");
        w.editor_mut().settings.set(
            vel_config::settings::Layer::View,
            "close_find_after_replace_all",
            json!(false),
        );
        action(&mut p, &mut w, "show_panel", json!({"panel":"replace"}));
        for query in ["c", "ca"] {
            type_query(&mut p, &mut w, query, 0..7);
            let mut window = vel_session::Window::default();
            p.capture_session(&mut window);
            assert!(window.find_history.is_empty());
        }
        p.replace.set("d");
        type_query(&mut p, &mut w, "cat", 0..7);
        action(&mut p, &mut w, "find_next", json!({}));
        assert_eq!(p.find_history, ["cat"]);
        assert!(p.replace_history.is_empty());
        p.replace.set("dog");
        action(&mut p, &mut w, "replace_all", json!({}));
        assert_eq!(p.replace_history, ["dog"]);
        p.find_history.insert(0, "older".into());
        p.query.set("draft");
        key(&mut p, &mut w, "up");
        assert_eq!(p.query.value(), "cat");
        key(&mut p, &mut w, "up");
        assert_eq!(p.query.value(), "older");
        key(&mut p, &mut w, "down");
        assert_eq!(p.query.value(), "cat");
        key(&mut p, &mut w, "down");
        assert_eq!(p.query.value(), "draft");
        key(&mut p, &mut w, "tab");
        p.replace.set("replacement draft");
        p.command(&mut w, Command::Up(false), 0..7).unwrap();
        assert_eq!(p.replace.value(), "dog");
        p.command(&mut w, Command::Down(false), 0..7).unwrap();
        assert_eq!(p.replace.value(), "replacement draft");
    }
    #[test]
    fn feedback_survives_poll_and_expires_without_regex_modal_errors() {
        let (mut w, mut p) = setup("cat cat");
        w.editor_mut().settings.set(
            vel_config::settings::Layer::View,
            "close_find_after_replace_all",
            json!(false),
        );
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"replace","text":"cat"}),
        );
        p.replace.set("dog");
        action(&mut p, &mut w, "replace_all", json!({}));
        wait(&mut p, &mut w, |p| p.all_matches_ready);
        assert_eq!(p.message, "Replaced 2 occurrences");
        action(&mut p, &mut w, "find_next", json!({}));
        assert_eq!(p.message, "No matches");
        p.options.regex = true;
        p.query.set("[");
        action(&mut p, &mut w, "replace_all", json!({}));
        assert!(!p.message.contains("(?m)"));
        assert!(p.message.contains("unclosed"));
        action(&mut p, &mut w, "find_in_files_start", json!({}));
        assert_eq!(p.panel, Some(Panel::Replace));
        assert!(p.file_task.is_none());
        assert!(p.expire_message(Instant::now() + Duration::from_secs(6)));
        assert!(p.message.is_empty());
    }
    #[test]
    fn replacing_the_final_match_retains_feedback_after_navigation_completes() {
        let (mut w, mut p) = setup("cat");
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"replace","text":"cat"}),
        );
        wait(&mut p, &mut w, |p| p.all_matches_ready);
        action(&mut p, &mut w, "find_next", json!({}));
        p.replace.set("dog");
        action(&mut p, &mut w, "replace_next", json!({}));
        wait(&mut p, &mut w, |p| p.all_matches_ready);
        assert_eq!(p.message, "Replaced 1 occurrence");
        assert_eq!(w.editor().text.to_string(), "dog");
    }
    #[test]
    fn panel_buttons_and_files_alt_enter_do_not_search_the_buffer() {
        let (mut w, mut p) = setup("cat cat");
        for (panel, labels) in [
            ("find", vec!["Find", "Find Prev", "Find All"]),
            ("replace", vec!["Find", "Replace", "Replace All"]),
            ("find_in_files", vec!["Find", "Replace"]),
        ] {
            action(
                &mut p,
                &mut w,
                "show_panel",
                json!({"panel":panel,"text":"cat"}),
            );
            p.layout(size());
            p.paint(&w, &mut Scene::default());
            assert_eq!(
                (50..50 + labels.len() as u64)
                    .map(|i| p.tree.nodes[&i].label.as_str())
                    .collect::<Vec<_>>(),
                labels
            );
        }
        let before = w.editor().buffer.view(w.editor().id).sel().clone();
        key(&mut p, &mut w, "alt+enter");
        assert_eq!(w.editor().buffer.view(w.editor().id).sel(), &before);
        assert_eq!(p.panel, Some(Panel::Files));
        let rect = p.tree.nodes[&50].rect;
        p.where_input.set("<current file>");
        p.mouse(
            &mut w,
            Point {
                x: rect.x + 4.,
                y: rect.y + 4.,
            },
            true,
            false,
            false,
            0..7,
        )
        .unwrap();
        wait(&mut p, &mut w, |p| p.file_complete);
        assert_eq!(p.result_status, "2 matches in 1 file");
    }
    #[test]
    fn file_progress_summary_navigation_and_close_cancel() {
        let (mut w, mut p) = setup("cat cat\ncontext\ncat");
        w.new_file().unwrap();
        w.editor_mut()
            .run("insert", &json!({"characters":"no match"}))
            .unwrap();
        w.select(0);
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"find_in_files","text":"cat"}),
        );
        p.where_input.set("<open files>");
        key(&mut p, &mut w, "enter");
        assert_eq!(p.result_status, "Searching 0 files…");
        wait(&mut p, &mut w, |p| p.file_complete);
        assert_eq!(p.files_searched, 2);
        assert_eq!(p.result_status, "3 matches in 1 file");
        assert_eq!(p.message, p.result_status);
        assert!(p.has_search_results());
        p.navigate_result(&mut w, true).unwrap();
        assert_eq!(w.editor().caret, 0);
        p.navigate_result(&mut w, true).unwrap();
        assert_eq!(w.editor().caret, 16);
        p.navigate_result(&mut w, false).unwrap();
        assert_eq!(w.editor().caret, 0);
        action(&mut p, &mut w, "find_in_files_start", json!({}));
        key(&mut p, &mut w, "escape");
        assert!(p.file_task.is_none());
        assert!(!p.file_complete);
        assert!(p.message.contains("cancelled"));
        p.poll(&mut w, 0..19);
        assert!(p.panel.is_none());
    }
    #[test]
    fn current_file_scope_uses_the_active_unsaved_buffer() {
        let (mut w, mut p) = setup("cat");
        w.new_file().unwrap();
        w.editor_mut()
            .run("insert", &json!({"characters":"cat cat"}))
            .unwrap();
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"find_in_files","text":"cat"}),
        );
        p.where_input.set("<current file>");
        key(&mut p, &mut w, "enter");
        wait(&mut p, &mut w, |p| p.file_complete);
        assert_eq!((p.file_matches, p.files_searched), (2, 1));
        p.navigate_result(&mut w, true).unwrap();
        assert_eq!(w.active, 1);
    }
    #[test]
    fn early_enter_opens_the_best_current_fuzzy_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let exact = dir.path().join("readme");
        let other = dir.path().join("really-distant-name");
        std::fs::write(&exact, "exact").unwrap();
        std::fs::write(&other, "other").unwrap();
        let (mut w, mut p) = setup("original");
        p.file_paths = Arc::new(vec![other.clone(), exact.clone()]);
        p.file_candidates = Arc::new(
            p.file_paths
                .iter()
                .map(|p| fuzzy::Candidate::new(p.to_string_lossy()))
                .collect(),
        );
        action(&mut p, &mut w, "show_overlay", json!({"text":"readme"}));
        assert!(p.quick_pending);
        key(&mut p, &mut w, "enter");
        assert!(p.overlay.is_none());
        assert_eq!(
            w.tabs[w.active].path.as_ref(),
            Some(&exact.canonicalize().unwrap())
        );
        assert!(!w.tabs[w.active].transient);
    }
    #[test]
    fn overlay_mouse_accepts_single_click_and_outside_click_restores_preview() {
        let (mut w, mut p) = setup("original");
        p.choices(
            &mut w,
            vec![vel_config::ui::PaletteItem {
                caption: "Indent Using Tabs".into(),
                invocation: Invocation {
                    command: "set_indentation".into(),
                    args: json!({"spaces":false}),
                },
            }],
        );
        wait(&mut p, &mut w, |p| !p.quick_pending);
        p.layout(size());
        p.mouse(
            &mut w,
            Point {
                x: p.quick_bounds.x + 20.,
                y: p.quick_bounds.y + 50.,
            },
            true,
            false,
            false,
            0..8,
        )
        .unwrap();
        assert!(p.overlay.is_none());
        assert_eq!(p.pending_command.take().unwrap().command, "set_indentation");
        w.editor_mut().set_caret(4, false);
        action(&mut p, &mut w, "show_overlay", json!({"text":":1:2"}));
        assert_eq!(w.editor().caret, 1);
        p.layout(size());
        p.mouse(&mut w, Point { x: 1., y: 1. }, true, false, false, 0..8)
            .unwrap();
        assert!(p.overlay.is_none());
        assert_eq!(w.editor().caret, 4);
    }
    #[test]
    fn every_quick_panel_paints_a_localized_footer_with_mac_key_glyphs() {
        let (mut w, mut p) = setup("text");
        p.message = "stale find message".into();
        for (overlays, en, zh) in [
            (
                &[Overlay::Palette][..],
                "↩ Run · ⎋ Cancel",
                "↩ 运行 · ⎋ 取消",
            ),
            (
                &[Overlay::Appearance][..],
                "↑↓ Preview · ↩ Save · ⎋ Cancel",
                "↑↓ 预览 · ↩ 保存 · ⎋ 取消",
            ),
            (
                &[Overlay::Syntax, Overlay::Choices][..],
                "↩ Select · ⎋ Cancel",
                "↩ 选择 · ⎋ 取消",
            ),
            (
                &[Overlay::ProjectSymbols, Overlay::Locations][..],
                "↩ Jump · ⎋ Cancel",
                "↩ 跳转 · ⎋ 取消",
            ),
            (
                &[Overlay::Goto][..],
                "↩ Open · ⌘↩ / ⌃↩ Open in Split · ⎋ Cancel",
                "↩ 打开 · ⌘↩ / ⌃↩ 在拆分视图中打开 · ⎋ 取消",
            ),
        ] {
            for &overlay in overlays {
                p.overlay = Some(overlay);
                p.layout(size());
                for (language, expected) in [("en", en), ("zh-Hans", zh)] {
                    w.settings.set(
                        vel_config::settings::Layer::User,
                        "ui_language",
                        json!(language),
                    );
                    w.sync_ui_language();
                    let mut scene = Scene::default();
                    p.paint(&w, &mut scene);
                    assert!(
                        scene
                            .layers
                            .iter()
                            .flat_map(|layer| &layer.text)
                            .any(|run| run.text == expected),
                        "{overlay:?} {language:?}: {expected}"
                    );
                }
            }
        }
    }

    #[test]
    fn overlay_and_sidebar_geometry_stay_aligned_across_scales() {
        let (w, mut p) = setup("text");
        p.overlay = Some(Overlay::Locations);
        for scale in [0.75, 1., 1.5, 2.] {
            p.ui_scale = scale;
            p.layout(size());
            assert!((p.quick_bounds.x + p.quick_bounds.width / 2. - 600.).abs() < 0.001);
            p.side_rows = vec![SideRow {
                path: "/root/child".into(),
                folder: 0,
                depth: 1,
                directory: true,
            }];
            let point = Point {
                x: 10.,
                y: p.side_bounds.y + (w.tabs.len() + 2) as f64 * 24. * scale + 10. * scale,
            };
            assert_eq!(
                p.review_side_path(&w, point),
                Some((PathBuf::from("/root/child"), true))
            );
        }
    }
    #[test]
    fn indentation_status_click_switches_width_and_converts_in_one_undo() {
        let (mut w, mut p) = setup("\t  code\n  \tmore\n");
        p.layout(size());
        // Status items are hit where the last paint placed them.
        p.paint(&w, &mut vel_render::scene::Scene::default());
        let rect = p.indentation_rect();
        assert!(rect.width > 0., "indentation item painted");
        p.mouse(
            &mut w,
            Point {
                x: rect.x + 5.,
                y: rect.y + 5.,
            },
            true,
            false,
            false,
            0..18,
        )
        .unwrap();
        assert_eq!(p.overlay, Some(Overlay::Choices));
        assert!(p.choice_items.iter().any(|i| i.caption == "Tab Width: 8"));
        action(
            &mut p,
            &mut w,
            "set_indentation",
            json!({"spaces":true,"width":4,"convert":true}),
        );
        assert_eq!(w.editor().text.to_string(), "      code\n    more\n");
        w.editor_mut().run("undo", &json!({})).unwrap();
        assert_eq!(w.editor().text.to_string(), "\t  code\n  \tmore\n");
        action(
            &mut p,
            &mut w,
            "set_indentation",
            json!({"spaces":false,"width":2,"convert":true}),
        );
        assert_eq!(w.editor().text.to_string(), "\t\tcode\n\t\tmore\n");
    }
    #[test]
    fn sidebar_focus_and_escape_preserve_visible_find_panel() {
        let (mut w, mut p) = setup("cat");
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"find","text":"cat"}),
        );
        action(&mut p, &mut w, "focus_side_bar", json!({}));
        assert_eq!(p.panel, Some(Panel::Find));
        assert!(p.side_focused);
        assert!(!p.panel_focused);
        key(&mut p, &mut w, "escape");
        assert_eq!(p.panel, Some(Panel::Find));
        assert!(!p.side_focused);
        assert!(p.input().is_none());
        key(&mut p, &mut w, "super+f");
        assert!(p.panel_focused);
        assert_eq!(p.panel, Some(Panel::Find));
        p.layout(size());
        let point = Point {
            x: 10.,
            y: p.side_bounds.y + 30.,
        };
        p.mouse(&mut w, point, true, false, false, 0..3).unwrap();
        assert!(p.side_focused);
        assert!(!p.panel_focused);
        key(&mut p, &mut w, "escape");
        assert_eq!(p.panel, Some(Panel::Find));
    }
    #[test]
    fn slurp_and_find_under_use_the_primary_without_collapsing_other_carets() {
        let (mut w, mut p) = setup("first second first");
        w.editor_mut().set_selection(Selection::from_regions([
            Region::caret(10),
            Region::caret(5),
        ]));
        action(&mut p, &mut w, "slurp_find_string", json!({}));
        assert_eq!(p.query.value(), "first");
        assert_eq!(w.editor().regions().len(), 2);
        assert_eq!(w.editor().selection(), 0..5);
        action(&mut p, &mut w, "find_under", json!({}));
        wait(&mut p, &mut w, |p| p.all_matches_ready);
        assert_eq!(w.editor().selection(), 13..18);
        w.editor_mut().set_selection(Selection::from_regions([
            Region::caret(10),
            Region::caret(5),
        ]));
        w.editor_mut().run("find_all_under", &json!({})).unwrap();
        assert_eq!(
            w.editor()
                .regions()
                .iter()
                .map(|r| r.to_range())
                .collect::<Vec<_>>(),
            [0..5, 13..18]
        );
    }
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    #[test]
    fn symbol_poll_keeps_worker_results_and_refreshes_after_an_edit() {
        let mut w = Workspace::new(
            vel_text::Text::from("fn alpha() {}\nfn beta() {}\n"),
            None,
            None,
            false,
        )
        .unwrap();
        w.set_file_type("Rust").unwrap();
        let mut p = Panels::new(&w, false);
        let until = Instant::now() + Duration::from_secs(10);
        while !w
            .editor()
            .language
            .borrow()
            .as_ref()
            .is_some_and(|l| l.ready())
        {
            w.poll();
            assert!(Instant::now() < until);
            std::thread::park_timeout(Duration::from_millis(1));
        }
        p.action(&mut w, "show_overlay", &json!({"text":"@"}), 0..26)
            .unwrap();
        loop {
            p.poll(&mut w, 0..26);
            if !p.quick_pending && p.quick.items.len() == 2 {
                break;
            }
            assert!(Instant::now() < until);
            std::thread::park_timeout(Duration::from_millis(1));
        }
        p.quick.navigate(1);
        let selected = p.quick.items[p.quick.selected].caption.clone();
        for _ in 0..25 {
            p.poll(&mut w, 0..26);
            assert!(!p.quick_pending);
            assert_eq!(p.quick.items[p.quick.selected].caption, selected);
        }
        let end = w.editor().text.len_bytes();
        w.editor_mut().set_caret(end, false);
        w.editor_mut()
            .run("insert", &json!({"characters":"fn gamma() {}\n"}))
            .unwrap();
        loop {
            w.poll();
            p.poll(&mut w, 0..40);
            if !p.quick_pending && p.quick.items.len() == 3 {
                break;
            }
            assert!(Instant::now() < until);
            std::thread::park_timeout(Duration::from_millis(1));
        }
        assert_eq!(p.quick.items[p.quick.selected].caption, selected);
    }
    #[test]
    fn actual_status_scene_shows_defaults_and_can_hide_formats() {
        let mut w = Workspace::new(vel_text::Text::from("你好"), None, None, false).unwrap();
        let mut p = Panels::new(&w, false);
        p.layout(Size {
            width: 1200.,
            height: 800.,
        });
        let text = |p: &mut Panels, w: &Workspace| {
            let mut scene = Scene::default();
            p.paint(w, &mut scene);
            scene
                .layers
                .iter()
                .flat_map(|l| &l.text)
                .filter(|t| t.baseline >= 780.)
                .map(|t| t.text.clone())
                .collect::<Vec<_>>()
                .join(" ")
        };
        let initial = text(&mut p, &w);
        assert!(initial.contains("Ln 1, Col 1"));
        assert!(!initial.contains("selected"));
        assert!(initial.contains("UTF-8"));
        assert!(initial.contains("Unix"));
        assert!(
            initial.contains(&vel_ui::status::indentation(
                w.editor().preferences.translate_tabs_to_spaces,
                w.editor().preferences.tab_size
            )),
            "{initial}"
        );
        w.editor_mut()
            .set_selection(Selection::from_regions([Region::new(0, 6)]));
        w.editor_mut().settings.set(
            vel_config::settings::Layer::View,
            "show_encoding",
            json!(false),
        );
        w.editor_mut().settings.set(
            vel_config::settings::Layer::View,
            "show_line_endings",
            json!(false),
        );
        let selected = text(&mut p, &w);
        assert!(selected.contains("2 characters selected"));
        assert!(!selected.contains("UTF-8"));
        assert!(!selected.contains(w.tabs[w.active].format.line_ending.display_name()));
        p.set_message("Unable to find missing");
        assert!(text(&mut p, &w).contains("Unable to find missing"));
    }
}
