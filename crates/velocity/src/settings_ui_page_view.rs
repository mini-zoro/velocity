//! Curated Settings pages: bold section headings with their controls indented below,
//! native-looking checkboxes, popups, radios, steppers and push buttons, "?" help, and live
//! previews of the editor font and indentation.
use super::{
    Entry, Focus, Ink, Kind, Page, Target, boxed, checkbox, focus_ring, mix, paint_chips,
    popup_button, push_button, radio, rect, rounded, text,
};
use vel_config::Value;
use vel_i18n::Translator;
use vel_platform::Rect;
use vel_render::scene::Layer;
use vel_ui::{minihtml::intersection, theme::Style, theme::ThemeUi};

/// Sample lines of the font preview. They are data, not interface text.
pub(in super::super) const FONT_SAMPLE: [&str; 2] = [
    "fn main() { println!(\"Hello, 世界\"); }",
    "0O 1lI {} [] () => != <= 你好，世界",
];
/// Lines of the indentation preview, by indentation level.
pub(in super::super) const INDENT_SAMPLE: [(usize, &str); 4] = [
    (0, "if ready {"),
    (1, "for task in queue {"),
    (2, "run(task);"),
    (1, "}"),
];

enum Line {
    Entry(usize),
    Buttons(Vec<usize>),
}

