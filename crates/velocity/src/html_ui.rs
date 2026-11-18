//! Per-view minihtml surfaces and tracked regions; no native/GPU initialization.
use crate::{editor::Editor, view::View};
use std::{
    collections::{BTreeMap, HashMap},
    hash::{Hash, Hasher},
};
use vel_config::{Value, json};
use vel_minihtml::{LayoutOptions, LinkTarget};
use vel_platform::{Point, Rect, Size};
use vel_render::scene::{Layer, Scene};
use vel_theme::{Element, ElementPath};
use vel_ui::minihtml::{Annotation, Html, Phantom, PhantomLayout, Popup, PopupFlags};
pub const COMMANDS: &[&str] = &[
    "show_scope_name",
    "copy_scope_name",
    "show_popup",
    "update_popup",
    "hide_popup",
    "add_phantoms",
    "erase_phantoms",
    "add_annotations",
    "add_regions",
    "erase_annotations",
];
#[derive(Debug, Default)]
pub struct ViewHtml {
    pub popup: Option<Popup>,
    pub phantoms: BTreeMap<String, Vec<Phantom>>,
    pub annotations: BTreeMap<String, Vec<Annotation>>,
    pub sheet: Option<Html>,
    pub cache: HashMap<u64, Html>,
    pub tooltip: Option<(Point, String)>,
}
impl ViewHtml {
    pub fn begin_frame(&mut self) {
        for h in self.cache.values_mut() {
            h.clip = Rect::default();
        }
        for p in self.phantoms.values_mut().flatten() {
            p.html.clip = Rect::default();
        }
        for a in self.annotations.values_mut().flatten() {
            a.html.clip = Rect::default();
        }
    }
    pub fn cached(&mut self, source: &str, inline: bool) -> Result<&mut Html, String> {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut hash);
        inline.hash(&mut hash);
        let key = hash.finish();
        if self.cache.len() > 512 {
            self.cache.clear();
        }
        if self.cache.get(&key).is_none_or(|h| h.source != source) {
            self.cache
                .insert(key, Html::new(source, inline).map_err(|e| e.to_string())?);
        }
        Ok(self.cache.get_mut(&key).unwrap())
    }
    pub fn selection_modified(&mut self) {
        if let Some(p) = &mut self.popup {
            p.selection_modified();
        }
    }
    pub fn character_event(&mut self) {
        if let Some(p) = &mut self.popup {
            p.character_event();
        }
    }
    pub fn mouse_move(&mut self, point: Point) {
        if let Some(p) = &mut self.popup {
            p.mouse_move(point);
        }
        self.tooltip = self
            .popup
            .as_ref()
            .filter(|p| p.visible)
            .and_then(|p| p.html.tooltip_at(point))
            .or_else(|| self.sheet.as_ref().and_then(|h| h.tooltip_at(point)))
            .or_else(|| self.cache.values().find_map(|h| h.tooltip_at(point)))
            .or_else(|| {
                self.phantoms
                    .values()
                    .flatten()
                    .find_map(|p| p.html.tooltip_at(point))
            })
            .or_else(|| {
                self.annotations
                    .values()
                    .flatten()
                    .find_map(|a| a.html.tooltip_at(point))
            })
            .map(|s| (point, s.into()));
    }
    pub fn navigate(&self, point: Point) -> Option<LinkTarget> {
        self.popup
            .as_ref()
            .filter(|p| p.visible)
            .and_then(|p| p.html.link_at(point))
            .or_else(|| self.sheet.as_ref().and_then(|h| h.link_at(point)))
            .or_else(|| self.cache.values().find_map(|h| h.link_at(point)))
            .or_else(|| {
                self.annotations
                    .values()
                    .flatten()
                    .find_map(|a| a.html.link_at(point))
            })
            .or_else(|| {
                self.phantoms
                    .values()
                    .flatten()
                    .find_map(|p| p.html.link_at(point))
            })
            .cloned()
    }
    pub fn scroll(&mut self, point: Point, x: f64, y: f64) -> bool {
        if let Some(p) = self.popup.as_mut().filter(|p| p.visible) {
            if vel_ui::contains(p.html.bounds, point) {
                if p.flags.0 & 12 != 0 {
                    p.mouse_action();
                } else {
                    p.html.scroll_by(x, y);
                }
                return true;
            }
            p.mouse_action();
        }
        if let Some(sheet) = self.sheet.as_mut() {
            sheet.scroll_by(x, y);
            return true;
        }
        false
    }
}
pub fn invocation(target: LinkTarget) -> vel_config::keymap::Invocation {
    match target {
        LinkTarget::Command { name, args } => vel_config::keymap::Invocation {
            command: name,
            args,
        },
        LinkTarget::Browser(url) => vel_config::keymap::Invocation {
            command: "open_url".into(),
            args: json!({"url":url}),
        },
        LinkTarget::Callback(url) => vel_config::keymap::Invocation {
            command: "minihtml_navigate".into(),
            args: json!({"href":url}),
        },
    }
}
impl Editor {
    pub fn html_command(&mut self, name: &str, args: &Value) -> Result<bool, String> {
        if name == "add_regions" {
            let regions = args["regions"]
                .as_array()
                .ok_or("regions must be an array")?;
            let annotations = args["annotations"]
                .as_array()
                .ok_or("annotations must be an array")?;
            if regions.len() != annotations.len() {
                return Err("one annotation is required per region".into());
            }
            let mut pairs = Vec::new();
            for (region, content) in regions.iter().zip(annotations) {
                let a = region[0].as_u64().ok_or("invalid region begin")? as usize;
                let b = region[1].as_u64().ok_or("invalid region end")? as usize;
                if a > self.text.len_bytes()
                    || b > self.text.len_bytes()
                    || !self.text.is_char_boundary(a)
                    || !self.text.is_char_boundary(b)
                {
                    return Err("invalid annotation region".into());
                }
                pairs.push((vel_buffer::Region::new(a, b), content.clone()));
            }
            pairs.sort_by_key(|(r, _)| r.begin());
            let items = pairs
                .iter()
                .map(|(r, c)| json!({"at":r.end(),"content":c}))
                .collect::<Vec<_>>();
            let key = args["key"].as_str().unwrap_or("default");
            self.html_command(
                "add_annotations",
                &json!({"key":key,"items":items,"annotation_color":args["annotation_color"]}),
            )?;
            self.buffer.add_regions(
                self.id,
                format!("velocity.annotations.{key}"),
                pairs.into_iter().map(|(r, _)| r).collect(),
                false,
            );
            return Ok(true);
        }
        let key = args["key"].as_str().unwrap_or("default");
        match name {
            "show_scope_name" => {
                // https://www.sublimetext.com/3 (3114): display the scope in a popup.
                let content = self.scope_popup_content();
                return self
                    .html_command("show_popup", &json!({"content":content,"max_width":480}));
            }
            "copy_scope_name" => {
                self.clipboard = args["scopes"]
                    .as_str()
                    .ok_or("missing scope string")?
                    .into();
                self.html.borrow_mut().popup = None;
            }
            "show_popup" => {
                let popup = Popup::new(
                    args["content"].as_str().unwrap_or(""),
                    PopupFlags(args["flags"].as_u64().unwrap_or(0) as u32),
                    Some(args["location"].as_u64().map_or(self.caret, |n| n as usize)),
                    Size {
                        width: args["max_width"].as_f64().unwrap_or(320.).clamp(1., 4096.),
                        height: args["max_height"].as_f64().unwrap_or(240.).clamp(1., 4096.),
                    },
                )
                .map_err(|e| e.to_string())?;
                self.html.borrow_mut().popup = Some(popup);
            }
            "update_popup" => {
                let html = Html::new(args["content"].as_str().unwrap_or(""), false)
                    .map_err(|e| e.to_string())?;
                if let Some(p) = &mut self.html.borrow_mut().popup {
                    p.html = html;
                }
            }
            "hide_popup" => self.html.borrow_mut().popup = None,
            "erase_phantoms" => {
                self.html.borrow_mut().phantoms.remove(key);
                self.buffer
                    .erase_regions(self.id, &format!("velocity.phantoms.{key}"));
            }
            "erase_annotations" => {
                self.html.borrow_mut().annotations.remove(key);
                self.buffer
                    .erase_regions(self.id, &format!("velocity.annotations.{key}"));
            }
            "add_phantoms" | "add_annotations" => {
                let entries = args["items"]
                    .as_array()
                    .ok_or("minihtml items must be an array")?;
                let mut regions = Vec::new();
                let mut phantoms = Vec::new();
                let mut annotations = Vec::new();
                for (id, item) in entries.iter().enumerate() {
                    let at = item["region"][0]
                        .as_u64()
                        .or_else(|| item["at"].as_u64())
                        .unwrap_or(self.caret as u64) as usize;
                    if at > self.text.len_bytes() || !self.text.is_char_boundary(at) {
                        return Err("minihtml location is not a buffer character boundary".into());
                    }
                    let html = Html::new(item["content"].as_str().unwrap_or(""), false)
                        .map_err(|e| e.to_string())?;
                    let end = item["region"][1].as_u64().map_or(at, |n| n as usize);
                    if end > self.text.len_bytes() || !self.text.is_char_boundary(end) {
                        return Err("invalid phantom region end".into());
                    }
                    regions.push(vel_buffer::Region::new(at, end));
                    if name == "add_phantoms" {
                        let layout = match item["layout"].as_str().unwrap_or("inline") {
                            "inline" => PhantomLayout::Inline,
                            "below" => PhantomLayout::Below,
                            "block" => PhantomLayout::Block,
                            _ => return Err("unknown phantom layout".into()),
                        };
                        phantoms.push(Phantom {
                            id: id as u64,
                            at,
                            layout,
                            html,
                        });
                    } else {
                        let color = item["annotation_color"]
                            .as_str()
                            .or_else(|| args["annotation_color"].as_str())
                            .and_then(|c| self.theme.borrow().palette.color(c).ok());
                        annotations.push(Annotation { at, html, color });
                    }
                }
                phantoms.sort_by_key(|p| p.at);
                annotations.sort_by_key(|a| a.at);
                let kind = if name == "add_phantoms" {
                    "phantoms"
                } else {
                    "annotations"
                };
                self.buffer
                    .add_regions(self.id, format!("velocity.{kind}.{key}"), regions, false);
                if name == "add_phantoms" {
                    self.html.borrow_mut().phantoms.insert(key.into(), phantoms);
                } else {
                    self.html
                        .borrow_mut()
                        .annotations
                        .insert(key.into(), annotations);
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn scope_popup_content(&self) -> String {
        use vel_highlight::ScopeProvider;
        let language = self.language.borrow();
        let snapshot = language
            .as_ref()
            .and_then(|l| l.scope_snapshot_at(self.caret));
        let Some(scopes) = snapshot.as_ref().and_then(|s| s.scopes_at(self.caret)) else {
            return "<p>Scopes not ready yet</p>".into();
        };
        let scopes: Vec<_> = scopes.iter().map(|s| s.build_string()).collect();
        let escape = |s: &str| {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('\'', "&#39;")
        };
        let body = scopes
            .iter()
            .map(|s| escape(s))
            .collect::<Vec<_>>()
            .join("<br>");
        let args = escape(&json!({"scopes":scopes.join(" ")}).to_string());
        format!(
            "<p><code>{body}</code></p><p><a href='subl:copy_scope_name {args}'>Copy scope</a></p>"
        )
    }
}
pub fn options(editor: &Editor, width: f64, popup: bool) -> LayoutOptions {
    let theme = editor.theme.borrow();
    let mut css = editor
        .language
        .borrow()
        .as_ref()
        .and_then(|l| {
            l.scheme
                .globals
                .value(if popup { "popup_css" } else { "phantom_css" })
        })
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if popup {
        // Default inset so popup text never touches the popup edge; the scheme's
        // popup_css (appended after) can still override it.
        css.insert_str(0, "html{padding:5px 8px}");
    } else {
        css.push_str("html{background-color:transparent}body{margin:0;padding:0}");
    }
    LayoutOptions {
        max_width: width.max(1.) as f32,
        root_font_size: editor.preferences.font_size as f32,
        palette: theme.palette.clone(),
        extra_css: css,
    }
}
/// Measures phantom space before text layout; tracked regions follow edits/undo/redo.
type PhantomSpace = (Vec<(usize, usize)>, Vec<(usize, usize)>);
pub fn prepare(editor: &Editor, width: f64, cell: f64, line_height: f64) -> PhantomSpace {
    let mut html = editor.html.borrow_mut();
    if html.phantoms.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let options = options(editor, width, false);
    let theme = editor.theme.borrow();
    let mut rows = Vec::new();
    let mut inline = Vec::new();
    for (key, set) in &mut html.phantoms {
        let regions = editor
            .buffer
            .view(editor.id)
            .get_regions(&format!("velocity.phantoms.{key}"));
        for (i, phantom) in set.iter_mut().enumerate() {
            if let Some(r) = regions.and_then(|r| r.regions.get(i)) {
                phantom.at = r.begin();
            }
            phantom
                .html
                .layout(&options, theme.revision, &theme.metrics);
            let row = editor
                .text
                .byte_to_line(phantom.at.min(editor.text.len_bytes()));
            let height =
                (f64::from(phantom.html.display.scroll_height) / line_height).ceil() as usize;
            match phantom.layout {
                PhantomLayout::Inline => {
                    let width = phantom
                        .html
                        .display
                        .items
                        .iter()
                        .map(|i| i.rect().x + i.rect().width)
                        .fold(0., f32::max);
                    inline.push((phantom.at, (f64::from(width) / cell).ceil() as usize));
                    if height > 1 {
                        rows.push((row + 1, height - 1));
                    }
                }
                _ => rows.push((row + 1, height.max(1))),
            }
        }
    }
    inline.sort_unstable();
    rows.sort_unstable();
    (rows, inline)
}
pub fn paint(editor: &Editor, view: &View, scene: &mut Scene) {
    {
        let html = editor.html.borrow();
        if html.popup.as_ref().is_none_or(|p| !p.visible)
            && html.phantoms.is_empty()
            && html.annotations.is_empty()
            && html.tooltip.is_none()
        {
            return;
        }
    }
    let viewport = Rect {
        x: view.gutter(editor),
        y: view.tab_height,
        width: (view.right(editor) - view.gutter(editor)).max(1.),
        height: (view.size.height - view.tab_height).max(1.),
    };
    let annotation_options = options(editor, viewport.width, false);
    let mut html = editor.html.borrow_mut();
    let theme = editor.theme.borrow();
    let mut offsets: HashMap<usize, f64> = HashMap::new();
    for set in html.phantoms.values_mut() {
        for phantom in set {
            let end = view.rect_at(editor, phantom.at.min(editor.text.len_bytes()));
            let row = editor
                .text
                .byte_to_line(phantom.at.min(editor.text.len_bytes()));
            let y = if phantom.layout == PhantomLayout::Inline {
                end.y
            } else {
                end.y + view.line_height + *offsets.entry(row).or_default()
            };
            let x = if phantom.layout == PhantomLayout::Block {
                viewport.x + 8.
            } else {
                end.x
            };
            let bounds = Rect {
                x,
                y,
                width: (viewport.x + viewport.width - x).max(1.),
                height: f64::from(phantom.html.display.scroll_height),
            };

            if phantom.layout != PhantomLayout::Inline {
                *offsets.entry(row).or_default() += f64::from(phantom.html.display.scroll_height);
            }
            phantom.html.paint_clipped(scene, bounds, viewport);
        }
    }
    for (key, set) in &mut html.annotations {
        let regions = editor
            .buffer
            .view(editor.id)
            .get_regions(&format!("velocity.annotations.{key}"));
        for (i, annotation) in set.iter_mut().enumerate() {
            if let Some(r) = regions.and_then(|r| r.regions.get(i)) {
                annotation.at = r.end();
            }
            let end = view.rect_at(editor, annotation.at.min(editor.text.len_bytes()));
            if end.y + end.height <= viewport.y || end.y >= viewport.y + viewport.height {
                annotation.html.bounds = Rect::default();
                continue;
            }
            annotation
                .html
                .layout(&annotation_options, theme.revision, &theme.metrics);
            annotation.paint(scene, end, viewport);
        }
    }
    drop(theme);
    if let Some(popup) = html.popup.as_mut().filter(|p| p.visible) {
        let options = options(editor, popup.max_size.width.min(viewport.width), true);
        let mut theme = editor.theme.borrow_mut();
        popup.html.layout(&options, theme.revision, &theme.metrics);
        let anchor = view.rect_at(
            editor,
            popup
                .location
                .unwrap_or(editor.caret)
                .min(editor.text.len_bytes()),
        );
        if let Some(mut rect) = popup.place(anchor, viewport, editor.completion.popup_rect()) {
            if (rect.width - f64::from(options.max_width)).abs() > 0.5 {
                let mut narrow = options.clone();
                narrow.max_width = rect.width.max(1.) as f32;
                popup.html.layout(&narrow, theme.revision, &theme.metrics);
                if let Some(resized) = popup.place(anchor, viewport, editor.completion.popup_rect())
                {
                    rect = resized;
                }
            }
            let style = theme.style(
                editor.ui_id(0xA000),
                &ElementPath(&[
                    Element::new("window", &[]),
                    Element::new("popup_control html_popup", &[]),
                ]),
                &editor.settings,
            );
            let mut layer = Layer::default();
            theme.surface(&mut layer, &style, rect, viewport);
            scene.layers.push(layer);
            popup.html.paint(scene, rect);
            // The html background covers the surface; keep the popup's edge visible.
            let mut edge = Layer::default();
            theme.border(&mut edge, &style, rect, viewport);
            scene.layers.push(edge);
        }
    }
    if let Some((point, text)) = &html.tooltip {
        let mut theme = editor.theme.borrow_mut();
        let style = theme.style(
            editor.ui_id(0xA001),
            &ElementPath(&[
                Element::new("window", &[]),
                Element::new("tool_tip_control", &[]),
            ]),
            &editor.settings,
        );
        let bounds = Rect {
            x: point.x.min((view.size.width - 260.).max(0.)),
            y: (point.y + 20.).min((view.size.height - 28.).max(view.tab_height)),
            width: 260f64.min(view.size.width),
            height: 28.,
        };
        // The label is its own element so the container's shadow is not applied to text.
        let label = theme.style(
            editor.ui_id(0xA002),
            &ElementPath(&[
                Element::new("window", &[]),
                Element::new("tool_tip_control", &[]),
                Element::new("tool_tip_label_control", &[]),
            ]),
            &editor.settings,
        );
        let mut layer = Layer::default();
        theme.surface(&mut layer, &style, bounds, viewport);
        theme.text(
            &mut layer,
            &label,
            text,
            bounds.x + 6.,
            bounds.y + 18.,
            bounds,
        );
        scene.layers.push(layer);
    }
}
pub fn sheet(editor: &Editor, view: &View) -> Option<Scene> {
    let source = editor
        .settings
        .get("velocity_html_sheet")
        .and_then(Value::as_str)?;
    let options = options(editor, view.size.width, true);
    let theme = editor.theme.borrow();
    let mut html = editor.html.borrow_mut();
    if html.sheet.as_ref().is_none_or(|h| h.source != source) {
        html.sheet = Html::new(source, false).ok();
    }
    let sheet = html.sheet.as_mut()?;
    sheet.layout(&options, theme.revision, &theme.metrics);
    let mut scene = Scene {
        background: vel_ui::theme::rgba(
            theme
                .palette
                .color("var(--background)")
                .unwrap_or(vel_theme::Color::rgb(20, 35, 46)),
        ),
        layers: Vec::new(),
    };
    sheet.paint(
        &mut scene,
        Rect {
            x: 0.,
            y: view.tab_height,
            width: view.size.width,
            height: (view.size.height - view.tab_height).max(1.),
        },
    );
    Some(scene)
}

pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\"', "&quot;")
        .replace('\'', "&#39;")
}

pub fn paint_build_errors<'a>(
    editor: &Editor,
    view: &View,
    diagnostics: impl Iterator<Item = &'a vel_build::Diagnostic>,
    scene: &mut Scene,
) {
    if !editor.settings.bool("show_errors_inline", true) {
        return;
    }
    let clip = Rect {
        x: view.gutter(editor),
        y: view.tab_height,
        width: (view.right(editor) - view.gutter(editor)).max(1.),
        height: (view.size.height - view.tab_height).max(0.),
    };
    for d in diagnostics {
        let row = d.line.saturating_sub(1).min(editor.text.len_lines() - 1);
        let at = editor.text.line_to_byte(row)
            + editor
                .text
                .line(row)
                .to_string()
                .trim_end_matches('\n')
                .len();
        let end = view.rect_at(editor, at);
        if end.y < view.tab_height || end.y >= view.size.height {
            continue;
        }
        let source = format!(
            "<span style='padding:2px 6px;color:var(--redish)' title='Build diagnostic'>{}</span>",
            escape(&d.message)
        );
        let mut html = editor.html.borrow_mut();
        let options = options(editor, clip.width * 0.55, false);
        let theme = editor.theme.borrow();
        if let Ok(content) = html.cached(&source, false) {
            content.layout(&options, theme.revision, &theme.metrics);
            vel_ui::minihtml::paint_annotation(
                content,
                theme.palette.color("var(--redish)").ok(),
                scene,
                end,
                clip,
            );
        }
    }
}
