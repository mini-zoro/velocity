//! Settings sheet layout and paint. Hit rectangles and accessibility use the same layout.
use super::model::{Control, Focus, Group, Page, Row, Target};
use std::{path::Path, time::Instant};
use vel_config::settings::Settings;
use vel_i18n::Translator;
use vel_platform::{
    Point, Rect, Size, TextInputState, Utf16Range,
    accessibility::{Id, Node, Region, Role, TextArea, Tree},
};
use vel_render::scene::{BorderEdge, Color, CssBox, Layer, Scene, SolidRect, Surface};
use vel_ui::{
    fill,
    minihtml::intersection,
    theme::{Style, ThemeUi},
};

const AX_ROOT: u64 = 1 << 48;
const AX_LIST: u64 = AX_ROOT + 1;
const AX_SEARCH: u64 = AX_ROOT + 2;
const AX_ROW: u64 = AX_ROOT + 16;
const AX_HEADING: u64 = AX_ROOT + 3;
const AX_ANNOUNCEMENT: u64 = AX_ROOT + 4;
const AX_POPUP: u64 = AX_ROOT + 5;
const AX_CHROME: u64 = AX_ROOT + 0x2000;
const AX_CONTROL: u64 = AX_ROOT + 0x4000;
const AX_CHOICE: u64 = AX_ROOT + 0x100000;
const ROW_HEIGHT: f64 = 48.;
const MAX_CHIPS: usize = 3;

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
    muted: Style,
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
            muted: derive(mix(bg, fg, 0.48), 1., false),
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

fn input_style(theme: &mut ThemeUi, settings: &Settings, focused: bool, invalid: bool) -> Style {
    use vel_theme::{Element, ElementPath};
    let attrs: &[&str] = match (focused, invalid) {
        (true, true) => &["focused", "invalid"],
        (true, false) => &["focused"],
        (false, true) => &["invalid"],
        (false, false) => &[],
    };
    theme.style(
        crate::panels::StyleId::SettingsInput.at(u64::from(focused) + u64::from(invalid) * 2),
        &ElementPath(&[
            Element::new("window", &[]),
            Element::new("panel_control find_panel", &[]),
            Element::new("text_input", attrs),
        ]),
        settings,
    )
}

fn ring(layer: &mut Layer, r: Rect, clip: Rect, color: Color) {
    layer.surfaces.push(Surface::CssBox(CssBox {
        rect: r,
        clip,
        background: [0.; 4],
        radii: [4.; 4],
        borders: [BorderEdge { color, width: 1. }; 4],
    }));
}

fn button(
    theme: &ThemeUi,
    layer: &mut Layer,
    ink: &Ink,
    caption: &str,
    bounds: Rect,
    enabled: bool,
    focused: bool,
) {
    rounded(layer, bounds, bounds, ink.field, 5.);
    // Solid primitives composite above themed surfaces; keep this stroke on top.
    if focused {
        fill(
            layer,
            rect(
                bounds.x + 5.,
                bounds.y + bounds.height - 2.,
                bounds.width - 10.,
                1.,
            ),
            bounds,
            ink.accent,
        );
    }
    text(
        theme,
        layer,
        if enabled { &ink.normal } else { &ink.muted },
        caption,
        rect(bounds.x + 9., bounds.y, bounds.width - 18., bounds.height),
        bounds,
    );
}

fn toggle(layer: &mut Layer, ink: &Ink, r: Rect, clip: Rect, on: bool, enabled: bool) {
    let bg = if on {
        mix(ink.bg, ink.accent, if enabled { 1. } else { 0.35 })
    } else {
        ink.line
    };
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
            ink.muted.foreground
        },
        (d * 0.5) as f32,
    );
}

fn abbreviated(path: &Path, home: Option<&Path>) -> String {
    if let Some(relative) = home
        .filter(|_| cfg!(unix))
        .and_then(|home| path.strip_prefix(home).ok())
    {
        return Path::new("~").join(relative).to_string_lossy().into_owned();
    }
    vel_config::paths::display(path)
}

fn middle_fit(path: &str, fits: impl Fn(&str) -> bool) -> String {
    if fits(path) {
        return path.to_owned();
    }
    let suffix = path
        .rfind("/Packages/User/")
        .or_else(|| path.rfind('/'))
        .map_or(path, |i| &path[i..]);
    let prefix_end = path.len() - suffix.len();
    for (i, _) in path[..prefix_end].match_indices('/').rev() {
        if fits(&format!("{}/…{suffix}", &path[..i])) {
            return format!("{}/…{suffix}", &path[..i]);
        }
    }
    let home = if path.starts_with("~/") { "~/" } else { "" };
    for (i, _) in suffix.char_indices() {
        let shortened = format!("{home}…{}", &suffix[i..]);
        if fits(&shortened) {
            return shortened;
        }
    }
    format!("{home}…")
}

fn divider(layer: &mut Layer, ink: &Ink, r: Rect, clip: Rect) {
    fill(layer, r, clip, ink.line);
}

