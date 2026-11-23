//! Settings window layout and paint. Hit rectangles and accessibility use the same layout.
#[path = "settings_ui_page_view.rs"]
mod page;
use super::model::{Control, Entry, Focus, Kind, Page, Pane, PopupKind, Row, Target, pages};
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
const AX_PANES: u64 = AX_ROOT + 6;
const AX_PANE: u64 = AX_ROOT + 0x1000;
const AX_SECTION: u64 = AX_ROOT + 0x1800;
const AX_BUTTON: u64 = AX_ROOT + 0x1c00;
const MAX_CHIPS: usize = 3;

pub(super) fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect {
        x,
        y,
        width: width.max(0.),
        height: height.max(0.),
    }
}

pub(super) fn rounded(layer: &mut Layer, rect: Rect, clip: Rect, color: Color, radius: f32) {
    layer.rects.push(SolidRect {
        rect,
        clip,
        color,
        radius,
    });
}

pub(super) fn mix(a: Color, b: Color, t: f32) -> Color {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

pub(super) struct Ink {
    normal: Style,
    strong: Style,
    heading: Style,
    quiet: Style,
    code: Style,
    link: Style,
    error: Style,
    muted: Style,
    bg: Color,
    sidebar: Color,
    line: Color,
    selection: Color,
    field: Color,
    accent: Color,
    /// Filled controls: checked boxes, the selected radio, the focus ring.
    fill: Color,
    /// Popup and push-button faces and their hairline.
    control: Color,
    border: Color,
    /// Unchecked checkbox and radio outlines (3:1 against the window, WCAG 1.4.11).
    outline: Color,
    /// Sidebar selection without and with keyboard focus.
    chosen: Color,
    chosen_focus: Color,
    fg: Color,
}

impl Ink {
    fn new(theme: &mut ThemeUi, settings: &Settings, dark: bool) -> Self {
        use vel_theme::{Element, ElementPath};
        // Calm neutral surfaces: a slightly darker sidebar, lighter control faces.
        let system = crate::field_chrome::accent(theme);
        let (bg, sidebar, control, fg, accent, fill, red) = if dark {
            (
                [0.122, 0.127, 0.139, 1.],
                [0.094, 0.098, 0.109, 1.],
                [0.188, 0.194, 0.210, 1.],
                [0.86, 0.87, 0.90, 1.],
                [0.36, 0.68, 1., 1.],
                [0.04, 0.50, 1., 1.],
                [1., 0.40, 0.37, 1.],
            )
        } else {
            (
                [0.968, 0.968, 0.975, 1.],
                [0.925, 0.926, 0.937, 1.],
                [1., 1., 1., 1.],
                [0.08, 0.085, 0.10, 1.],
                [0., 0.36, 0.80, 1.],
                [0., 0.40, 0.85, 1.],
                [0.68, 0.020, 0.026, 1.],
            )
        };
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
            strong: derive(fg, 1.04, true),
            heading: derive(fg, 1.25, true),
            quiet: derive(mix(fg, bg, 0.30), 0.93, false),
            link: derive(accent, 0.93, false),
            error: derive(red, 0.93, false),
            muted: derive(mix(bg, fg, 0.48), 1., false),
            normal,
            code,
            bg,
            sidebar,
            selection: mix(bg, accent, 0.14),
            field: mix(bg, fg, 0.07),
            line: mix(bg, fg, 0.13),
            accent,
            // Checked boxes, chosen radios and focus rings follow the system accent, like
            // native controls; accent-colored text keeps the fixed, contrast-checked blue.
            fill: if vel_platform::system_accent_color().is_some() {
                system
            } else {
                fill
            },
            control,
            border: mix(bg, fg, if dark { 0.22 } else { 0.20 }),
            outline: mix(bg, fg, if dark { 0.50 } else { 0.47 }),
            chosen: mix(sidebar, fg, if dark { 0.13 } else { 0.085 }),
            chosen_focus: mix(sidebar, fill, if dark { 0.42 } else { 0.22 }),
            fg,
        }
    }
}

