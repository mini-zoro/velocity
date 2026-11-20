//! Settings sheet layout and paint. Hit rectangles and accessibility use the same layout.
use super::model::{Control, Focus, Group, Page, Target};
use vel_config::settings::Settings;
use vel_i18n::Translator;
use vel_platform::{
    Point, Rect, Size,
    accessibility::{Node, Region, Role, Tree},
};
use vel_render::scene::{Color, Layer, Scene, SolidRect};
use vel_ui::{
    fill,
    minihtml::intersection,
    theme::{Style, ThemeUi},
};

const AX_ROOT: u64 = 1 << 48;
const AX_LIST: u64 = AX_ROOT + 1;
const AX_SEARCH: u64 = AX_ROOT + 2;
const AX_ROW: u64 = AX_ROOT + 16;

fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect {
        x,
        y,
        width: width.max(0.),
        height: height.max(0.),
    }
}

fn rounded(layer: &mut Layer, rect: Rect, clip: Rect, color: Color, radius: f32) {
    layer.rects.push(SolidRect {
        rect,
        clip,
        color,
        radius,
    });
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

struct Ink {
    normal: Style,
    strong: Style,
    quiet: Style,
    code: Style,
    link: Style,
    error: Style,
    title: Style,
    bg: Color,
    sidebar: Color,
    line: Color,
    selection: Color,
    field: Color,
    accent: Color,
}

impl Ink {
    fn new(theme: &mut ThemeUi, settings: &Settings, dark: bool) -> Self {
        use vel_theme::{Element, ElementPath};
        let colors = if dark {
            (
                [0.040, 0.043, 0.049, 1.],
                [0.81, 0.83, 0.88, 1.],
                [0.27, 0.65, 1., 1.],
                [1., 0.33, 0.30, 1.],
            )
        } else {
            (
                [0.99, 0.99, 1., 1.],
                [0.03, 0.04, 0.057, 1.],
                [0.012, 0.20, 0.52, 1.],
                [0.68, 0.020, 0.026, 1.],
            )
        };
        let (bg, fg, accent, red) = colors;
        let mut normal = theme.style(
            crate::panels::StyleId::SettingsLabel.at(0),
            &ElementPath(&[
                Element::new("window", &[]),
                Element::new("label_control", &[]),
            ]),
            settings,
        );
        normal.foreground = fg;
        normal.case = None;
        let derive = |color, ratio, bold| {
            let mut style = normal.clone();
            style.foreground = color;
            style.size *= ratio;
            style.font.bold = bold;
            style
        };
        let mut code = derive(mix(fg, bg, 0.29), 0.90, false);
        code.family = Some("Menlo".into());
        Self {
            strong: derive(fg, 1.09, true),
            quiet: derive(mix(fg, bg, 0.28), 0.93, false),
            link: derive(accent, 0.93, false),
            error: derive(red, 0.93, false),
            title: derive(fg, 1.55, true),
            normal,
            code,
            bg,
            sidebar: mix(bg, fg, 0.032),
            selection: mix(bg, accent, 0.12),
            field: mix(bg, fg, 0.07),
            line: mix(bg, fg, 0.12),
            accent,
        }
    }
}

fn text(theme: &ThemeUi, layer: &mut Layer, style: &Style, value: &str, in_rect: Rect, clip: Rect) {
    let baseline = in_rect.y + (in_rect.height + f64::from(style.size) * 0.69) / 2.;
    theme.text_fit(
        layer,
        style,
        value,
        in_rect.x,
        baseline,
        in_rect.width,
        clip,
    );
}

fn button(theme: &ThemeUi, layer: &mut Layer, ink: &Ink, caption: &str, bounds: Rect) {
    rounded(layer, bounds, bounds, ink.field, 5.);
    text(
        theme,
        layer,
        &ink.normal,
        caption,
        rect(bounds.x + 9., bounds.y, bounds.width - 18., bounds.height),
        bounds,
    );
}

fn toggle(layer: &mut Layer, ink: &Ink, r: Rect, clip: Rect, on: bool, enabled: bool) {
    let bg = if on && enabled { ink.accent } else { ink.line };
    rounded(layer, r, clip, bg, (r.height * 0.5) as f32);
    let d = (r.height - 6.).max(0.);
    let x = if on { r.x + r.width - d - 3. } else { r.x + 3. };
    rounded(
        layer,
        rect(x, r.y + 3., d, d),
        clip,
        if enabled {
            [0.98, 0.98, 1., 1.]
        } else {
            ink.quiet.foreground
        },
        (d * 0.5) as f32,
    );
}

fn divider(layer: &mut Layer, ink: &Ink, r: Rect, clip: Rect) {
    fill(layer, r, clip, ink.line);
}

fn frame_layer(bg: Color, bounds: Rect) -> Layer {
    let mut layer = Layer::default();
    fill(&mut layer, bounds, bounds, bg);
    layer
}

impl Page {
    pub(crate) fn paint(
        &mut self,
        theme: &mut ThemeUi,
        settings: &Settings,
        tr: &Translator,
        size: Size,
        dark: bool,
        focused: bool,
    ) -> Scene {
        use vel_theme::{Element, ElementPath};
        let ink = Ink::new(theme, settings, dark);
        let input_style = theme.style(
            crate::panels::StyleId::SettingsInput.at(0),
            &ElementPath(&[
                Element::new("window", &[]),
                Element::new("text_line_control", &[]),
            ]),
            settings,
        );
        let scale = (f64::from(ink.normal.size) / 12.).clamp(0.85, 2.);
        let bounds = rect(0., 0., size.width, size.height);
        self.bounds = bounds;
        self.hits.clear();
        let mut layer = frame_layer(ink.bg, bounds);
        let margin = 18. * scale;
        let field_h = 30. * scale;
        let title_h = 43. * scale;
        let toolbar_y = title_h + 9. * scale;
        let header = toolbar_y + field_h + 12. * scale;
        let footer_h = if self.error.is_some() { 65. } else { 42. } * scale;
        let footer_y = (size.height - footer_h).max(header);
        let sidebar_w = (188. * scale).min(size.width * 0.36);
        self.sidebar_bounds = rect(0., header, sidebar_w, footer_y - header);
        self.list_bounds = rect(sidebar_w, header, size.width - sidebar_w, footer_y - header);
        let title = tr.tr("Settings");
        let edit_json = tr.tr("Edit in JSON");
        let json_w = (theme.advance(&ink.normal, &edit_json) + 20. * scale).min(size.width * 0.42);
        let json = rect(size.width - margin - json_w, 10. * scale, json_w, field_h);
        text(
            theme,
            &mut layer,
            &ink.title,
            &title,
            rect(margin, 5. * scale, json.x - margin * 2., title_h),
            bounds,
        );
        button(theme, &mut layer, &ink, &edit_json, json);
        self.hits.push((json, Target::Json));

        let modified = tr.tr("Show modified only");
        let modified_w =
            (theme.advance(&ink.normal, &modified) + 52. * scale).min(size.width * 0.49);
        let modify = rect(
            size.width - margin - modified_w,
            toolbar_y,
            modified_w,
            field_h,
        );
        let search_bounds = rect(margin, toolbar_y, modify.x - margin * 1.7, field_h);
        self.search.bounds = search_bounds;
        if self.focus == Focus::Search && focused {
            rounded(
                &mut layer,
                rect(
                    search_bounds.x - 1.,
                    search_bounds.y - 1.,
                    search_bounds.width + 2.,
                    search_bounds.height + 2.,
                ),
                bounds,
                ink.accent,
                5.,
            );
        }
        crate::panels::paint_themed_input(
            theme,
            &input_style,
            &mut layer,
            &mut self.search,
            self.focus == Focus::Search && focused,
        );
        if self.search.value().is_empty() && !self.search.composing() {
            text(
                theme,
                &mut layer,
                &ink.quiet,
                &tr.tr("Search settings"),
                rect(
                    search_bounds.x + 14. * scale,
                    search_bounds.y,
                    search_bounds.width - 20. * scale,
                    field_h,
                ),
                search_bounds,
            );
        }
        self.hits.push((search_bounds, Target::Search));
        toggle(
            &mut layer,
            &ink,
            rect(modify.x, modify.y + 6. * scale, 34. * scale, 18. * scale),
            modify,
            self.modified_only,
            true,
        );
        text(
            theme,
            &mut layer,
            &ink.normal,
            &modified,
            rect(
                modify.x + 43. * scale,
                modify.y,
                modify.width - 43. * scale,
                field_h,
            ),
            modify,
        );
        self.hits.push((modify, Target::Modified));

        fill(&mut layer, self.sidebar_bounds, bounds, ink.sidebar);
        divider(&mut layer, &ink, rect(0., header, size.width, 1.), bounds);
        divider(
            &mut layer,
            &ink,
            rect(sidebar_w, header, 1., footer_y - header),
            bounds,
        );
        self.paint_categories(theme, tr, &ink, &mut layer, scale);
        self.paint_rows(theme, tr, &ink, &input_style, &mut layer, scale, focused);
        divider(&mut layer, &ink, rect(0., footer_y, size.width, 1.), bounds);

        let footer = rect(
            margin,
            footer_y + 1.,
            size.width - 2. * margin,
            footer_h - 1.,
        );
        let count = tr.tr_fmt(
            "Modified settings ({count})",
            &[("count", self.modified_count.to_string())],
        );
        let path = self.path.as_ref().map(|p| {
            tr.tr_fmt(
                "User file: {path}",
                &[("path", p.to_string_lossy().into_owned())],
            )
        });
        if let Some(error) = &self.error {
            let repair = tr.tr("Edit JSON to fix");
            let w = (theme.advance(&ink.normal, &repair) + 20. * scale).min(footer.width * 0.45);
            let b = rect(
                footer.x + footer.width - w,
                footer.y + 6. * scale,
                w,
                field_h,
            );
            text(
                theme,
                &mut layer,
                &ink.error,
                &tr.tr_fmt("Read-only: {detail}", &[("detail", error.clone())]),
                rect(footer.x, footer.y, b.x - footer.x - margin, 34. * scale),
                footer,
            );
            if self.path.is_some() {
                button(theme, &mut layer, &ink, &repair, b);
                self.hits.push((b, Target::Repair));
            }
            if let Some(path) = &path {
                text(
                    theme,
                    &mut layer,
                    &ink.quiet,
                    path,
                    rect(footer.x, footer.y + 34. * scale, footer.width, 24. * scale),
                    footer,
                );
            }
        } else {
            let count_w =
                (theme.advance(&ink.strong, &count) + margin * 1.5).min(footer.width * 0.48);
            text(
                theme,
                &mut layer,
                &ink.strong,
                &count,
                rect(footer.x, footer.y, count_w, footer.height),
                footer,
            );
            if let Some(path) = &path {
                text(
                    theme,
                    &mut layer,
                    &ink.quiet,
                    path,
                    rect(
                        footer.x + count_w,
                        footer.y,
                        footer.width - count_w,
                        footer.height,
                    ),
                    footer,
                );
            }
        }
        Scene {
            background: ink.bg,
            layers: vec![layer],
        }
    }

    fn paint_categories(
        &mut self,
        theme: &ThemeUi,
        tr: &Translator,
        ink: &Ink,
        layer: &mut Layer,
        scale: f64,
    ) {
        let clip = self.sidebar_bounds;
        let inset = 9. * scale;
        let all = tr.tr("All settings");
        self.category_scroll.row_height = 33. * scale;
        self.category_scroll.rows = self.groups.len() + 1;
        self.category_scroll
            .set_viewport((clip.height - 2. * inset).max(0.));
        let category_clip = rect(
            clip.x,
            clip.y + inset,
            clip.width,
            (clip.height - 2. * inset).max(0.),
        );
        let items = std::iter::once((None, all, self.modified_count)).chain(
            self.groups
                .iter()
                .map(|(&g, &count)| (Some(g), g.name(tr), count)),
        );
        let shown = self.category_scroll.visible();
        for (i, (group, name, count)) in items.enumerate().skip(shown.start).take(shown.len()) {
            let h = self.category_scroll.row_height;
            let r = rect(
                clip.x + inset,
                category_clip.y + h * i as f64 - self.category_scroll.offset,
                clip.width - 2. * inset,
                h - 3. * scale,
            );
            let selected = group == self.category && self.search.value().trim().is_empty();
            if selected {
                rounded(layer, r, category_clip, ink.selection, 5.);
                if self.focus == Focus::Sidebar {
                    rounded(
                        layer,
                        rect(r.x, r.y + 4. * scale, 3., r.height - 8. * scale),
                        category_clip,
                        ink.accent,
                        1.5,
                    );
                }
            }
            let count = count.to_string();
            let count_w = theme.advance(&ink.code, &count) + 8. * scale;
            text(
                theme,
                layer,
                if selected { &ink.strong } else { &ink.normal },
                &name,
                rect(
                    r.x + 8. * scale,
                    r.y,
                    r.width - count_w - 13. * scale,
                    r.height,
                ),
                category_clip,
            );
            text(
                theme,
                layer,
                if count == "0" { &ink.quiet } else { &ink.link },
                &count,
                rect(r.x + r.width - count_w, r.y, count_w, r.height),
                category_clip,
            );
            self.hits
                .push((intersection(category_clip, r), Target::Category(group)));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_rows(
        &mut self,
        theme: &ThemeUi,
        tr: &Translator,
        ink: &Ink,
        input_style: &Style,
        layer: &mut Layer,
        scale: f64,
        focused: bool,
    ) {
        let clip = self.list_bounds;
        let pad = 15. * scale;
        let right = clip.x + clip.width - pad;
        let heading = if self.search.value().trim().is_empty() {
            self.category
                .map_or_else(|| tr.tr("All settings"), |group: Group| group.name(tr))
        } else {
            tr.tr("Search results")
        };
        text(
            theme,
            layer,
            &ink.strong,
            &heading,
            rect(clip.x + pad, clip.y, clip.width - 2. * pad, 39. * scale),
            clip,
        );
        let list = rect(
            clip.x,
            clip.y + 39. * scale,
            clip.width,
            clip.height - 39. * scale,
        );
        self.scroll.row_height = 89. * scale;
        self.scroll.set_viewport(list.height);
        if self.filtered.is_empty() {
            text(
                theme,
                layer,
                &ink.quiet,
                &tr.tr("No matching settings."),
                rect(
                    clip.x + pad,
                    list.y + 10. * scale,
                    clip.width - 2. * pad,
                    29. * scale,
                ),
                list,
            );
            return;
        }
        let editable = self.error.is_none();
        let edit_json = tr.tr("Edit in JSON");
        let link_w = (theme.advance(&ink.link, &edit_json) + 5. * scale).min(clip.width * 0.43);
        for i in self.scroll.visible() {
            let id = self.filtered[i];
            let row = &self.rows[id];
            let y = list.y + self.scroll.row_height * i as f64 - self.scroll.offset;
            let cell = rect(
                clip.x + pad,
                y + 1.,
                clip.width - 2. * pad,
                self.scroll.row_height - 2.,
            );
            if intersection(cell, list).height <= 0. {
                continue;
            }
            let active = self.selected == i && matches!(self.focus, Focus::List | Focus::Edit);
            if active {
                rounded(layer, cell, list, ink.selection, 5.);
                if focused {
                    rounded(
                        layer,
                        rect(cell.x, cell.y + 5., 3., cell.height - 10.),
                        list,
                        ink.accent,
                        1.5,
                    );
                }
            }
            divider(
                layer,
                ink,
                rect(
                    cell.x + 7.,
                    y + self.scroll.row_height - 1.,
                    cell.width - 14.,
                    1.,
                ),
                list,
            );
            self.hits.push((intersection(list, cell), Target::Row(id)));
            let left = cell.x + 9. * scale;
            let right = right - 8. * scale;
            let control_w = (142. * scale).min(cell.width * 0.41);
            let control = rect(
                right - control_w - 25. * scale,
                y + 8. * scale,
                control_w,
                26. * scale,
            );
            let name_w = control.x - left - 7. * scale;
            let can_edit = editable && row.editable();
            let style = if row.compatibility {
                &ink.quiet
            } else {
                &ink.strong
            };
            text(
                theme,
                layer,
                style,
                &row.name,
                rect(left, y + 3. * scale, name_w, 23. * scale),
                list,
            );
            let mark_w = if row.modified { 9. * scale } else { 0. };
            if row.modified {
                rounded(
                    layer,
                    rect(left, y + 30. * scale, 5. * scale, 5. * scale),
                    list,
                    ink.accent,
                    3.,
                );
            }
            text(
                theme,
                layer,
                &ink.code,
                row.key(),
                rect(left + mark_w, y + 23. * scale, name_w - mark_w, 18. * scale),
                list,
            );
            if row.modified && editable {
                let reset = rect(
                    right - 21. * scale,
                    y + 8. * scale,
                    21. * scale,
                    26. * scale,
                );
                text(theme, layer, &ink.link, &tr.tr("✕"), reset, list);
                self.hits
                    .push((intersection(list, reset), Target::Reset(id)));
            }

            let edit = self.edit.as_mut().filter(|edit| edit.row == id);
            let mut editor_error = None;
            if can_edit && row.control() == Control::Bool {
                let switch = rect(
                    control.x + control.width - 37. * scale,
                    control.y + 4. * scale,
                    36. * scale,
                    18. * scale,
                );
                toggle(
                    layer,
                    ink,
                    switch,
                    list,
                    row.value.as_bool().unwrap_or(false),
                    true,
                );
                self.hits
                    .push((intersection(list, control), Target::Control(id)));
            } else {
                let number = can_edit && row.control() == Control::Number;
                let step_w = if number {
                    (21. * scale).min(control.width * 0.25)
                } else {
                    0.
                };
                let field = rect(
                    control.x,
                    control.y,
                    control.width - 2. * step_w,
                    control.height,
                );
                if let Some(edit) = edit {
                    editor_error = edit.error.as_deref();
                    edit.input.bounds = field;
                    if intersection(field, list) == field {
                        rounded(
                            layer,
                            rect(
                                field.x - 1.,
                                field.y - 1.,
                                field.width + 2.,
                                field.height + 2.,
                            ),
                            list,
                            if editor_error.is_some() {
                                ink.error.foreground
                            } else {
                                ink.accent
                            },
                            5.,
                        );
                        crate::panels::paint_themed_input(
                            theme,
                            input_style,
                            layer,
                            &mut edit.input,
                            focused,
                        );
                    }
                } else {
                    if can_edit {
                        rounded(layer, field, list, ink.field, 4.);
                    }
                    text(
                        theme,
                        layer,
                        if can_edit { &ink.normal } else { &ink.quiet },
                        &row.display(tr),
                        rect(
                            field.x + 7. * scale,
                            field.y,
                            field.width - 14. * scale,
                            field.height,
                        ),
                        list,
                    );
                }
                if can_edit {
                    self.hits
                        .push((intersection(list, field), Target::Control(id)));
                }
                if number {
                    for (j, (up, caption)) in [(false, tr.tr("−")), (true, tr.tr("+"))]
                        .into_iter()
                        .enumerate()
                    {
                        let r = rect(
                            field.x + field.width + step_w * j as f64,
                            field.y,
                            step_w,
                            field.height,
                        );
                        rounded(
                            layer,
                            rect(r.x + 1., r.y, r.width - 1., r.height),
                            list,
                            ink.field,
                            3.,
                        );
                        text(
                            theme,
                            layer,
                            &ink.normal,
                            &caption,
                            rect(r.x + 4. * scale, r.y, r.width - 4. * scale, r.height),
                            list,
                        );
                        self.hits
                            .push((intersection(list, r), Target::Step(id, up)));
                    }
                }
            }
            let description = editor_error.unwrap_or(&row.description);
            text(
                theme,
                layer,
                if editor_error.is_some() {
                    &ink.error
                } else {
                    &ink.quiet
                },
                description,
                rect(left, y + 40. * scale, right - left, 22. * scale),
                list,
            );
            let link = rect(right - link_w, y + 64. * scale, link_w, 19. * scale);
            text(
                theme,
                layer,
                &ink.quiet,
                &row.effect,
                rect(left, link.y, link.x - left - 10. * scale, link.height),
                list,
            );
            text(theme, layer, &ink.link, &edit_json, link, list);
            self.hits.push((intersection(list, link), Target::Json));
        }
        let total = self.scroll.rows as f64 * self.scroll.row_height;
        if total > list.height && list.height > 0. {
            let thumb = (list.height * list.height / total)
                .clamp(0., list.height)
                .max(12.)
                .min(list.height);
            let travel = list.height - thumb;
            let y = list.y + travel * self.scroll.offset / (total - list.height);
            rounded(
                layer,
                rect(list.x + list.width - 7., y, 3., thumb),
                list,
                ink.line,
                1.5,
            );
        }
    }

    pub(crate) fn accessibility(&self, tr: &Translator) -> Tree {
        let mut tree = Tree {
            nodes: vec![
                Node::new(AX_ROOT, None, Role::Group, tr.tr("Settings"), self.bounds),
                Node::new(
                    AX_LIST,
                    Some(AX_ROOT),
                    Role::List,
                    tr.tr("Settings"),
                    self.list_bounds,
                ),
            ],
        };
        let mut search = Node::new(
            AX_SEARCH,
            Some(AX_ROOT),
            Role::TextField,
            tr.tr("Search settings"),
            self.search.bounds,
        );
        search.editable = true;
        search.focused = self.focus == Focus::Search;
        search.region = Some(Region::Panel);
        tree.nodes.push(search);
        for &(frame, target) in &self.hits {
            let Target::Row(row) = target else { continue };
            let value = &self.rows[row];
            let state = if value.modified {
                tr.tr("Modified")
            } else {
                tr.tr("Unmodified")
            };
            let label = tr.tr_fmt(
                "{name}, {key}, {value}, {state}",
                &[
                    ("name", value.name.clone()),
                    ("key", value.key().into()),
                    ("value", value.display(tr)),
                    ("state", state.into_owned()),
                ],
            );
            let mut node = Node::new(AX_ROW + row as u64, Some(AX_LIST), Role::Row, label, frame);
            node.help = value.description.clone();
            node.selected = self.filtered.get(self.selected) == Some(&row);
            node.focused = node.selected && matches!(self.focus, Focus::List | Focus::Edit);
            node.enabled = self.error.is_none();
            node.region = Some(Region::Panel);
            node.activation = Some((
                Point {
                    x: frame.x + 4.,
                    y: frame.y + frame.height / 2.,
                },
                false,
            ));
            tree.nodes.push(node);
        }
        tree
    }
}