impl Page {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn paint_page(
        &mut self,
        theme: &ThemeUi,
        tr: &Translator,
        ink: &Ink,
        styles: [&Style; 3],
        [layer, controls, fields]: [&mut Layer; 3],
        content: Rect,
        scale: f64,
        focused: bool,
    ) {
        let super::Pane::Page(page) = self.pane else {
            return;
        };
        let entries = std::mem::take(&mut self.entries[page]);
        self.page_bounds = content;
        let pad_left = 30. * scale;
        let pad_right = 22. * scale;
        let inner = rect(
            content.x + pad_left,
            content.y,
            (content.width - pad_left - pad_right).max(0.),
            content.height,
        );
        let indent = 16. * scale;
        let sub = 24. * scale;
        let help = 18. * scale;
        let right = inner.x + inner.width;
        // One label column per page, so popups and fields line up like a form.
        let label_col = entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Setting {
                    kind,
                    label,
                    indent: nested,
                    ..
                } if *kind != Kind::Check => {
                    Some(theme.advance(&ink.normal, label) + if *nested { sub } else { 0. })
                }
                _ => None,
            })
            .fold(0., f64::max)
            .max(96. * scale)
            + 20. * scale;
        let label_col = label_col.min(inner.width * 0.46);
        let control_x = inner.x + indent + label_col;
        let control_max = (right - help - 10. * scale - control_x).max(60. * scale);

        // Layout.
        let mut lines = Vec::new();
        let mut y = 22. * scale;
        let mut first = true;
        let mut i = 0;
        while i < entries.len() {
            match &entries[i] {
                Entry::Heading { .. } => {
                    if !first {
                        y += 20. * scale;
                    }
                    first = false;
                    lines.push((Line::Entry(i), y, 22. * scale));
                    y += 28. * scale;
                }
                Entry::Setting { kind, .. } => {
                    let h = match kind {
                        Kind::Check => 28.,
                        Kind::Tags => 60.,
                        _ => 34.,
                    } * scale;
                    lines.push((Line::Entry(i), y, h));
                    y += h;
                }
                Entry::Button { .. } => {
                    let mut ids = Vec::new();
                    while let Some(Entry::Button { .. }) = entries.get(i) {
                        ids.push(i);
                        i += 1;
                    }
                    lines.push((Line::Buttons(ids), y + 2. * scale, 30. * scale));
                    y += 38. * scale;
                    continue;
                }
                Entry::Preview(preview) => {
                    let h = self.preview_height(*preview, scale);
                    lines.push((Line::Entry(i), y + 4. * scale, h));
                    y += h + 14. * scale;
                }
            }
            i += 1;
        }
        self.page_extent = y + 28. * scale;
        let viewport = content.height;
        if self.reveal {
            let wanted = lines.iter().find(|(line, _, _)| match line {
                Line::Entry(i) => matches!(
                    &entries[*i],
                    Entry::Setting { row, .. }
                        if self.focus == Focus::List && self.selected_row() == Some(*row)
                ),
                Line::Buttons(ids) => ids.iter().any(|i| {
                    matches!(&entries[*i], Entry::Button { id, .. }
                        if self.focus == Focus::Action(Target::Button(*id)))
                }),
            });
            if let Some(&(_, top, h)) = wanted {
                let margin = 12. * scale;
                if top - margin < self.page_offset {
                    self.page_offset = top - margin;
                } else if top + h + margin > self.page_offset + viewport {
                    self.page_offset = top + h + margin - viewport;
                }
            }
            self.reveal = false;
        }
        self.page_offset = self
            .page_offset
            .clamp(0., (self.page_extent - viewport).max(0.));
        let offset = self.page_offset;
        let writable = self.writable();

        for (line, top, h) in &lines {
            let sy = content.y + top - offset;
            if sy + h < content.y || sy > content.y + content.height {
                continue;
            }
            match line {
                Line::Buttons(ids) => {
                    let mut bx = inner.x + indent;
                    for &i in ids {
                        let Entry::Button { id, caption } = &entries[i] else {
                            continue;
                        };
                        let w = theme.advance(&ink.normal, caption) + 30. * scale;
                        if bx + w > right && bx > inner.x + indent {
                            break;
                        }
                        let r = rect(bx, sy, w.min(right - bx), *h);
                        let target = Target::Button(*id);
                        push_button(
                            theme,
                            controls,
                            ink,
                            caption,
                            r,
                            content,
                            true,
                            focused && self.focus == Focus::Action(target),
                        );
                        self.hits.push((intersection(content, r), target));
                        bx += w + 10. * scale;
                    }
                }
                Line::Entry(i) => match &entries[*i] {
                    Entry::Heading { title, hint } => {
                        let r = rect(inner.x, sy, inner.width, *h);
                        let title_w = theme.advance(&ink.strong, title).min(inner.width);
                        text(theme, layer, &ink.strong, title, r, content);
                        if let Some(hint) = hint {
                            let x = inner.x + title_w + 10. * scale;
                            text(
                                theme,
                                layer,
                                &ink.link,
                                hint,
                                rect(x, sy, (right - x).max(0.), *h),
                                content,
                            );
                        }
                        self.headings.push((
                            intersection(content, rect(inner.x, sy, title_w, *h)),
                            title.clone(),
                        ));
                    }
                    Entry::Preview(preview) => {
                        let x = inner.x + indent;
                        let r = rect(x, sy, (right - x).max(0.), *h);
                        self.paint_preview(theme, ink, *preview, controls, r, content, scale);
                    }
                    Entry::Setting {
                        row,
                        label,
                        kind,
                        indent: nested,
                        enabled,
                        hint,
                        choices,
                        ..
                    } => {
                        let row = *row;
                        let x = inner.x + indent + if *nested { sub } else { 0. };
                        let entry = &self.rows[row];
                        let enabled = *enabled && writable && entry.editable();
                        let conflict = self.conflicts.contains_key(&row)
                            || self
                                .edit
                                .as_ref()
                                .is_some_and(|edit| edit.row == row && edit.conflict);
                        let label_style = if enabled { &ink.normal } else { &ink.muted };
                        let control_focused = focused
                            && self.focus == Focus::List
                            && self.selected_row() == Some(row)
                            && self.popup.is_none();
                        let line_h = 28f64.min(*h / scale) * scale;
                        let help_r = rect(right - help, sy + (line_h - help) / 2. + 1., help, help);
                        match kind {
                            Kind::Check => {
                                let size = 15. * scale;
                                let check = rect(x, sy + (line_h - size) / 2., size, size);
                                checkbox(
                                    theme,
                                    controls,
                                    ink,
                                    check,
                                    content,
                                    entry.value.as_bool().unwrap_or(false),
                                    enabled,
                                );
                                if control_focused {
                                    focus_ring(controls, ink, check, content, 4.);
                                }
                                let lx = check.x + size + 8. * scale;
                                let room = (help_r.x - 10. * scale - lx).max(0.);
                                let label_w = theme.advance(label_style, label).min(room);
                                text(
                                    theme,
                                    layer,
                                    label_style,
                                    label,
                                    rect(lx, sy, label_w + 1., line_h),
                                    content,
                                );
                                if let Some(hint) = hint {
                                    let hx = lx + label_w + 10. * scale;
                                    if help_r.x - hx > 40. * scale {
                                        text(
                                            theme,
                                            layer,
                                            if enabled { &ink.link } else { &ink.muted },
                                            hint,
                                            rect(hx, sy, help_r.x - 8. * scale - hx, line_h),
                                            content,
                                        );
                                    }
                                }
                                self.hits.push((
                                    intersection(
                                        content,
                                        rect(x - 4., sy, lx + label_w + 4. - x, line_h),
                                    ),
                                    Target::Control(row),
                                ));
                            }
                            _ => {
                                let label_w = (control_x - 12. * scale - x).max(0.);
                                text(
                                    theme,
                                    layer,
                                    label_style,
                                    label,
                                    rect(x, sy, label_w, line_h.max(30. * scale)),
                                    content,
                                );
                                if let Some(hint) = hint {
                                    self.tooltips.push((
                                        intersection(content, rect(x, sy, label_w, line_h)),
                                        hint.clone(),
                                    ));
                                }
                                let field_h = 24. * scale;
                                let cy = sy + (30. * scale - field_h) / 2.;
                                match kind {
                                    Kind::Popup => {
                                        let r = rect(
                                            control_x,
                                            cy,
                                            control_max.min(280. * scale),
                                            field_h,
                                        );
                                        popup_button(
                                            theme,
                                            controls,
                                            ink,
                                            &entry.choice_label(&entry.value, tr),
                                            r,
                                            content,
                                            enabled,
                                            control_focused,
                                        );
                                        if enabled {
                                            self.hits.push((
                                                intersection(content, r),
                                                Target::Control(row),
                                            ));
                                        }
                                    }
                                    Kind::Radio => {
                                        let size = 15. * scale;
                                        let mut ox = control_x;
                                        for (j, choice) in choices.iter().enumerate() {
                                            let caption = entry.choice_label(choice, tr);
                                            let w = size
                                                + 7. * scale
                                                + theme.advance(label_style, &caption);
                                            if ox + w > right - help && j > 0 {
                                                break;
                                            }
                                            let dot =
                                                rect(ox, cy + (field_h - size) / 2., size, size);
                                            let chosen = *choice == entry.value;
                                            radio(controls, ink, dot, content, chosen, enabled);
                                            if chosen && control_focused {
                                                focus_ring(
                                                    controls,
                                                    ink,
                                                    dot,
                                                    content,
                                                    (size / 2.) as f32,
                                                );
                                            }
                                            text(
                                                theme,
                                                layer,
                                                label_style,
                                                &caption,
                                                rect(
                                                    dot.x + size + 7. * scale,
                                                    cy,
                                                    w - size - 6. * scale,
                                                    field_h,
                                                ),
                                                content,
                                            );
                                            if enabled {
                                                self.hits.push((
                                                    intersection(
                                                        content,
                                                        rect(ox - 3., cy, w + 6., field_h),
                                                    ),
                                                    Target::Choice(row, j),
                                                ));
                                            }
                                            ox += w + 22. * scale;
                                        }
                                    }
                                    Kind::Stepper | Kind::Field => {
                                        let width = if *kind == Kind::Stepper {
                                            84. * scale
                                        } else {
                                            control_max.min(280. * scale)
                                        };
                                        let field =
                                            rect(control_x, cy, width.min(control_max), field_h);
                                        self.paint_field(
                                            theme,
                                            tr,
                                            ink,
                                            styles,
                                            controls,
                                            fields,
                                            row,
                                            field,
                                            content,
                                            scale,
                                            focused,
                                            enabled && !conflict,
                                            "",
                                        );
                                        if control_focused {
                                            focus_ring(controls, ink, field, content, 5.);
                                        }
                                        if *kind == Kind::Stepper {
                                            let stepper = rect(
                                                field.x + field.width + 5. * scale,
                                                cy,
                                                17. * scale,
                                                field_h,
                                            );
                                            boxed(
                                                controls,
                                                stepper,
                                                content,
                                                ink.control,
                                                ink.border,
                                                5.,
                                            );
                                            let icon = 13. * scale;
                                            theme.icon(
                                                controls,
                                                "Theme - Velocity/updown.png",
                                                rect(
                                                    stepper.x + (stepper.width - icon) / 2.,
                                                    stepper.y + (stepper.height - icon) / 2.,
                                                    icon,
                                                    icon,
                                                ),
                                                content,
                                                label_style.foreground,
                                            );
                                            if enabled && !conflict {
                                                let half = stepper.height / 2.;
                                                self.hits.push((
                                                    intersection(
                                                        content,
                                                        rect(
                                                            stepper.x,
                                                            stepper.y,
                                                            stepper.width,
                                                            half,
                                                        ),
                                                    ),
                                                    Target::Step(row, true),
                                                ));
                                                self.hits.push((
                                                    intersection(
                                                        content,
                                                        rect(
                                                            stepper.x,
                                                            stepper.y + half,
                                                            stepper.width,
                                                            half,
                                                        ),
                                                    ),
                                                    Target::Step(row, false),
                                                ));
                                            }
                                        }
                                    }
                                    Kind::Tags => {
                                        let chips = rect(control_x, cy, control_max, field_h);
                                        paint_chips(
                                            theme,
                                            controls,
                                            ink,
                                            tr,
                                            &mut self.hits,
                                            entry,
                                            row,
                                            chips,
                                            content,
                                            scale,
                                            enabled && !conflict,
                                            self.focus,
                                        );
                                        if entry.tags().is_empty() {
                                            text(
                                                theme,
                                                layer,
                                                &ink.quiet,
                                                &tr.tr("None"),
                                                chips,
                                                content,
                                            );
                                        }
                                        let field = rect(
                                            control_x,
                                            cy + field_h + 6. * scale,
                                            control_max.min(220. * scale),
                                            22. * scale,
                                        );
                                        let placeholder = tr.tr("Add tag…");
                                        self.paint_field(
                                            theme,
                                            tr,
                                            ink,
                                            styles,
                                            controls,
                                            fields,
                                            row,
                                            field,
                                            content,
                                            scale,
                                            focused,
                                            enabled && !conflict,
                                            &placeholder,
                                        );
                                        if control_focused {
                                            focus_ring(controls, ink, field, content, 5.);
                                        }
                                    }
                                    Kind::Check => {}
                                }
                            }
                        }
                        // The "?" at the right edge explains the option on hover.
                        let entry = &self.rows[row];
                        if !entry.description.is_empty() {
                            boxed(
                                controls,
                                help_r,
                                content,
                                [0.; 4],
                                ink.outline,
                                (help / 2.) as f32,
                            );
                            let mark = "?";
                            let w = theme.advance(&ink.quiet, mark);
                            text(
                                theme,
                                layer,
                                &ink.quiet,
                                mark,
                                rect(
                                    help_r.x + (help_r.width - w) / 2.,
                                    help_r.y,
                                    w + 1.,
                                    help_r.height,
                                ),
                                content,
                            );
                            let mut tip = entry.description.clone();
                            if let Some(effect) = &entry.effect {
                                tip.push(' ');
                                tip.push_str(effect);
                            }
                            if conflict {
                                tip.push(' ');
                                tip.push_str(&tr.tr("This value changed outside Settings."));
                            }
                            let area = intersection(
                                content,
                                rect(help_r.x - 4., help_r.y - 4., help + 8., help + 8.),
                            );
                            self.hits.push((area, Target::Help(row)));
                            self.tooltips.push((area, tip));
                        }
                        if conflict {
                            let msg = tr.tr("This value changed outside Settings.");
                            text(
                                theme,
                                layer,
                                &ink.error,
                                &msg,
                                rect(
                                    control_x,
                                    sy + line_h - 2.,
                                    (right - control_x).max(0.),
                                    14. * scale,
                                ),
                                content,
                            );
                        } else if let Some(error) = self
                            .edit
                            .as_ref()
                            .filter(|edit| edit.row == row)
                            .and_then(|edit| edit.error.clone())
                        {
                            text(
                                theme,
                                layer,
                                &ink.error,
                                &error,
                                rect(
                                    control_x,
                                    sy + line_h + 1.,
                                    (right - control_x).max(0.),
                                    14. * scale,
                                ),
                                content,
                            );
                        }
                    }
                    Entry::Button { .. } => {}
                },
            }
        }
        if self.page_extent > viewport && viewport > 0. {
            let thumb =
                (viewport * viewport / self.page_extent).clamp(24f64.min(viewport), viewport);
            let y = content.y + (viewport - thumb) * offset / (self.page_extent - viewport);
            rounded(
                layer,
                rect(content.x + content.width - 7., y + 2., 4., thumb - 4.),
                content,
                mix(ink.bg, ink.fg, 0.25),
                2.,
            );
        }
        self.entries[page] = entries;
    }

    /// A single-line editor for text and numbers; the live input while it is being edited.
    #[allow(clippy::too_many_arguments)]
    fn paint_field(
        &mut self,
        theme: &ThemeUi,
        tr: &Translator,
        ink: &Ink,
        styles: [&Style; 3],
        controls: &mut Layer,
        fields: &mut Layer,
        row: usize,
        field: Rect,
        clip: Rect,
        scale: f64,
        focused: bool,
        enabled: bool,
        placeholder: &str,
    ) {
        let tags = self.rows[row].control() == super::Control::Array;
        let visible = intersection(field, clip) == field;
        if enabled {
            self.hits
                .push((intersection(clip, field), Target::Control(row)));
        }
        let label_bounds = rect(
            field.x + 7. * scale,
            field.y,
            field.width - 14. * scale,
            field.height,
        );
        if let Some(edit) = self
            .edit
            .as_mut()
            .filter(|edit| edit.row == row && visible && enabled)
        {
            edit.input.bounds = field;
            edit.input.scale = scale * if tags { 0.9 } else { 1. };
            let style = if edit.conflict || edit.error.is_some() {
                styles[2]
            } else if focused {
                styles[1]
            } else {
                styles[0]
            };
            crate::panels::paint_themed_input(theme, style, fields, &mut edit.input, focused);
            if tags && edit.input.value().is_empty() && !edit.input.composing() {
                text(
                    theme,
                    fields,
                    &ink.quiet,
                    placeholder,
                    rect(
                        label_bounds.x + 4. * scale,
                        label_bounds.y,
                        label_bounds.width,
                        label_bounds.height,
                    ),
                    clip,
                );
            }
            return;
        }
        if enabled {
            boxed(controls, field, clip, ink.control, ink.border, 5.);
        } else {
            boxed(controls, field, clip, ink.field, [0.; 4], 5.);
        }
        let caption = self.conflicts.get(&row).cloned().unwrap_or_else(|| {
            if tags {
                placeholder.to_owned()
            } else {
                self.rows[row].display(tr)
            }
        });
        text(
            theme,
            fields,
            if !enabled {
                &ink.muted
            } else if tags {
                &ink.quiet
            } else {
                &ink.normal
            },
            &caption,
            label_bounds,
            clip,
        );
    }

    fn preview_height(&self, preview: super::pages::Preview, scale: f64) -> f64 {
        match preview {
            super::pages::Preview::Font => {
                let size = self.preview_font_size();
                (2. * 1.5 * size + 22.).max(62.) * scale.min(1.) + (scale - 1.).max(0.) * 10.
            }
            super::pages::Preview::Indentation => {
                INDENT_SAMPLE.len() as f64 * 18. * scale + 20. * scale
            }
        }
    }

    fn value_of(&self, key: &str) -> Option<&Value> {
        self.row_of(key).map(|row| &self.rows[row].value)
    }

    fn preview_font_size(&self) -> f64 {
        self.value_of("font_size")
            .and_then(Value::as_f64)
            .unwrap_or(13.)
            .clamp(8., 24.)
    }

    /// The editor font at its size, or the indentation as typed, in a framed sample.
    #[allow(clippy::too_many_arguments)]
    fn paint_preview(
        &self,
        theme: &ThemeUi,
        ink: &Ink,
        preview: super::pages::Preview,
        layer: &mut Layer,
        r: Rect,
        clip: Rect,
        scale: f64,
    ) {
        // The frame is a surface; text and guides in the same layer draw above it.
        boxed(layer, r, clip, ink.control, ink.border, 6.);
        let face = self
            .value_of("font_face")
            .and_then(Value::as_str)
            .filter(|face| !face.is_empty())
            .unwrap_or("Menlo")
            .to_owned();
        let mut style = ink.normal.clone();
        style.family = Some(face.into());
        style.font.bold = false;
        style.foreground = ink.fg;
        let inner = rect(
            r.x + 12. * scale,
            r.y + 8. * scale,
            r.width - 24. * scale,
            r.height - 16. * scale,
        );
        match preview {
            super::pages::Preview::Font => {
                style.size = self.preview_font_size() as f32;
                let line = inner.height / 2.;
                for (i, sample) in FONT_SAMPLE.iter().enumerate() {
                    text(
                        theme,
                        layer,
                        &style,
                        sample,
                        rect(inner.x, inner.y + line * i as f64, inner.width, line),
                        intersection(clip, r),
                    );
                }
            }
            super::pages::Preview::Indentation => {
                style.size = (12. * scale) as f32;
                let tab = self
                    .value_of("tab_size")
                    .and_then(Value::as_u64)
                    .unwrap_or(4)
                    .clamp(1, 16) as usize;
                let spaces = self
                    .value_of("translate_tabs_to_spaces")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let guides = self
                    .value_of("draw_indent_guides")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                let column = theme.advance(&style, " ").max(1.);
                let line = 18. * scale;
                let mut marks = style.clone();
                marks.foreground = mix(ink.fg, ink.control, 0.6);
                let frame = intersection(clip, r);
                for (i, (level, code)) in INDENT_SAMPLE.iter().enumerate() {
                    let y = inner.y + 2. * scale + line * i as f64;
                    for l in 0..*level {
                        let x = inner.x + (l * tab) as f64 * column;
                        if guides && l > 0 {
                            vel_ui::fill(layer, rect(x, y, 1., line), frame, ink.line);
                        }
                        if spaces {
                            for c in 0..tab {
                                text(
                                    theme,
                                    layer,
                                    &marks,
                                    "·",
                                    rect(x + c as f64 * column, y, column + 1., line),
                                    frame,
                                );
                            }
                        } else {
                            text(
                                theme,
                                layer,
                                &marks,
                                "→",
                                rect(x, y, column * tab as f64, line),
                                frame,
                            );
                        }
                    }
                    text(
                        theme,
                        layer,
                        &style,
                        code,
                        rect(
                            inner.x + (level * tab) as f64 * column,
                            y,
                            inner.width,
                            line,
                        ),
                        frame,
                    );
                }
            }
        }
    }
}