#[allow(clippy::too_many_arguments)]
fn paint_chips(
    theme: &ThemeUi,
    layer: &mut Layer,
    ink: &Ink,
    tr: &Translator,
    hits: &mut Vec<(Rect, Target)>,
    row: &Row,
    id: usize,
    control: Rect,
    clip: Rect,
    scale: f64,
    enabled: bool,
    focus: Focus,
) {
    let mut x = control.x;
    let tags = row.tags();
    let right = control.x + control.width;
    let gap = 4. * scale;
    let padding = 6. * scale;
    let remove_width = 18. * scale;
    let chip_style = if enabled { &ink.normal } else { &ink.muted };
    let mut shown = 0;
    for (index, tag) in tags.iter().take(MAX_CHIPS).enumerate() {
        let label = Row::tag_label(tag);
        let remaining = tags.len() - index - 1;
        let more = if remaining == 0 {
            0.
        } else {
            let label = tr.tr_fmt("+{count} more", &[("count", remaining.to_string())]);
            theme.advance(&ink.quiet, &label) + 14. * scale + gap
        };
        let width = (theme.advance(chip_style, &label).ceil() + 2. * padding + remove_width)
            .min(152. * scale);
        if x + width + more > right {
            break;
        }
        let chip = rect(
            x,
            control.y + 2. * scale,
            width,
            control.height - 4. * scale,
        );
        let remove = rect(
            chip.x + chip.width - remove_width,
            chip.y,
            remove_width,
            chip.height,
        );
        let target = Target::RemoveTag(id, index);
        rounded(
            layer,
            chip,
            clip,
            if focus == Focus::Action(target) {
                ink.selection
            } else {
                ink.field
            },
            4.,
        );
        text(
            theme,
            layer,
            chip_style,
            &label,
            rect(
                chip.x + padding,
                chip.y,
                chip.width - 2. * padding - remove_width,
                chip.height,
            ),
            clip,
        );
        text(
            theme,
            layer,
            chip_style,
            &tr.tr("✕"),
            rect(
                remove.x + 3. * scale,
                remove.y,
                remove.width - 3. * scale,
                remove.height,
            ),
            clip,
        );
        if enabled {
            hits.push((intersection(clip, remove), target));
        }
        x += width + gap;
        shown += 1;
    }
    if shown < tags.len() {
        let label = tr.tr_fmt(
            "+{count} more",
            &[("count", (tags.len() - shown).to_string())],
        );
        let w = (theme.advance(&ink.quiet, &label) + 14. * scale).min((right - x).max(0.));
        let more = rect(x, control.y + 2. * scale, w, control.height - 4. * scale);
        let target = Target::MoreTags(id);
        rounded(
            layer,
            more,
            clip,
            if focus == Focus::Action(target) {
                ink.selection
            } else {
                ink.field
            },
            4.,
        );
        text(
            theme,
            layer,
            if enabled { &ink.quiet } else { &ink.muted },
            &label,
            rect(
                more.x + 5. * scale,
                more.y,
                more.width - 8. * scale,
                more.height,
            ),
            clip,
        );
        if enabled {
            hits.push((intersection(clip, more), target));
        }
    }
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
        let ink = Ink::new(theme, settings, dark);
        let idle = input_style(theme, settings, false, false);
        let active = input_style(theme, settings, true, false);
        let invalid = input_style(theme, settings, true, true);
        let scale = (f64::from(ink.normal.size) / 12.).clamp(0.85, 2.);
        let bounds = rect(0., 0., size.width, size.height);
        self.bounds = bounds;
        self.hits.clear();
        self.tooltips.clear();
        let mut layer = frame_layer(ink.bg, bounds);
        let mut fields = Layer::default();
        let margin = 16. * scale;
        let field_h = 28. * scale;
        let title_h = 40. * scale;
        let toolbar_y = title_h + 6. * scale;
        let header = toolbar_y + field_h + 10. * scale;
        let footer_h = if self.error.is_some() || self.notice.is_some() {
            64.
        } else {
            36.
        } * scale;
        let footer_y = (size.height - footer_h).max(header);
        let sidebar_w = (188. * scale).min(size.width * 0.36);
        self.sidebar_bounds = rect(0., header, sidebar_w, footer_y - header);
        self.list_bounds = rect(sidebar_w, header, size.width - sidebar_w, footer_y - header);
        let title = tr.tr("Settings");
        let edit_json = tr.tr("Edit in JSON");
        let json_w = (theme.advance(&ink.normal, &edit_json) + 20. * scale).min(size.width * 0.29);
        let json = rect(size.width - margin - json_w, 10. * scale, json_w, field_h);
        let reset_all = tr.tr("Reset all");
        let reset_w = (theme.advance(&ink.normal, &reset_all) + 20. * scale).min(size.width * 0.23);
        let reset = rect(json.x - 8. * scale - reset_w, json.y, reset_w, field_h);
        text(
            theme,
            &mut layer,
            &ink.title,
            &title,
            rect(margin, 4. * scale, reset.x - margin * 2., title_h),
            bounds,
        );
        button(
            theme,
            &mut layer,
            &ink,
            &edit_json,
            json,
            true,
            self.focus == Focus::Action(Target::Json),
        );
        self.hits.push((json, Target::Json));
        button(
            theme,
            &mut layer,
            &ink,
            &reset_all,
            reset,
            self.writable(),
            self.focus == Focus::Action(Target::ResetAll),
        );
        if self.writable() {
            self.hits.push((reset, Target::ResetAll));
        }

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
        self.search.scale = scale;
        let search_focused = self.focus == Focus::Search && focused;
        let search_style = if search_focused { &active } else { &idle };
        crate::panels::paint_themed_input(
            theme,
            search_style,
            &mut fields,
            &mut self.search,
            search_focused,
        );
        if self.search.value().is_empty() && !self.search.composing() {
            let mut placeholder = search_style.clone();
            placeholder.foreground = mix(search_style.foreground, ink.bg, 0.26);
            text(
                theme,
                &mut fields,
                &placeholder,
                &tr.tr("Search settings"),
                rect(
                    search_bounds.x + search_style.margin.left as f64 + 4. * scale,
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
        if self.focus == Focus::Action(Target::Modified) {
            fill(
                &mut layer,
                rect(modify.x, modify.y + modify.height - 1., modify.width, 1.),
                modify,
                ink.accent,
            );
        }

        fill(&mut layer, self.sidebar_bounds, bounds, ink.sidebar);
        divider(&mut layer, &ink, rect(0., header, size.width, 1.), bounds);
        divider(
            &mut layer,
            &ink,
            rect(sidebar_w, header, 1., footer_y - header),
            bounds,
        );
        self.paint_categories(theme, tr, &ink, &mut layer, scale);
        self.paint_rows(
            theme,
            tr,
            &ink,
            [&idle, &active, &invalid],
            &mut layer,
            &mut fields,
            scale,
            focused,
        );
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
        let path_rect = if self.error.is_some() || self.notice.is_some() {
            let (message, action, target, enabled) = if let Some(error) = &self.error {
                (
                    tr.tr_fmt("Read-only: {detail}", &[("detail", error.clone())]),
                    tr.tr("Edit JSON to fix"),
                    Target::Repair,
                    self.path.is_some(),
                )
            } else {
                (
                    self.notice.clone().unwrap(),
                    tr.tr("Reload"),
                    Target::Reload,
                    self.can_reload(),
                )
            };
            let w = (theme.advance(&ink.normal, &action) + 20. * scale).min(footer.width * 0.45);
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
                &message,
                rect(footer.x, footer.y, b.x - footer.x - margin, 34. * scale),
                footer,
            );
            button(
                theme,
                &mut layer,
                &ink,
                &action,
                b,
                enabled,
                self.focus == Focus::Action(target),
            );
            if enabled {
                self.hits.push((b, target));
            }
            self.tooltips.push((
                rect(footer.x, footer.y, b.x - footer.x - margin, 34. * scale),
                message,
            ));
            rect(footer.x, footer.y + 34. * scale, footer.width, 24. * scale)
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
            rect(
                footer.x + count_w,
                footer.y,
                footer.width - count_w,
                footer.height,
            )
        };
        if let Some(path) = &self.path {
            let prefix = |path| tr.tr_fmt("User file: {path}", &[("path", path)]);
            let abbreviated = abbreviated(path, self.home.as_deref());
            let short = middle_fit(&abbreviated, |candidate| {
                theme.advance(&ink.quiet, &prefix(candidate.to_owned())) <= path_rect.width
            });
            text(
                theme,
                &mut layer,
                &ink.quiet,
                &prefix(short),
                path_rect,
                footer,
            );
            self.tooltips
                .push((path_rect, path.to_string_lossy().into_owned()));
        }
        let mut layers = vec![layer, fields];
        if let Some(popup) = self.paint_popup(theme, settings, &ink, scale) {
            layers.push(popup);
        } else if let Some(tip) = self.paint_tooltip(theme, &ink, scale) {
            layers.push(tip);
        }
        Scene {
            background: ink.bg,
            layers,
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
        styles: [&Style; 3],
        layer: &mut Layer,
        fields: &mut Layer,
        scale: f64,
        focused: bool,
    ) {
        let clip = self.list_bounds;
        let pad = 13. * scale;
        self.heading = if self.search.value().trim().is_empty() {
            self.category
                .map_or_else(|| tr.tr("All settings"), |group: Group| group.name(tr))
        } else {
            tr.tr("Search results")
        }
        .into_owned();
        self.heading_bounds = rect(clip.x + pad, clip.y, clip.width - 2. * pad, 30. * scale);
        text(
            theme,
            layer,
            &ink.strong,
            &self.heading,
            self.heading_bounds,
            clip,
        );
        let list = rect(
            clip.x,
            clip.y + 30. * scale,
            clip.width,
            clip.height - 30. * scale,
        );
        self.row_list = list;
        self.scroll.row_height = ROW_HEIGHT * scale;
        self.scroll.set_viewport(list.height);
        if self.filtered.is_empty() {
            text(
                theme,
                layer,
                &ink.quiet,
                &tr.tr("No matching settings."),
                rect(clip.x + pad, list.y, clip.width - 2. * pad, 32. * scale),
                list,
            );
            return;
        }

        let edit_json = tr.tr("Edit in JSON");
        let link_w = (theme.advance(&ink.link, &edit_json) + 2. * scale).min(list.width * 0.27);
        let writable = self.writable();
        let conflict_text = tr.tr("This value changed outside Settings.");
        for i in self.scroll.visible() {
            let id = self.filtered[i];
            let row = &self.rows[id];
            let y = list.y + self.scroll.row_height * i as f64 - self.scroll.offset;
            let bounds = rect(
                list.x + pad,
                y,
                list.width - 2. * pad,
                self.scroll.row_height,
            );
            let visible = intersection(list, bounds);
            if visible.width == 0. || visible.height == 0. {
                continue;
            }
            self.hits.push((visible, Target::Row(id)));
            self.tooltips.push((visible, row.description.clone()));
            let selected = self.selected == i
                && matches!(
                    self.focus,
                    Focus::List
                        | Focus::Edit
                        | Focus::Action(
                            Target::RowJson(_)
                                | Target::NewWindow(_)
                                | Target::Reset(_)
                                | Target::RemoveTag(..)
                                | Target::MoreTags(_)
                        )
                );
            let hovered = self.hovered.is_some_and(|p| vel_ui::contains(visible, p));
            if selected || hovered {
                rounded(
                    layer,
                    bounds,
                    list,
                    if selected { ink.selection } else { ink.sidebar },
                    4.,
                );
            }
            divider(
                layer,
                ink,
                rect(bounds.x, y + bounds.height - 1., bounds.width, 1.),
                list,
            );
            let left = bounds.x + 13. * scale;
            if row.modified {
                rounded(
                    layer,
                    rect(left - 9. * scale, y + 13. * scale, 5. * scale, 5. * scale),
                    list,
                    ink.accent,
                    2.5,
                );
            }
            let reset_w = 23. * scale;
            let right = (bounds.x + bounds.width - reset_w).max(left);
            let control_w = match row.control() {
                Control::Bool => 40.,
                Control::Array => 304.,
                _ => 151.,
            } * scale;
            let control_w = control_w.min(bounds.width * 0.46);
            let is_array = row.control() == Control::Array && row.editable();
            let control = rect(right - control_w, y + 4. * scale, control_w, 23. * scale);
            let title_end = (control.x - 8. * scale).max(left);
            let title_w = title_end - left;
            let name_w = theme.advance(&ink.strong, &row.name).min(title_w * 0.59);
            text(
                theme,
                layer,
                &ink.strong,
                &row.name,
                rect(left, y + 3. * scale, name_w + 1., 23. * scale),
                list,
            );
            let key_x = left + name_w + 8. * scale;
            text(
                theme,
                layer,
                &ink.code,
                row.key(),
                rect(key_x, y + 3. * scale, title_end - key_x, 23. * scale),
                list,
            );

            let edit = self.edit.as_mut().filter(|edit| edit.row == id);
            let saved = self.conflicts.get(&id).map(String::as_str);
            let conflict = saved.is_some() || edit.as_ref().is_some_and(|edit| edit.conflict);
            let error = edit.as_ref().and_then(|edit| edit.error.clone());
            let enabled = writable && row.editable() && !conflict;
            let caption_style = if enabled { &ink.normal } else { &ink.muted };
            let mut field = control;
            match row.control() {
                Control::Bool if row.editable() => {
                    let switch = rect(
                        control.x + (control.width - 35. * scale).max(0.),
                        y + 7. * scale,
                        control.width.min(35. * scale),
                        18. * scale,
                    );
                    toggle(
                        layer,
                        ink,
                        switch,
                        list,
                        row.value.as_bool().unwrap_or(false),
                        enabled,
                    );
                    self.hits
                        .push((intersection(list, switch), Target::Control(id)));
                }
                Control::Enum if row.editable() => {
                    rounded(layer, control, list, ink.field, 4.);
                    let chevron = rect(
                        control.x + control.width - 17. * scale,
                        control.y,
                        15. * scale,
                        control.height,
                    );
                    text(theme, layer, caption_style, "⌄", chevron, list);
                    text(
                        theme,
                        layer,
                        caption_style,
                        &row.display(tr),
                        rect(
                            control.x + 7. * scale,
                            control.y,
                            control.width - 27. * scale,
                            control.height,
                        ),
                        list,
                    );
                    self.hits
                        .push((intersection(list, control), Target::Control(id)));
                }
                Control::Number | Control::Text | Control::Array if row.editable() => {
                    if is_array {
                        paint_chips(
                            theme,
                            layer,
                            ink,
                            tr,
                            &mut self.hits,
                            row,
                            id,
                            control,
                            list,
                            scale,
                            enabled,
                            self.focus,
                        );
                        field = rect(control.x, y + 27. * scale, control.width, 19. * scale);
                    } else if row.control() == Control::Number {
                        let step_w = (20. * scale).min(control.width * 0.25);
                        field.width = (control.width - 2. * step_w).max(0.);
                        for (j, (up, caption)) in [(false, tr.tr("−")), (true, tr.tr("+"))]
                            .into_iter()
                            .enumerate()
                        {
                            let step = rect(
                                field.x + field.width + step_w * j as f64 + 1.,
                                field.y,
                                step_w - 1.,
                                field.height,
                            );
                            rounded(layer, step, list, ink.field, 3.);
                            text(
                                theme,
                                layer,
                                caption_style,
                                &caption,
                                rect(
                                    step.x + 4. * scale,
                                    step.y,
                                    step.width - 4. * scale,
                                    step.height,
                                ),
                                list,
                            );
                            if enabled {
                                self.hits
                                    .push((intersection(list, step), Target::Step(id, up)));
                            }
                        }
                    }
                    self.hits
                        .push((intersection(list, field), Target::Control(id)));
                    let placeholder = tr.tr("Add tag…");
                    let label_bounds = rect(
                        field.x + 7. * scale,
                        field.y,
                        field.width - 14. * scale,
                        field.height,
                    );
                    if let Some(edit) =
                        edit.filter(|_| intersection(field, list) == field && writable)
                    {
                        edit.input.bounds = field;
                        edit.input.scale = scale * if is_array { 0.82 } else { 1. };
                        let style = if conflict || error.is_some() {
                            styles[2]
                        } else if focused {
                            styles[1]
                        } else {
                            styles[0]
                        };
                        crate::panels::paint_themed_input(
                            theme,
                            style,
                            fields,
                            &mut edit.input,
                            focused,
                        );
                        if is_array && edit.input.value().is_empty() && !edit.input.composing() {
                            text(
                                theme,
                                fields,
                                &ink.quiet,
                                &placeholder,
                                rect(
                                    label_bounds.x + 5. * scale,
                                    label_bounds.y,
                                    label_bounds.width - 5. * scale,
                                    label_bounds.height,
                                ),
                                list,
                            );
                        }
                    } else {
                        if enabled {
                            theme.surface(fields, styles[0], field, list);
                        } else {
                            rounded(fields, field, list, ink.field, 4.);
                        }
                        let caption = saved
                            .map(str::to_owned)
                            .or_else(|| {
                                self.edit
                                    .as_ref()
                                    .filter(|edit| edit.row == id)
                                    .map(|edit| edit.input.value())
                            })
                            .unwrap_or_else(|| {
                                if is_array {
                                    placeholder.into_owned()
                                } else {
                                    row.display(tr)
                                }
                            });
                        text(
                            theme,
                            fields,
                            if is_array && enabled {
                                &ink.quiet
                            } else {
                                caption_style
                            },
                            &caption,
                            label_bounds,
                            list,
                        );
                    }
                }
                _ => text(theme, layer, &ink.quiet, &row.display(tr), control, list),
            }

            if row.modified {
                let reset = rect(
                    right + 3. * scale,
                    y + 4. * scale,
                    reset_w - 3. * scale,
                    23. * scale,
                );
                if self.focus == Focus::Action(Target::Reset(id)) {
                    rounded(layer, reset, list, ink.field, 4.);
                }
                text(
                    theme,
                    layer,
                    if writable { &ink.normal } else { &ink.muted },
                    &tr.tr("✕"),
                    rect(
                        reset.x + 4. * scale,
                        reset.y,
                        reset.width - 4. * scale,
                        reset.height,
                    ),
                    list,
                );
                self.hits
                    .push((intersection(list, reset), Target::Reset(id)));
                self.tooltips.push((
                    intersection(list, reset),
                    tr.tr("Reset setting").into_owned(),
                ));
            }
            let second_end = if is_array {
                control.x - 8. * scale
            } else {
                right + reset_w
            };
            let link = rect(second_end - link_w, y + 26. * scale, link_w, 19. * scale);
            let enum_open = self.popup.as_ref().is_some_and(|p| p.row == id && !p.tags);
            if (selected || hovered) && link.x > left && !enum_open {
                text(theme, layer, &ink.link, &edit_json, link, list);
                if self.focus == Focus::Action(Target::RowJson(id)) {
                    fill(
                        layer,
                        rect(link.x, link.y + link.height - 2., link.width, 1.),
                        list,
                        ink.accent,
                    );
                }
                self.hits
                    .push((intersection(list, link), Target::RowJson(id)));
            }
            let mut description_end = (link.x - 8. * scale).max(left);
            if row.next_window() && writable && (selected || hovered) {
                let caption = tr.tr("New Window");
                let width = (theme.advance(&ink.link, &caption) + scale)
                    .min((description_end - left) * 0.35);
                let button = rect(description_end - width, link.y, width, link.height);
                text(theme, layer, &ink.link, &caption, button, list);
                let target = Target::NewWindow(id);
                if self.focus == Focus::Action(target) {
                    fill(
                        layer,
                        rect(button.x, button.y + button.height - 2., button.width, 1.),
                        list,
                        ink.accent,
                    );
                }
                self.hits.push((intersection(list, button), target));
                description_end = (button.x - 8. * scale).max(left);
            }
            let warning = if conflict {
                Some(conflict_text.as_ref())
            } else {
                error.as_deref()
            };
            if warning.is_none()
                && let Some(effect) = &row.effect
            {
                let width = theme
                    .advance(&ink.quiet, effect)
                    .min((description_end - left) * 0.49);
                let note = rect(description_end - width, link.y, width, link.height);
                text(theme, layer, &ink.quiet, effect, note, list);
                description_end = note.x - 10. * scale;
            }
            let description = rect(left, link.y, description_end - left, link.height);
            text(
                theme,
                layer,
                if warning.is_some() {
                    &ink.error
                } else {
                    &ink.quiet
                },
                warning.unwrap_or(&row.description),
                description,
                list,
            );
            if let Some(warning) = warning {
                self.tooltips
                    .push((intersection(list, description), warning.to_owned()));
            }
        }
        let total = self.scroll.rows as f64 * self.scroll.row_height;
        if total > list.height && list.height > 0. {
            let thumb =
                (list.height * list.height / total).clamp(12f64.min(list.height), list.height);
            let y = list.y + (list.height - thumb) * self.scroll.offset / (total - list.height);
            rounded(
                layer,
                rect(list.x + list.width - 6., y, 3., thumb),
                list,
                ink.line,
                1.5,
            );
        }
    }

    fn paint_popup(
        &mut self,
        theme: &mut ThemeUi,
        settings: &Settings,
        ink: &Ink,
        scale: f64,
    ) -> Option<Layer> {
        let popup = self.popup.as_mut()?;
        let anchor_wanted = if popup.tags {
            Target::MoreTags(popup.row)
        } else {
            Target::Control(popup.row)
        };
        let (anchor, _) = self
            .hits
            .iter()
            .find(|(_, target)| *target == anchor_wanted)?;
        let row_height = 27. * scale;
        let height =
            (row_height * popup.panel.items.len().min(8) as f64).min(self.bounds.height * 0.75);
        let width = (anchor.width + if popup.tags { 150. * scale } else { 0. })
            .max(164. * scale)
            .min(self.bounds.width);
        let x = (anchor.x + anchor.width - width).clamp(0., self.bounds.width - width);
        let below = anchor.y + anchor.height + 3. * scale;
        let y = if below + height <= self.bounds.height {
            below
        } else {
            (anchor.y - height - 3. * scale).max(0.)
        };
        popup.bounds = rect(x, y, width, height);
        popup.panel.scroll.row_height = row_height;
        if popup.panel.scroll.viewport != height {
            popup.panel.scroll.set_viewport(height);
            popup.panel.scroll.reveal(popup.panel.selected);
        }
        for i in popup.panel.scroll.visible() {
            let frame = rect(
                x,
                y + row_height * i as f64 - popup.panel.scroll.offset,
                width,
                row_height,
            );
            self.hits
                .push((intersection(popup.bounds, frame), Target::PopupItem(i)));
        }
        let mut layer = Layer::default();
        layer.surfaces.push(Surface::Fill(SolidRect {
            rect: popup.bounds,
            clip: self.bounds,
            color: ink.sidebar,
            radius: 5.,
        }));
        popup
            .panel
            .paint_with_theme(&mut layer, popup.bounds, theme, settings);
        ring(&mut layer, popup.bounds, self.bounds, ink.line);
        Some(layer)
    }

    fn paint_tooltip(&self, theme: &ThemeUi, ink: &Ink, scale: f64) -> Option<Layer> {
        let point = self.hovered?;
        if Instant::now() < self.hover_deadline()? {
            return None;
        }
        let (anchor, tip) = self
            .tooltips
            .iter()
            .rfind(|(area, _)| vel_ui::contains(*area, point))?;
        let width = (theme.advance(&ink.normal, tip) + 16. * scale)
            .min(490. * scale)
            .min(self.bounds.width);
        if width < 30. * scale {
            return None;
        }
        let mut lines = Vec::new();
        let mut start = 0;
        let mut space = None;
        for (i, ch) in tip.char_indices() {
            if theme.advance(&ink.normal, &tip[start..i + ch.len_utf8()]) > width - 16. * scale
                && i > start
            {
                let end = space.filter(|&at| at > start).unwrap_or(i);
                lines.push(tip[start..end].trim_end());
                start = end;
                space = None;
            }
            if ch.is_whitespace() {
                space = Some(i + ch.len_utf8());
            }
        }
        lines.push(&tip[start..]);
        let line_height = 17. * scale;
        let height = line_height * lines.len() as f64 + 12. * scale;
        let x = (point.x + 10. * scale).clamp(0., self.bounds.width - width);
        let y = if point.y + 18. * scale + height <= self.bounds.height {
            point.y + 18. * scale
        } else {
            (anchor.y - height - 4. * scale).max(0.)
        };
        let r = rect(x, y, width, height);
        let mut layer = Layer::default();
        layer.surfaces.push(Surface::Fill(SolidRect {
            rect: r,
            clip: self.bounds,
            color: ink.sidebar,
            radius: 5.,
        }));
        ring(&mut layer, r, self.bounds, ink.line);
        for (i, line) in lines.into_iter().enumerate() {
            text(
                theme,
                &mut layer,
                &ink.normal,
                line,
                rect(
                    x + 8. * scale,
                    y + 6. * scale + i as f64 * line_height,
                    width - 16. * scale,
                    line_height,
                ),
                intersection(r, self.bounds),
            );
        }
        Some(layer)
    }

    pub(crate) fn accessibility(&self, tr: &Translator, include_text: bool) -> Tree {
        let mut tree = Tree {
            nodes: vec![
                Node::new(AX_ROOT, None, Role::Group, tr.tr("Settings"), self.bounds),
                Node::new(
                    AX_LIST,
                    Some(AX_ROOT),
                    Role::List,
                    tr.tr("Settings"),
                    self.row_list,
                ),
                Node::new(
                    AX_HEADING,
                    Some(AX_LIST),
                    Role::Heading,
                    &self.heading,
                    self.heading_bounds,
                ),
            ],
            ..Default::default()
        };
        if let Some(text) = &self.announcement {
            let mut node = Node::new(AX_ANNOUNCEMENT, None, Role::Status, text, Rect::default());
            node.announcement = Some(text.clone());
            tree.nodes.push(node);
        }
        if let Some(popup) = &self.popup {
            tree.nodes.push(Node::new(
                AX_POPUP,
                Some(AX_ROOT),
                Role::List,
                &self.rows[popup.row].name,
                popup.bounds,
            ));
        }
        for &(frame, target) in &self.hits {
            if frame.width <= 0. || frame.height <= 0. {
                continue;
            }
            let row_id = |row: usize| AX_ROW + row as Id;
            let control_id = |row: usize| AX_CONTROL + row as Id * 64;
            let center = Point {
                x: frame.x + frame.width / 2.,
                y: frame.y + frame.height / 2.,
            };
            let chrome = |offset, role, title: &str| {
                Node::new(AX_CHROME + offset, Some(AX_ROOT), role, title, frame)
            };
            let mut node = match target {
                Target::Search => Node::new(
                    AX_SEARCH,
                    Some(AX_ROOT),
                    Role::TextField,
                    tr.tr("Search settings"),
                    frame,
                ),
                Target::Category(group) => {
                    let label = group.map_or_else(|| tr.tr("All settings"), |group| group.name(tr));
                    let mut node =
                        chrome(20 + group.map_or(0, |g| g as u64 + 1), Role::Button, &label);
                    node.selected = group == self.category && self.search.value().trim().is_empty();
                    node.focused = node.selected && self.focus == Focus::Sidebar;
                    node
                }
                Target::Modified => {
                    let mut node = chrome(0, Role::Switch, &tr.tr("Show modified only"));
                    node.checked = Some(self.modified_only);
                    node
                }
                Target::Json => chrome(1, Role::Button, &tr.tr("Edit in JSON")),
                Target::Repair => chrome(2, Role::Button, &tr.tr("Edit JSON to fix")),
                Target::ResetAll => chrome(3, Role::Button, &tr.tr("Reset all")),
                Target::Reload => chrome(4, Role::Button, &tr.tr("Reload")),
                Target::Row(row) => {
                    let entry = &self.rows[row];
                    let label = tr.tr_fmt(
                        "{name}, {key}, {value}, {state}",
                        &[
                            ("name", entry.name.clone()),
                            ("key", entry.key().into()),
                            ("value", entry.display(tr)),
                            (
                                "state",
                                tr.tr(if entry.modified {
                                    "Modified"
                                } else {
                                    "Unmodified"
                                })
                                .into_owned(),
                            ),
                        ],
                    );
                    let mut node = Node::new(row_id(row), Some(AX_LIST), Role::Row, label, frame);
                    node.help = entry.description.clone();
                    if let Some(effect) = &entry.effect {
                        node.help.push(' ');
                        node.help.push_str(effect);
                    }
                    if self.conflicts.contains_key(&row)
                        || self
                            .edit
                            .as_ref()
                            .is_some_and(|edit| edit.row == row && edit.conflict)
                    {
                        node.help.push(' ');
                        node.help
                            .push_str(&tr.tr("This value changed outside Settings."));
                    }
                    node.selected = self.filtered.get(self.selected) == Some(&row);
                    node.focused =
                        node.selected && self.focus == Focus::List && self.popup.is_none();
                    node.activation = Some((
                        Point {
                            x: frame.x + 4.,
                            y: center.y,
                        },
                        false,
                    ));
                    tree.nodes.push(node);
                    continue;
                }
                Target::Control(row) => {
                    let entry = &self.rows[row];
                    let role = match entry.control() {
                        Control::Bool => Role::Switch,
                        Control::Enum => Role::PopupButton,
                        Control::Number | Control::Text | Control::Array => Role::TextField,
                        _ => continue,
                    };
                    let label = if role == Role::PopupButton {
                        tr.tr_fmt(
                            "{name}, {value}",
                            &[("name", entry.name.clone()), ("value", entry.display(tr))],
                        )
                    } else if entry.control() == Control::Array {
                        tr.tr_fmt(
                            "{name}, {value}",
                            &[
                                ("name", entry.name.clone()),
                                ("value", tr.tr("Add tag…").into_owned()),
                            ],
                        )
                    } else {
                        entry.name.clone()
                    };
                    let mut node =
                        Node::new(control_id(row), Some(row_id(row)), role, label, frame);
                    let edit = self.edit.as_ref().filter(|edit| edit.row == row);
                    node.enabled = self.writable()
                        && entry.editable()
                        && edit.is_none_or(|edit| !edit.conflict)
                        && !self.conflicts.contains_key(&row);
                    node.help = entry.description.clone();
                    if role == Role::Switch {
                        node.checked = entry.value.as_bool();
                    }
                    if role == Role::PopupButton {
                        node.expanded =
                            Some(self.popup.as_ref().is_some_and(|popup| popup.row == row));
                    }
                    if role == Role::TextField {
                        node.editable = node.enabled;
                        node.focused =
                            self.focus == Focus::Edit && edit.is_some() && self.popup.is_none();
                        if include_text {
                            let value = if let Some(edit) = edit {
                                edit.input.state()
                            } else {
                                let text = self.conflicts.get(&row).cloned().unwrap_or_else(|| {
                                    if entry.control() == Control::Array {
                                        String::new()
                                    } else {
                                        entry.edit_value()
                                    }
                                });
                                TextInputState {
                                    text: vel_text::Text::from(text.as_str()),
                                    ..Default::default()
                                }
                            };
                            node.text = Some(TextArea::new(
                                &value,
                                Utf16Range::new(0, value.text.len_utf16()),
                            ));
                        }
                    }
                    node
                }
                Target::Step(row, up) => Node::new(
                    control_id(row) + 1 + u64::from(up),
                    Some(row_id(row)),
                    Role::Button,
                    tr.tr(if up { "Increase" } else { "Decrease" }),
                    frame,
                ),
                Target::Reset(row) => {
                    let mut node = Node::new(
                        control_id(row) + 3,
                        Some(row_id(row)),
                        Role::Button,
                        tr.tr("Reset setting"),
                        frame,
                    );
                    node.enabled = self.writable();
                    node
                }
                Target::RowJson(row) => Node::new(
                    control_id(row) + 4,
                    Some(row_id(row)),
                    Role::Link,
                    tr.tr("Edit in JSON"),
                    frame,
                ),
                Target::NewWindow(row) => Node::new(
                    control_id(row) + 12,
                    Some(row_id(row)),
                    Role::Button,
                    tr.tr("New Window"),
                    frame,
                ),
                Target::MoreTags(row) => Node::new(
                    control_id(row) + 5,
                    Some(row_id(row)),
                    Role::PopupButton,
                    tr.tr("Show all tags"),
                    frame,
                ),
                Target::RemoveTag(row, index) => Node::new(
                    control_id(row) + 6 + index as Id,
                    Some(row_id(row)),
                    Role::Button,
                    tr.tr_fmt(
                        "Remove {tag}",
                        &[("tag", Row::tag_label(&self.rows[row].tags()[index]))],
                    ),
                    frame,
                ),
                Target::PopupItem(index) => {
                    let Some(popup) = &self.popup else { continue };
                    let mut node = Node::new(
                        AX_CHOICE + index as Id,
                        Some(AX_POPUP),
                        Role::Row,
                        &popup.panel.items[index].caption,
                        frame,
                    );
                    node.selected = popup.panel.selected == index;
                    node.focused = node.selected;
                    node
                }
            };
            if target == Target::Search {
                node.editable = true;
                node.focused = self.focus == Focus::Search;
                if include_text {
                    let state = self.search.state();
                    node.text = Some(TextArea::new(
                        &state,
                        Utf16Range::new(0, state.text.len_utf16()),
                    ));
                }
            }
            node.focused |= self.focus == Focus::Action(target) && self.popup.is_none();
            node.region = Some(Region::Panel);
            if node.enabled {
                node.activation = Some((center, false));
            }
            tree.nodes.push(node);
        }
        tree
    }

    pub(crate) fn ax_text_input(&self, id: Id) -> Option<&vel_ui::input::Input> {
        if id == AX_SEARCH {
            Some(&self.search)
        } else {
            self.edit
                .as_ref()
                .and_then(|edit| (id == AX_CONTROL + edit.row as Id * 64).then_some(&edit.input))
        }
    }
}