pub(super) fn text(
    theme: &ThemeUi,
    layer: &mut Layer,
    style: &Style,
    value: &str,
    in_rect: Rect,
    clip: Rect,
) {
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

pub(super) fn input_style(
    theme: &mut ThemeUi,
    settings: &Settings,
    focused: bool,
    invalid: bool,
) -> Style {
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

/// A filled box with a hairline, as a surface so icons and text can sit above it.
pub(super) fn boxed(
    layer: &mut Layer,
    r: Rect,
    clip: Rect,
    background: Color,
    border: Color,
    radius: f32,
) {
    layer.surfaces.push(Surface::CssBox(CssBox {
        rect: r,
        clip,
        background,
        radii: [radius; 4],
        borders: [BorderEdge {
            color: border,
            width: if border[3] > 0. { 1. } else { 0. },
        }; 4],
    }));
}

/// The keyboard focus ring, shared with the tab rename field: a translucent accent band just
/// outside the control.
pub(super) fn focus_ring(layer: &mut Layer, ink: &Ink, r: Rect, clip: Rect, radius: f32) {
    let mut color = ink.fill;
    color[3] = 0.5;
    crate::field_chrome::focus_ring(layer, r, clip, color, radius);
}

/// A push button: a raised face with a hairline and a centered caption.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_button(
    theme: &ThemeUi,
    layer: &mut Layer,
    ink: &Ink,
    caption: &str,
    bounds: Rect,
    clip: Rect,
    enabled: bool,
    focused: bool,
) {
    boxed(layer, bounds, clip, ink.control, ink.border, 5.);
    if focused {
        focus_ring(layer, ink, bounds, clip, 5.);
    }
    let style = if enabled { &ink.normal } else { &ink.muted };
    let width = theme.advance(style, caption).min(bounds.width - 12.);
    text(
        theme,
        layer,
        style,
        caption,
        rect(
            bounds.x + (bounds.width - width) / 2.,
            bounds.y,
            width + 1.,
            bounds.height,
        ),
        clip,
    );
}

/// A popup button: the current choice and up/down chevrons on a control face.
#[allow(clippy::too_many_arguments)]
pub(super) fn popup_button(
    theme: &ThemeUi,
    layer: &mut Layer,
    ink: &Ink,
    caption: &str,
    bounds: Rect,
    clip: Rect,
    enabled: bool,
    focused: bool,
) {
    boxed(layer, bounds, clip, ink.control, ink.border, 5.);
    if focused {
        focus_ring(layer, ink, bounds, clip, 5.);
    }
    let style = if enabled { &ink.normal } else { &ink.muted };
    let icon = (bounds.height * 0.62).min(14.);
    theme.icon(
        layer,
        "Theme - Velocity/updown.png",
        rect(
            bounds.x + bounds.width - icon - 7.,
            bounds.y + (bounds.height - icon) / 2.,
            icon,
            icon,
        ),
        clip,
        style.foreground,
    );
    text(
        theme,
        layer,
        style,
        caption,
        rect(
            bounds.x + 9.,
            bounds.y,
            bounds.width - icon - 22.,
            bounds.height,
        ),
        clip,
    );
}

/// The up/down half of a number field: the top increases, the bottom decreases.
pub(super) fn stepper(
    theme: &ThemeUi,
    layer: &mut Layer,
    ink: &Ink,
    r: Rect,
    clip: Rect,
    color: Color,
) {
    boxed(layer, r, clip, ink.control, ink.border, 5.);
    let icon = (r.width * 0.78).min(13. * r.height / 24.);
    theme.icon(
        layer,
        "Theme - Velocity/updown.png",
        rect(
            r.x + (r.width - icon) / 2.,
            r.y + (r.height - icon) / 2.,
            icon,
            icon,
        ),
        clip,
        color,
    );
}

/// A checkbox square, filled with a check mark when on.
pub(super) fn checkbox(
    theme: &ThemeUi,
    layer: &mut Layer,
    ink: &Ink,
    r: Rect,
    clip: Rect,
    checked: bool,
    enabled: bool,
) {
    if checked {
        let fill = if enabled {
            ink.fill
        } else {
            mix(ink.bg, ink.fill, 0.4)
        };
        boxed(layer, r, clip, fill, [0.; 4], 4.);
        let inset = r.width * 0.1;
        theme.icon(
            layer,
            "Theme - Velocity/check.png",
            rect(
                r.x + inset,
                r.y + inset,
                r.width - 2. * inset,
                r.height - 2. * inset,
            ),
            clip,
            [1., 1., 1., if enabled { 1. } else { 0.8 }],
        );
    } else {
        let outline = if enabled {
            ink.outline
        } else {
            mix(ink.bg, ink.outline, 0.5)
        };
        boxed(layer, r, clip, ink.control, outline, 4.);
    }
}

/// A radio circle, filled with a center dot when chosen.
pub(super) fn radio(
    layer: &mut Layer,
    ink: &Ink,
    r: Rect,
    clip: Rect,
    chosen: bool,
    enabled: bool,
) {
    let radius = (r.width / 2.) as f32;
    if chosen {
        let fill = if enabled {
            ink.fill
        } else {
            mix(ink.bg, ink.fill, 0.4)
        };
        boxed(layer, r, clip, fill, [0.; 4], radius);
        let d = r.width * 0.38;
        boxed(
            layer,
            rect(r.x + (r.width - d) / 2., r.y + (r.height - d) / 2., d, d),
            clip,
            [1., 1., 1., 1.],
            [0.; 4],
            (d / 2.) as f32,
        );
    } else {
        let outline = if enabled {
            ink.outline
        } else {
            mix(ink.bg, ink.outline, 0.5)
        };
        boxed(layer, r, clip, ink.control, outline, radius);
    }
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
    for (i, _) in suffix.match_indices('/').skip(1) {
        let shortened = format!("{home}…{}", &suffix[i..]);
        if fits(&shortened) {
            return shortened;
        }
    }
    for (i, _) in suffix.char_indices() {
        let shortened = format!("{home}…{}", &suffix[i..]);
        if fits(&shortened) {
            return shortened;
        }
    }
    format!("{home}…")
}

pub(super) fn divider(layer: &mut Layer, ink: &Ink, r: Rect, clip: Rect) {
    fill(layer, r, clip, ink.line);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn paint_chips(
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

/// Preview samples are data: the text audit does not treat them as interface text.
pub(super) fn sample_text(drawn: &str) -> bool {
    page::FONT_SAMPLE.contains(&drawn)
        || page::INDENT_SAMPLE.iter().any(|(_, line)| *line == drawn)
        || matches!(drawn, "·" | "→")
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
        self.headings.clear();
        let mut layer = frame_layer(ink.bg, bounds);
        let mut controls = Layer::default();
        let mut fields = Layer::default();
        let sidebar_w = (196. * scale).min(size.width * 0.36).round();
        self.sidebar_bounds = rect(0., 0., sidebar_w, size.height);
        fill(&mut layer, self.sidebar_bounds, bounds, ink.sidebar);
        divider(
            &mut layer,
            &ink,
            rect(sidebar_w, 0., 1., size.height),
            bounds,
        );
        self.paint_sidebar(
            theme,
            tr,
            &ink,
            [&idle, &active],
            [&mut layer, &mut controls, &mut fields],
            scale,
            focused,
        );
        let searching = !self.search.value().trim().is_empty();
        let curated = self.curated_pane();
        let footer_h = if self.error.is_some() || self.notice.is_some() {
            64.
        } else if !curated && !searching {
            34.
        } else {
            0.
        } * scale;
        let content = rect(
            sidebar_w + 1.,
            0.,
            size.width - sidebar_w - 1.,
            (size.height - footer_h).max(0.),
        );
        self.list_bounds = content;
        if curated {
            self.paint_page(
                theme,
                tr,
                &ink,
                [&idle, &active, &invalid],
                [&mut layer, &mut controls, &mut fields],
                content,
                scale,
                focused,
                true,
            );
        } else {
            self.paint_list_pane(
                theme,
                tr,
                &ink,
                [&idle, &active, &invalid],
                [&mut layer, &mut controls, &mut fields],
                content,
                scale,
                focused,
            );
        }
        if footer_h > 0. {
            let footer = rect(
                content.x,
                content.y + content.height,
                content.width,
                footer_h,
            );
            divider(
                &mut layer,
                &ink,
                rect(footer.x, footer.y, footer.width, 1.),
                bounds,
            );
            self.paint_footer(theme, tr, &ink, &mut layer, &mut controls, footer, scale);
        }
        let mut layers = vec![layer, controls, fields];
        if let Some(popup) = self.paint_popup(theme, &ink, scale) {
            layers.push(popup);
        } else if let Some(tip) = self.paint_tooltip(theme, &ink, scale) {
            layers.push(tip);
        }
        Scene {
            background: ink.bg,
            layers,
        }
    }

    /// The search field and the category list.
    #[allow(clippy::too_many_arguments)]
    fn paint_sidebar(
        &mut self,
        theme: &ThemeUi,
        tr: &Translator,
        ink: &Ink,
        [idle, active]: [&Style; 2],
        [layer, controls, fields]: [&mut Layer; 3],
        scale: f64,
        focused: bool,
    ) {
        let clip = self.sidebar_bounds;
        let pad = 12. * scale;
        let field_h = 28. * scale;
        let search_bounds = rect(pad, pad, clip.width - 2. * pad, field_h);
        self.search.bounds = search_bounds;
        self.search.scale = scale;
        let search_focused = self.focus == Focus::Search && focused;
        let search_style = if search_focused { active } else { idle };
        crate::panels::paint_themed_input(
            theme,
            search_style,
            fields,
            &mut self.search,
            search_focused,
        );
        if search_focused {
            focus_ring(controls, ink, search_bounds, clip, 5.);
        }
        if self.search.value().is_empty() && !self.search.composing() {
            let mut placeholder = search_style.clone();
            placeholder.foreground = mix(search_style.foreground, ink.bg, 0.3);
            text(
                theme,
                fields,
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
        let inset = 8. * scale;
        let row_h = 30. * scale;
        let gap = 4. * scale;
        let mut y = search_bounds.y + field_h + 14. * scale;
        let searching = !self.search.value().trim().is_empty();
        for pane in Self::panes() {
            if pane == Pane::Advanced {
                y += 6. * scale;
                divider(layer, ink, rect(pad, y, clip.width - 2. * pad, 1.), clip);
                y += 7. * scale;
            }
            let r = rect(inset, y, clip.width - 2. * inset, row_h);
            let selected = pane == self.pane && !searching;
            if selected {
                let color = if self.focus == Focus::Sidebar && focused {
                    ink.chosen_focus
                } else {
                    ink.chosen
                };
                rounded(layer, r, clip, color, 6.);
            }
            let (icon, name) = match pane {
                Pane::Page(index) => {
                    let page = &pages::pages()[index];
                    (page.icon.as_str(), tr.tr(&page.name))
                }
                Pane::Advanced => ("Theme - Velocity/palette-syntax.png", tr.tr("Advanced")),
            };
            let icon_size = 16.;
            let ink_color = if selected {
                ink.fg
            } else {
                mix(ink.fg, ink.sidebar, 0.22)
            };
            theme.icon(
                controls,
                icon,
                rect(
                    r.x + 9. * scale,
                    r.y + (r.height - icon_size) / 2.,
                    icon_size,
                    icon_size,
                ),
                clip,
                ink_color,
            );
            text(
                theme,
                controls,
                &ink.normal,
                &name,
                rect(
                    r.x + 9. * scale + icon_size + 9. * scale,
                    r.y,
                    r.width - icon_size - 24. * scale,
                    r.height,
                ),
                clip,
            );
            self.hits.push((r, Target::Pane(pane)));
            y += row_h + gap;
        }
    }

    /// Advanced (every setting, with keys and filters) and search results.
    #[allow(clippy::too_many_arguments)]
    fn paint_list_pane(
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
        let pad = 20. * scale;
        let searching = !self.search.value().trim().is_empty();
        let field_h = 26. * scale;
        let top = 14. * scale;
        self.heading = if searching {
            tr.tr("Search results")
        } else {
            tr.tr("Advanced")
        }
        .into_owned();
        let mut header_right = content.x + content.width - pad;
        {
            let json = tr.tr("Edit in JSON");
            let json_w = (theme.advance(&ink.normal, &json) + 26. * scale).min(content.width * 0.3);
            let json_r = rect(header_right - json_w, top, json_w, field_h);
            push_button(
                theme,
                controls,
                ink,
                &json,
                json_r,
                content,
                true,
                self.focus == Focus::Action(Target::Json) && focused,
            );
            self.hits.push((json_r, Target::Json));
            header_right = json_r.x - 10. * scale;
        }
        if !searching {
            let json_r = rect(header_right + 10. * scale, top, 0., field_h);
            let reset = tr.tr("Reset all");
            let reset_w =
                (theme.advance(&ink.normal, &reset) + 26. * scale).min(content.width * 0.25);
            let reset_r = rect(json_r.x - 8. * scale - reset_w, top, reset_w, field_h);
            push_button(
                theme,
                controls,
                ink,
                &reset,
                reset_r,
                content,
                self.writable(),
                self.focus == Focus::Action(Target::ResetAll) && focused,
            );
            if self.writable() {
                self.hits.push((reset_r, Target::ResetAll));
            }
            header_right = reset_r.x - 10. * scale;
        }
        self.heading_bounds = rect(
            content.x + pad,
            top,
            (header_right - content.x - pad).max(0.),
            field_h,
        );
        text(
            theme,
            layer,
            &ink.heading,
            &self.heading,
            self.heading_bounds,
            content,
        );
        let mut list_top = top + field_h + 12. * scale;
        if !searching {
            let group = self
                .category
                .map_or_else(|| tr.tr("All settings"), |group| group.name(tr));
            let group_w = (210. * scale).min(content.width * 0.45);
            let group_r = rect(content.x + pad, list_top, group_w, 24. * scale);
            popup_button(
                theme,
                controls,
                ink,
                &group,
                group_r,
                content,
                true,
                self.focus == Focus::Action(Target::Groups) && focused,
            );
            self.hits.push((group_r, Target::Groups));
            let modified = tr.tr("Show modified only");
            let box_size = 15. * scale;
            let check = rect(
                group_r.x + group_r.width + 18. * scale,
                group_r.y + (group_r.height - box_size) / 2.,
                box_size,
                box_size,
            );
            checkbox(
                theme,
                controls,
                ink,
                check,
                content,
                self.modified_only,
                true,
            );
            if self.focus == Focus::Action(Target::Modified) && focused {
                focus_ring(controls, ink, check, content, 4.);
            }
            let label_x = check.x + box_size + 7. * scale;
            let label_w = theme
                .advance(&ink.normal, &modified)
                .min((content.x + content.width - pad - label_x).max(0.));
            text(
                theme,
                layer,
                &ink.normal,
                &modified,
                rect(label_x, group_r.y, label_w + 1., group_r.height),
                content,
            );
            self.hits.push((
                rect(
                    check.x,
                    group_r.y,
                    label_x + label_w - check.x,
                    group_r.height,
                ),
                Target::Modified,
            ));
            // The number of modified settings, as a badge after the filter.
            let count = self.modified_count.to_string();
            let badge_w = theme.advance(&ink.quiet, &count) + 12. * scale;
            let badge = rect(
                label_x + label_w + 8. * scale,
                group_r.y + (group_r.height - 18. * scale) / 2.,
                badge_w,
                18. * scale,
            );
            if badge.x + badge.width < content.x + content.width - pad {
                boxed(
                    controls,
                    badge,
                    content,
                    ink.field,
                    [0.; 4],
                    (9. * scale) as f32,
                );
                text(
                    theme,
                    controls,
                    &ink.quiet,
                    &count,
                    rect(
                        badge.x + 6. * scale,
                        badge.y,
                        badge_w - 11. * scale,
                        badge.height,
                    ),
                    content,
                );
                self.tooltips.push((
                    badge,
                    tr.tr_fmt(
                        "Modified settings ({count})",
                        &[("count", self.modified_count.to_string())],
                    ),
                ));
            }
            list_top = group_r.y + group_r.height + 10. * scale;
        }
        divider(
            layer,
            ink,
            rect(content.x, list_top - 1., content.width, 1.),
            content,
        );
        let list = rect(
            content.x,
            list_top,
            content.width,
            (content.y + content.height - list_top).max(0.),
        );
        self.row_list = list;
        self.paint_page(
            theme,
            tr,
            ink,
            styles,
            [layer, controls, fields],
            list,
            scale,
            focused,
            false,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_footer(
        &mut self,
        theme: &ThemeUi,
        tr: &Translator,
        ink: &Ink,
        layer: &mut Layer,
        controls: &mut Layer,
        area: Rect,
        scale: f64,
    ) {
        let margin = 16. * scale;
        let field_h = 26. * scale;
        let footer = rect(
            area.x + margin,
            area.y + 1.,
            area.width - 2. * margin,
            area.height - 1.,
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
            let w = (theme.advance(&ink.normal, &action) + 24. * scale).min(footer.width * 0.45);
            let b = rect(
                footer.x + footer.width - w,
                footer.y + 6. * scale,
                w,
                field_h,
            );
            text(
                theme,
                layer,
                &ink.error,
                &message,
                rect(footer.x, footer.y, b.x - footer.x - margin, 34. * scale),
                footer,
            );
            push_button(
                theme,
                controls,
                ink,
                &action,
                b,
                footer,
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
            footer
        };
        if let Some(path) = &self.path {
            let prefix = |path| tr.tr_fmt("User file: {path}", &[("path", path)]);
            let abbreviated = abbreviated(path, self.home.as_deref());
            let short = middle_fit(&abbreviated, |candidate| {
                theme.advance(&ink.quiet, &prefix(candidate.to_owned())) <= path_rect.width
            });
            text(theme, layer, &ink.quiet, &prefix(short), path_rect, footer);
            self.tooltips
                .push((path_rect, path.to_string_lossy().into_owned()));
        }
    }

    fn paint_popup(&mut self, theme: &ThemeUi, ink: &Ink, scale: f64) -> Option<Layer> {
        let popup = self.popup.as_mut()?;
        let anchor_wanted = match popup.kind {
            PopupKind::Tags(row) => Target::MoreTags(row),
            PopupKind::Choices(row) => Target::Control(row),
            PopupKind::Groups => Target::Groups,
        };
        let (anchor, _) = self
            .hits
            .iter()
            .find(|(_, target)| *target == anchor_wanted)?;
        let tags = matches!(popup.kind, PopupKind::Tags(_));
        let row_height = 27. * scale;
        let height =
            (row_height * popup.panel.items.len().min(10) as f64).min(self.bounds.height * 0.75);
        let width = (anchor.width + if tags { 150. * scale } else { 0. })
            .max(180. * scale)
            .min(self.bounds.width);
        let x = if tags {
            anchor.x + anchor.width - width
        } else {
            anchor.x
        }
        .clamp(0., self.bounds.width - width);
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
        // A native-looking menu: a raised face with a soft shadow, the highlighted row in the
        // accent color, and a check mark beside the current value.
        let mut layer = Layer::default();
        let mut shadow = ink.fg;
        shadow[3] = 0.22;
        layer.shadows.push(vel_render::scene::Shadow {
            rect: rect(x - 6., y - 2., width + 12., height + 14.),
            radius: 8.,
            blur: 12.,
            color: shadow,
            clip: self.bounds,
        });
        boxed(
            &mut layer,
            popup.bounds,
            self.bounds,
            ink.control,
            ink.border,
            7.,
        );
        let clip = intersection(popup.bounds, self.bounds);
        let check_w = 22. * scale;
        for i in popup.panel.scroll.visible() {
            let frame = rect(
                x,
                y + row_height * i as f64 - popup.panel.scroll.offset,
                width,
                row_height,
            );
            let caption = &popup.panel.items[i].caption;
            let (current, caption) = match caption.strip_prefix("✓  ") {
                Some(rest) => (true, rest),
                None => (false, caption.as_str()),
            };
            let selected = popup.panel.selected == i;
            if selected {
                rounded(
                    &mut layer,
                    rect(
                        frame.x + 4.,
                        frame.y + 1.,
                        frame.width - 8.,
                        frame.height - 2.,
                    ),
                    clip,
                    ink.fill,
                    4.,
                );
            }
            let mut style = ink.normal.clone();
            if selected {
                style.foreground = [1., 1., 1., 1.];
            }
            if current {
                let icon = 13. * scale;
                // Icons are surfaces, beneath the highlight; draw the mark as text above it.
                text(
                    theme,
                    &mut layer,
                    &style,
                    "✓",
                    rect(frame.x + 9. * scale, frame.y, icon + 2., frame.height),
                    clip,
                );
            }
            text(
                theme,
                &mut layer,
                &style,
                caption,
                rect(
                    frame.x + check_w + 4. * scale,
                    frame.y,
                    frame.width - check_w - 14. * scale,
                    frame.height,
                ),
                clip,
            );
        }
        let total = popup.panel.items.len() as f64 * row_height;
        if total > height {
            let thumb = (height * height / total).max(16.);
            let ty = y + (height - thumb) * popup.panel.scroll.offset / (total - height);
            rounded(
                &mut layer,
                rect(x + width - 6., ty + 2., 3., thumb - 4.),
                clip,
                mix(ink.control, ink.fg, 0.3),
                1.5,
            );
        }
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

    /// The curated caption of a row on screen.
    fn curated_label(&self, row: usize) -> Option<&str> {
        self.display.iter().find_map(|entry| match entry {
            Entry::Setting { row: id, label, .. } if *id == row => Some(label.as_str()),
            _ => None,
        })
    }

    /// A search result's "Page › Section" caption.
    fn location_of(&self, row: usize) -> Option<&str> {
        self.display.iter().find_map(|entry| match entry {
            Entry::Setting {
                row: id,
                location: Some((_, caption)),
                ..
            } if *id == row => Some(caption.as_str()),
            _ => None,
        })
    }

    pub(crate) fn accessibility(&self, tr: &Translator, include_text: bool) -> Tree {
        let curated = self.curated_pane();
        let searching = !self.search.value().trim().is_empty();
        let pane_name = match self.pane {
            Pane::Page(index) if curated => tr.tr(&pages::pages()[index].name),
            _ => std::borrow::Cow::Owned(self.heading.clone()),
        };
        let mut tree = Tree {
            nodes: vec![
                Node::new(AX_ROOT, None, Role::Group, tr.tr("Settings"), self.bounds),
                Node::new(
                    AX_PANES,
                    Some(AX_ROOT),
                    Role::List,
                    tr.tr("Settings categories"),
                    self.sidebar_bounds,
                ),
                Node::new(
                    AX_LIST,
                    Some(AX_ROOT),
                    if matches!(self.pane, Pane::Advanced) && !searching {
                        Role::List
                    } else {
                        Role::Group
                    },
                    pane_name.as_ref(),
                    if curated {
                        self.page_bounds
                    } else {
                        self.row_list
                    },
                ),
            ],
            ..Default::default()
        };
        if !curated {
            tree.nodes.push(Node::new(
                AX_HEADING,
                Some(AX_LIST),
                Role::Heading,
                &self.heading,
                self.heading_bounds,
            ));
        }
        if let Some(text) = &self.announcement {
            let mut node = Node::new(AX_ANNOUNCEMENT, None, Role::Status, text, Rect::default());
            node.announcement = Some(text.clone());
            tree.nodes.push(node);
        }
        if let Some(popup) = &self.popup {
            let name = match popup.kind {
                PopupKind::Choices(row) | PopupKind::Tags(row) => self.rows[row].name.clone(),
                PopupKind::Groups => tr.tr("Group").into_owned(),
            };
            tree.nodes.push(Node::new(
                AX_POPUP,
                Some(AX_ROOT),
                Role::List,
                name,
                popup.bounds,
            ));
        }
        // Curated content reads in drawing order: section headings among their controls.
        let mut content: Vec<Node> = self
            .headings
            .iter()
            .enumerate()
            .map(|(i, (frame, title))| {
                let mut node = Node::new(
                    AX_SECTION + i as Id,
                    Some(AX_LIST),
                    Role::Heading,
                    title,
                    *frame,
                );
                node.region = Some(Region::Panel);
                node
            })
            .collect();
        for &(frame, target) in &self.hits {
            if frame.width <= 0. || frame.height <= 0. {
                continue;
            }
            let row_id = |row: usize| AX_ROW + row as Id;
            let control_id = |row: usize| AX_CONTROL + row as Id * 64;
            let owner = |row: usize| {
                if self.curated(row).is_some() {
                    AX_LIST
                } else {
                    row_id(row)
                }
            };
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
                    Role::SearchField,
                    tr.tr("Search settings"),
                    frame,
                ),
                Target::Pane(pane) => {
                    let (index, label) = match pane {
                        Pane::Page(index) => (index, tr.tr(&pages::pages()[index].name)),
                        Pane::Advanced => (pages::pages().len(), tr.tr("Advanced")),
                    };
                    let mut node = Node::new(
                        AX_PANE + index as Id,
                        Some(AX_PANES),
                        Role::Row,
                        label,
                        frame,
                    );
                    node.selected = pane == self.pane && self.search.value().trim().is_empty();
                    node.focused = node.selected && self.focus == Focus::Sidebar;
                    node
                }
                Target::Groups => {
                    let value = self
                        .category
                        .map_or_else(|| tr.tr("All settings"), |group| group.name(tr));
                    let mut node = chrome(
                        5,
                        Role::PopupButton,
                        &tr.tr_fmt(
                            "{name}, {value}",
                            &[
                                ("name", tr.tr("Group").into_owned()),
                                ("value", value.into_owned()),
                            ],
                        ),
                    );
                    node.expanded = Some(
                        self.popup
                            .as_ref()
                            .is_some_and(|popup| popup.kind == PopupKind::Groups),
                    );
                    node
                }
                Target::Modified => {
                    let mut node = chrome(0, Role::CheckBox, &tr.tr("Show modified only"));
                    node.checked = Some(self.modified_only);
                    node
                }
                Target::Json => chrome(1, Role::Button, &tr.tr("Edit in JSON")),
                Target::Repair => chrome(2, Role::Button, &tr.tr("Edit JSON to fix")),
                Target::ResetAll => chrome(3, Role::Button, &tr.tr("Reset all")),
                Target::Reload => chrome(4, Role::Button, &tr.tr("Reload")),
                Target::Button(id) => Node::new(
                    AX_BUTTON + id as Id,
                    Some(AX_LIST),
                    Role::Button,
                    tr.tr(pages::buttons()[id].caption),
                    frame,
                ),
                Target::Help(_) => continue,
                Target::Locate(row) => {
                    let mut node = Node::new(
                        control_id(row) + 13,
                        Some(AX_LIST),
                        Role::Link,
                        self.location_of(row).unwrap_or_default(),
                        frame,
                    );
                    node.help = tr.tr("Show on its page").into_owned();
                    node
                }
                Target::Choice(row, index) => {
                    let entry = &self.rows[row];
                    let Some((_, enabled, choices)) = self.curated(row) else {
                        continue;
                    };
                    let Some(choice) = choices.get(index) else {
                        continue;
                    };
                    let mut node = Node::new(
                        control_id(row) + 16 + index as Id,
                        Some(AX_LIST),
                        Role::RadioButton,
                        tr.tr_fmt(
                            "{name}, {value}",
                            &[
                                (
                                    "name",
                                    self.curated_label(row).unwrap_or(&entry.name).to_owned(),
                                ),
                                ("value", self.choice_caption(row, choice, tr)),
                            ],
                        ),
                        frame,
                    );
                    node.checked = Some(*choice == entry.value);
                    node.enabled = enabled && self.writable();
                    node.focused = node.checked == Some(true)
                        && self.focus == Focus::List
                        && self.selected_row() == Some(row)
                        && self.popup.is_none();
                    node.help = entry.description.clone();
                    node
                }
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
                    node.region = Some(Region::Panel);
                    content.push(node);
                    continue;
                }
                Target::Control(row) => {
                    let entry = &self.rows[row];
                    let curated_kind = self.curated(row);
                    let role = match (curated_kind.map(|(kind, ..)| kind), entry.control()) {
                        (Some(Kind::Check), _) | (None, Control::Bool) => Role::CheckBox,
                        (Some(Kind::Popup), _) | (None, Control::Enum) => Role::PopupButton,
                        (Some(Kind::Field | Kind::Stepper | Kind::Tags), _)
                        | (None, Control::Number | Control::Text | Control::Array) => {
                            Role::TextField
                        }
                        _ => continue,
                    };
                    let name = self
                        .curated_label(row)
                        .map_or_else(|| entry.name.clone(), str::to_owned);
                    let label = if role == Role::PopupButton {
                        let value = if curated_kind.is_some() {
                            entry.choice_label(&entry.value, tr)
                        } else {
                            entry.display(tr)
                        };
                        tr.tr_fmt("{name}, {value}", &[("name", name), ("value", value)])
                    } else if entry.control() == Control::Array {
                        tr.tr_fmt(
                            "{name}, {value}",
                            &[("name", name), ("value", tr.tr("Add tag…").into_owned())],
                        )
                    } else {
                        name
                    };
                    let mut node = Node::new(control_id(row), Some(owner(row)), role, label, frame);
                    let edit = self.edit.as_ref().filter(|edit| edit.row == row);
                    node.enabled = self.writable()
                        && entry.editable()
                        && curated_kind.is_none_or(|(_, enabled, _)| enabled)
                        && edit.is_none_or(|edit| !edit.conflict)
                        && !self.conflicts.contains_key(&row);
                    node.help = entry.description.clone();
                    if let Some(effect) = &entry.effect {
                        node.help.push(' ');
                        node.help.push_str(effect);
                    }
                    if role == Role::CheckBox {
                        node.checked = entry.value.as_bool();
                    }
                    if role == Role::PopupButton {
                        node.expanded = Some(
                            self.popup
                                .as_ref()
                                .is_some_and(|popup| popup.kind == PopupKind::Choices(row)),
                        );
                    }
                    if curated && role != Role::TextField {
                        node.focused = self.focus == Focus::List
                            && self.selected_row() == Some(row)
                            && self.popup.is_none();
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
                    Some(owner(row)),
                    Role::Button,
                    tr.tr(if up { "Increase" } else { "Decrease" }),
                    frame,
                ),
                Target::Reset(row) => {
                    let mut node = Node::new(
                        control_id(row) + 3,
                        Some(owner(row)),
                        Role::Button,
                        tr.tr("Reset setting"),
                        frame,
                    );
                    node.enabled = self.writable();
                    node
                }
                Target::RowJson(row) => Node::new(
                    control_id(row) + 4,
                    Some(owner(row)),
                    Role::Link,
                    tr.tr("Edit in JSON"),
                    frame,
                ),
                Target::NewWindow(row) => Node::new(
                    control_id(row) + 12,
                    Some(owner(row)),
                    Role::Button,
                    tr.tr("New Window"),
                    frame,
                ),
                Target::MoreTags(row) => Node::new(
                    control_id(row) + 5,
                    Some(owner(row)),
                    Role::PopupButton,
                    tr.tr("Show all tags"),
                    frame,
                ),
                Target::RemoveTag(row, index) => Node::new(
                    control_id(row) + 6 + index as Id,
                    Some(owner(row)),
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
            if node.parent == Some(AX_LIST) {
                content.push(node);
            } else {
                tree.nodes.push(node);
            }
        }
        content.sort_by(|a, b| {
            (a.frame.y, a.frame.x)
                .partial_cmp(&(b.frame.y, b.frame.x))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        tree.nodes.extend(content);
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
