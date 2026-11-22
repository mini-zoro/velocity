//! Retained minihtml documents, font metrics, popup lifecycle and clipped hit testing.
use crate::images::remote;
use crate::theme::rgba;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
};
use vel_minihtml::{
    DisplayItem, DisplayList, Document, FontMetrics, ImageSource, LayoutOptions, LinkTarget,
    TextMetrics, TextStyle,
};
use vel_platform::{Point, Rect, Size};
use vel_render::scene::{FontStyle, Layer, Scene, TextRun, Underline};

pub use crate::images::remote::enable as enable_remote_images;

/// Text measurement shared by minihtml layout and the renderer's proportional shaping path.
/// The default is deterministic and device-free; native loading is always explicit.
#[derive(Default)]
pub struct Metrics {
    images: crate::images::Images,
    cache: RefCell<HashMap<u64, (String, TextStyle, TextMetrics)>>,
    #[cfg(target_os = "macos")]
    native: RefCell<Option<vel_render::font::Fonts>>,
}
impl std::fmt::Debug for Metrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Metrics")
            .field("cached_runs", &self.cache.borrow().len())
            .finish()
    }
}
impl Metrics {
    /// Installs package image bytes; decoding is asynchronous and never runs in painting.
    pub fn set_image_resources(
        &self,
        resources: std::collections::BTreeMap<String, std::sync::Arc<[u8]>>,
    ) {
        self.images.set_resources(resources);
    }
    /// Polls completed image decodes and reports whether layouts should be invalidated.
    pub fn poll_images(&self) -> bool {
        self.images.poll() | remote::poll()
    }
    /// Whether the host should schedule a bounded worker poll.
    pub fn images_pending(&self) -> bool {
        self.images.pending() || remote::pending()
    }
    /// Whether native text shaping has been explicitly initialized.
    #[cfg(target_os = "macos")]
    pub fn native_enabled(&self) -> bool {
        self.native.borrow().is_some()
    }
    /// Enables the existing native font/fallback stack. Never called by headless modes/tests.
    #[cfg(target_os = "macos")]
    pub fn enable_native(&self) -> Result<(), String> {
        let mut fonts = vel_render::font::Fonts::load()?;
        fonts.shape_ui("M世界 👋", "system-ui", 12., FontStyle::default());
        *self.native.borrow_mut() = Some(fonts);
        self.cache.borrow_mut().clear();
        Ok(())
    }
}
impl FontMetrics for Metrics {
    fn image_size(&self, source: &vel_minihtml::ImageSource) -> Option<vel_minihtml::Size> {
        self.images.get(source).map(|i| vel_minihtml::Size {
            width: i.logical_size[0],
            height: i.logical_size[1],
        })
    }
    fn measure(&self, text: &str, style: &TextStyle) -> TextMetrics {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        style.font_family.hash(&mut h);
        style.font_size.to_bits().hash(&mut h);
        style.font_weight.hash(&mut h);
        style.italic.hash(&mut h);
        let key = h.finish();
        if let Some((cached, font, metrics)) = self.cache.borrow().get(&key)
            && cached == text
            && font.font_family == style.font_family
            && font.font_size == style.font_size
            && font.font_weight == style.font_weight
            && font.italic == style.italic
        {
            return *metrics;
        }
        #[cfg(target_os = "macos")]
        let native = self.native.borrow_mut().as_mut().map(|fonts| {
            let run = fonts.shape_ui(
                text,
                &style.font_family,
                style.font_size,
                FontStyle {
                    bold: style.font_weight >= 600,
                    italic: style.italic,
                },
            );
            TextMetrics {
                advance: run.advance,
                ascent: run.ascent,
                descent: run.descent,
            }
        });
        let estimate = || TextMetrics {
            advance: crate::input::cells(text) as f32 * style.font_size * 0.6,
            ascent: style.font_size * 0.8,
            descent: style.font_size * 0.2,
        };
        #[cfg(target_os = "macos")]
        let metrics = native.unwrap_or_else(estimate);
        #[cfg(not(target_os = "macos"))]
        let metrics = estimate();
        let mut cache = self.cache.borrow_mut();
        if cache.len() >= 4096 {
            cache.clear();
        }
        cache.insert(key, (text.into(), style.clone(), metrics));
        metrics
    }
}
/// Parsed document and cached layout, retained until width, palette, CSS or fonts change.
#[derive(Debug)]
pub struct Html {
    /// Original markup, retained for sheet/session reconstruction.
    pub source: String,
    document: Document,
    images: HashMap<String, std::sync::Arc<vel_render::scene::Image>>,
    remote: Option<Box<RemoteDocument>>,
    key: Option<(u64, u32, u32)>,
    /// Layout display list in document coordinates.
    pub display: DisplayList,
    /// Translated clipping viewport from the last paint.
    pub bounds: Rect,
    /// Intersection with ancestor clips, independent of the document origin.
    pub clip: Rect,
    /// Horizontal/vertical scroll in logical points.
    pub scroll: Point,
}

#[derive(Debug, Default)]
struct RemoteDocument {
    generation: u64,
    requested: HashSet<String>,
}

impl Html {
    /// Parses full minihtml or the restricted completion/quick-panel inline subset.
    pub fn new(source: &str, inline: bool) -> Result<Self, vel_minihtml::Error> {
        let document = if inline {
            Document::parse_inline(source)?
        } else {
            Document::parse(source)?
        };
        Ok(Self {
            source: source.into(),
            document,
            images: HashMap::new(),
            remote: None,
            key: None,
            display: DisplayList::default(),
            bounds: Rect::default(),
            clip: Rect::default(),
            scroll: Point::default(),
        })
    }
    /// Relayouts only when one of the caller's layout inputs changes.
    pub fn layout(&mut self, options: &LayoutOptions, revision: u64, metrics: &Metrics) {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        revision.hash(&mut hash);
        metrics.images.generation.get().hash(&mut hash);
        options.extra_css.hash(&mut hash);
        for (k, v) in &options.palette.variables {
            k.hash(&mut hash);
            v.hash(&mut hash);
        }
        let key = (
            hash.finish(),
            options.max_width.to_bits(),
            options.root_font_size.to_bits(),
        );
        if self.key == Some(key)
            && self
                .remote
                .as_ref()
                .is_none_or(|d| d.generation == remote::generation())
        {
            return;
        }
        self.display = vel_minihtml::layout(&self.document, options, metrics);
        self.images.clear();
        for item in &self.display.items {
            let DisplayItem::Image { source, .. } = item else {
                continue;
            };
            if let ImageSource::Https(url) = source {
                let d = self.remote.get_or_insert_with(Box::default);
                // Do not let cache eviction cause a fetch/repaint loop when a single
                // document shows more images than the shared cache can hold.
                if d.requested.len() < 128 && !d.requested.contains(url) && remote::request(url) {
                    d.requested.insert(url.clone());
                }
            } else if let Some(image) = metrics.images.get(source) {
                self.images.insert(crate::images::key(source).into(), image);
            }
        }
        if let Some(d) = &mut self.remote {
            d.generation = remote::generation();
        }
        self.key = Some(key);
        self.clamp_scroll();
    }
    /// Clamps both axes after resize or document replacement.
    pub fn clamp_scroll(&mut self) {
        self.scroll.x = self.scroll.x.clamp(
            0.,
            (f64::from(self.display.size.width) - self.bounds.width).max(0.),
        );
        self.scroll.y = self.scroll.y.clamp(
            0.,
            (f64::from(self.display.scroll_height) - self.bounds.height).max(0.),
        );
    }
    /// Scrolls a retained popup/sheet without reparsing or relayout.
    pub fn scroll_by(&mut self, x: f64, y: f64) {
        self.scroll.x += x;
        self.scroll.y += y;
        self.clamp_scroll();
    }
    /// Exposes visible popup text and links using the already laid-out display list.
    /// Coordinates include the caller's translation into native content-view points.
    pub fn accessibility_nodes(
        &self,
        base: u64,
        offset: Point,
    ) -> Vec<vel_platform::accessibility::Node> {
        use vel_platform::accessibility::{Node, Role};
        let translate = |r: vel_minihtml::Rect| Rect {
            x: self.bounds.x + f64::from(r.x) - self.scroll.x + offset.x,
            y: self.bounds.y + f64::from(r.y) - self.scroll.y + offset.y,
            width: f64::from(r.width),
            height: f64::from(r.height),
        };
        let clip = Rect {
            x: self.clip.x + offset.x,
            y: self.clip.y + offset.y,
            ..self.clip
        };
        let intersect = |r: Rect| {
            let x = r.x.max(clip.x);
            let y = r.y.max(clip.y);
            let right = (r.x + r.width).min(clip.x + clip.width);
            let bottom = (r.y + r.height).min(clip.y + clip.height);
            (right > x && bottom > y).then_some(Rect {
                x,
                y,
                width: right - x,
                height: bottom - y,
            })
        };
        if clip.width <= 0. || clip.height <= 0. {
            return vec![];
        }
        let mut nodes = vec![Node::new(
            base,
            None,
            Role::Group,
            "Information popup",
            clip,
        )];
        for (i, item) in self.display.items.iter().enumerate() {
            if let DisplayItem::Text { rect, text, .. } = item
                && let Some(frame) = intersect(translate(*rect))
            {
                nodes.push(Node::new(
                    base + 1 + i as u64,
                    Some(base),
                    Role::Status,
                    text.clone(),
                    frame,
                ));
            }
        }
        let link_base = base + 1 + self.display.items.len() as u64;
        for (i, link) in self.display.links.iter().enumerate() {
            if let Some(frame) = intersect(translate(link.rect)) {
                let label = self
                    .display
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        DisplayItem::Text { rect, text, .. }
                            if rect.x < link.rect.x + link.rect.width
                                && rect.x + rect.width > link.rect.x
                                && rect.y < link.rect.y + link.rect.height
                                && rect.y + rect.height > link.rect.y =>
                        {
                            Some(text.as_str())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let mut node = Node::new(
                    link_base + i as u64,
                    Some(base),
                    Role::Link,
                    if label.is_empty() {
                        "Open link".into()
                    } else {
                        label
                    },
                    frame,
                );
                node.activation = Some((
                    Point {
                        x: frame.x + frame.width / 2.,
                        y: frame.y + frame.height / 2.,
                    },
                    false,
                ));
                nodes.push(node);
            }
        }
        nodes
    }
    /// Returns a link only inside the painted clipping viewport.
    pub fn link_at(&self, p: Point) -> Option<&LinkTarget> {
        if !crate::contains(self.clip, p) {
            return None;
        }
        self.display.link_at(
            (p.x - self.bounds.x + self.scroll.x) as f32,
            (p.y - self.bounds.y + self.scroll.y) as f32,
        )
    }
    /// Returns an inherited/overridden HTML title at a translated point.
    pub fn tooltip_at(&self, p: Point) -> Option<&str> {
        if !crate::contains(self.clip, p) {
            return None;
        }
        self.display.tooltip_at(
            (p.x - self.bounds.x + self.scroll.x) as f32,
            (p.y - self.bounds.y + self.scroll.y) as f32,
        )
    }
    /// Paints in display-list order; every item's clip follows the viewport on scroll/resize.
    pub fn paint(&mut self, scene: &mut Scene, bounds: Rect) {
        self.paint_clipped(scene, bounds, bounds);
    }
    /// Paints a document origin through an independent ancestor clip, preserving hit coordinates.
    pub fn paint_clipped(&mut self, scene: &mut Scene, bounds: Rect, ancestor: Rect) {
        let clip = intersection(bounds, ancestor);
        self.clip = clip;
        self.bounds = bounds;
        self.clamp_scroll();
        for item in &self.display.items {
            let r = item.rect();
            let rect = Rect {
                x: bounds.x + f64::from(r.x) - self.scroll.x,
                y: bounds.y + f64::from(r.y) - self.scroll.y,
                width: f64::from(r.width),
                height: f64::from(r.height),
            };
            if rect.y + rect.height <= clip.y
                || rect.y >= clip.y + clip.height
                || rect.x + rect.width <= clip.x
                || rect.x >= clip.x + clip.width
            {
                continue;
            }
            let mut layer = Layer::default();
            match item {
                DisplayItem::Box {
                    background,
                    borders,
                    radii,
                    ..
                } => {
                    layer.surfaces.push(vel_render::scene::Surface::CssBox(
                        vel_render::scene::CssBox {
                            rect,
                            clip,
                            background: rgba(*background),
                            radii: *radii,
                            borders: borders.map(|b| vel_render::scene::BorderEdge {
                                width: b.width,
                                color: rgba(b.color),
                            }),
                        },
                    ));
                }
                DisplayItem::Text {
                    text,
                    style,
                    baseline,
                    ..
                } => {
                    layer.text.push(TextRun {
                        family: Some(style.font_family.clone()),
                        style: FontStyle {
                            bold: style.font_weight >= 600,
                            italic: style.italic,
                        },
                        text: text.clone(),
                        x: rect.x as f32,
                        baseline: rect.y as f32 + *baseline,
                        size: style.font_size,
                        color: rgba(style.color),
                        highlight: None,
                        clip,
                    });
                    if style.underline {
                        layer.underlines.push(Underline {
                            stippled: false,
                            rect: Rect {
                                y: rect.y + f64::from(*baseline) + 1.,
                                height: 1.,
                                ..rect
                            },
                            thickness: 1.,
                            squiggle: false,
                            color: rgba(style.color),
                            clip,
                        });
                    }
                }
                DisplayItem::Image { source, .. } => {
                    let image = match source {
                        ImageSource::Https(url) => remote::get(url),
                        _ => self.images.get(crate::images::key(source)).cloned(),
                    };
                    if let Some(image) = image {
                        layer.surfaces.push(vel_render::scene::Surface::Image(
                            vel_render::scene::ImageQuad {
                                source: [0., 0., image.width as f32, image.height as f32],
                                image,
                                rect,
                                clip,
                                color: [1.; 4],
                            },
                        ));
                    } else {
                        crate::fill(&mut layer, rect, clip, [0.5, 0.5, 0.5, 0.2]);
                    }
                }
            }
            scene.layers.push(layer);
        }
    }
}
/// Documented popup bit flags, usable without a plugin runtime.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PopupFlags(pub u32);
impl PopupFlags {
    /// Position beside the completion menu; otherwise the menu suppresses the popup.
    pub const COOPERATE_WITH_AUTO_COMPLETE: Self = Self(2);
    /// Hide on mouse move/click/scroll.
    pub const HIDE_ON_MOUSE_MOVE: Self = Self(4);
    /// Hide on movement away, preserving motion towards/inside the popup.
    pub const HIDE_ON_MOUSE_MOVE_AWAY: Self = Self(8);
    /// Retain when the selection changes.
    pub const KEEP_ON_SELECTION_MODIFIED: Self = Self(16);
    /// Hide on committed character input.
    pub const HIDE_ON_CHARACTER_EVENT: Self = Self(32);
    /// Tests one or more flags.
    pub fn contains(self, flag: Self) -> bool {
        self.0 & flag.0 == flag.0
    }
}
/// Transient minihtml popup. The application supplies text-location geometry each frame.
#[derive(Debug)]
pub struct Popup {
    /// Retained document/layout/scroll.
    pub html: Html,
    /// Lifecycle rules.
    pub flags: PopupFlags,
    /// Buffer byte location; None uses the current caret.
    pub location: Option<usize>,
    /// Content maximum dimensions.
    pub max_size: Size,
    /// Whether still open.
    pub visible: bool,
    last_mouse: Option<Point>,
}
impl Popup {
    /// Constructs a popup, with parse errors returned before replacing an existing one.
    pub fn new(
        content: &str,
        flags: PopupFlags,
        location: Option<usize>,
        max_size: Size,
    ) -> Result<Self, vel_minihtml::Error> {
        Ok(Self {
            html: Html::new(content, false)?,
            flags,
            location,
            max_size,
            visible: true,
            last_mouse: None,
        })
    }
    /// Hides on a selection change unless explicitly retained.
    pub fn selection_modified(&mut self) {
        if !self.flags.contains(PopupFlags::KEEP_ON_SELECTION_MODIFIED) {
            self.visible = false;
        }
    }
    /// Hides on character input only when requested.
    pub fn character_event(&mut self) {
        if self.flags.contains(PopupFlags::HIDE_ON_CHARACTER_EVENT) {
            self.visible = false;
        }
    }
    /// Updates visibility from actual mouse motion; movement towards the box may cross its gap.
    pub fn mouse_move(&mut self, p: Point) {
        let distance = |p: Point| {
            let r = self.html.bounds;
            let dx = (r.x - p.x).max(0.).max(p.x - r.x - r.width);
            let dy = (r.y - p.y).max(0.).max(p.y - r.y - r.height);
            dx * dx + dy * dy
        };
        if let Some(old) = self.last_mouse
            && old != p
            && (self.flags.contains(PopupFlags::HIDE_ON_MOUSE_MOVE)
                || (self.flags.contains(PopupFlags::HIDE_ON_MOUSE_MOVE_AWAY)
                    && distance(p) > distance(old)))
        {
            self.visible = false;
        }
        self.last_mouse = Some(p);
    }
    /// Applies click/scroll dismissal before ordinary view event routing.
    pub fn mouse_action(&mut self) {
        if self.flags.0 & 12 != 0 {
            self.visible = false;
        }
    }
    /// Positions beside completions when requested, then clamps/flips within the view.
    pub fn place(
        &mut self,
        anchor: Rect,
        viewport: Rect,
        completion: Option<Rect>,
    ) -> Option<Rect> {
        if !self.visible {
            return None;
        }
        let mut width = self.max_size.width.max(1.).min(viewport.width);
        let anchor = if let Some(menu) = completion {
            if !self
                .flags
                .contains(PopupFlags::COOPERATE_WITH_AUTO_COMPLETE)
            {
                return None;
            }
            let right = (viewport.x + viewport.width - menu.x - menu.width).max(0.);
            let left = (menu.x - viewport.x).max(0.);
            let x = if right >= left {
                width = width.min(right.max(1.));
                menu.x + menu.width
            } else {
                width = width.min(left.max(1.));
                menu.x - width
            };
            Rect {
                x,
                y: menu.y,
                width: 0.,
                height: 0.,
            }
        } else {
            anchor
        };
        let height = f64::from(self.html.display.scroll_height)
            .min(self.max_size.height.max(1.))
            .min(viewport.height);
        let x = anchor.x.clamp(
            viewport.x,
            (viewport.x + viewport.width - width).max(viewport.x),
        );
        let below = anchor.y + anchor.height;
        let y = if below + height <= viewport.y + viewport.height {
            below
        } else {
            anchor.y - height
        };
        let rect = Rect {
            x,
            y: y.clamp(
                viewport.y,
                (viewport.y + viewport.height - height).max(viewport.y),
            ),
            width,
            height,
        };
        self.html.bounds = rect;
        self.html.clamp_scroll();
        Some(rect)
    }
}
/// Phantom placement relative to a tracked buffer region beginning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhantomLayout {
    /// Insert horizontally at the region beginning, reserving space in text flow.
    Inline,
    /// Reserve rows below the containing line, aligned with the region end.
    Below,
    /// Reserve rows below the containing line, aligned with the text margin.
    Block,
}
/// Retained phantom content; region tracking belongs to the editor's buffer region store.
#[derive(Debug)]
pub struct Phantom {
    /// Stable set-local identity.
    pub id: u64,
    /// Buffer byte region beginning.
    pub at: usize,
    /// Flow placement.
    pub layout: PhantomLayout,
    /// Parsed minihtml.
    pub html: Html,
}
/// Annotation at a region end, aligned with the viewport's right edge.
#[derive(Debug)]
pub struct Annotation {
    /// Buffer byte region end.
    pub at: usize,
    /// Parsed content.
    pub html: Html,
    /// Optional CSS left-border color.
    pub color: Option<vel_theme::Color>,
}
impl Annotation {
    /// Paints on the region's visual line, without reserving phantom rows.
    pub fn paint(&mut self, scene: &mut Scene, end: Rect, viewport: Rect) {
        paint_annotation(&mut self.html, self.color, scene, end, viewport);
    }
}
/// Paints retained HTML as a right-aligned annotation, with an optional CSS left border.
pub fn paint_annotation(
    html: &mut Html,
    color: Option<vel_theme::Color>,
    scene: &mut Scene,
    end: Rect,
    viewport: Rect,
) {
    let width = (html
        .display
        .items
        .iter()
        .map(|i| i.rect().x + i.rect().width)
        .fold(0., f32::max) as f64)
        .min(viewport.width * 0.6)
        .max(1.);
    let rect = Rect {
        x: (viewport.x + viewport.width - width).max(end.x + end.width + 8.),
        y: end.y,
        width,
        height: end.height.min(f64::from(html.display.scroll_height)),
    };
    let clip = intersection(rect, viewport);
    if let Some(color) = color {
        let mut layer = Layer::default();
        crate::fill(
            &mut layer,
            Rect {
                x: clip.x - 3.,
                width: 2.,
                ..clip
            },
            viewport,
            rgba(color),
        );
        scene.layers.push(layer);
    }
    html.paint_clipped(scene, rect, viewport);
}
/// Intersects two clipping rectangles, preserving finite nonnegative extents.
pub fn intersection(a: Rect, b: Rect) -> Rect {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    Rect {
        x,
        y,
        width: ((a.x + a.width).min(b.x + b.width) - x).max(0.),
        height: ((a.y + a.height).min(b.y + b.height) - y).max(0.),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_layout_scroll_clipping_and_navigation_share_document_coordinates() {
        let metrics = Metrics::default();
        let mut html=Html::new("<div style='height:50px'>above</div><a title='Open definition' href='subl:goto_line {\"line\":7}'>世界 &amp; result</a>",false).unwrap();
        let options = LayoutOptions {
            max_width: 180.,
            ..Default::default()
        };
        html.layout(&options, 1, &metrics);
        let pointer = html.display.items.as_ptr();
        html.layout(&options, 1, &metrics);
        assert_eq!(pointer, html.display.items.as_ptr());
        let bounds = Rect {
            x: 40.,
            y: 80.,
            width: 180.,
            height: 30.,
        };
        let mut scene = Scene::default();
        html.paint(&mut scene, bounds);
        html.scroll_by(0., 45.);
        html.paint_clipped(&mut scene, bounds, Rect { x: 50., ..bounds });
        let link = &html.display.links[0];
        let point = Point {
            x: (bounds.x + f64::from(link.rect.x) + 3.).max(51.),
            y: bounds.y + f64::from(link.rect.y) - html.scroll.y + 3.,
        };
        assert!(
            matches!(html.link_at(point),Some(LinkTarget::Command{name,args}) if name=="goto_line" && args["line"]==7)
        );
        assert_eq!(html.tooltip_at(point), Some("Open definition"));
        assert!(html.link_at(Point { x: 49., y: point.y }).is_none());
        assert!(
            scene
                .layers
                .iter()
                .flat_map(|l| &l.text)
                .all(|t| t.clip.x >= 40.)
        );
    }
    #[test]
    fn popup_flags_completion_cooperation_and_directional_mouse_dismissal() {
        let flags = PopupFlags(
            PopupFlags::KEEP_ON_SELECTION_MODIFIED.0
                | PopupFlags::HIDE_ON_MOUSE_MOVE_AWAY.0
                | PopupFlags::HIDE_ON_CHARACTER_EVENT.0,
        );
        let mut popup = Popup::new(
            "<b>Hover</b>",
            flags,
            None,
            Size {
                width: 180.,
                height: 60.,
            },
        )
        .unwrap();
        popup
            .html
            .layout(&LayoutOptions::default(), 0, &Metrics::default());
        let viewport = Rect {
            x: 0.,
            y: 0.,
            width: 300.,
            height: 200.,
        };
        let anchor = Rect {
            x: 10.,
            y: 100.,
            width: 1.,
            height: 20.,
        };
        let rect = popup.place(anchor, viewport, None).unwrap();
        popup.selection_modified();
        assert!(popup.visible);
        popup.mouse_move(Point { x: 10., y: 100. });
        popup.mouse_move(Point { x: 20., y: 110. });
        assert!(popup.visible);
        popup.mouse_move(Point {
            x: rect.x + 5.,
            y: rect.y + 5.,
        });
        assert!(popup.visible);
        popup.mouse_move(Point { x: 290., y: 5. });
        assert!(!popup.visible);
        popup.visible = true;
        assert!(popup.place(anchor, viewport, Some(anchor)).is_none());
        popup.flags = PopupFlags::COOPERATE_WITH_AUTO_COMPLETE;
        let menu = Rect {
            x: 80.,
            y: 20.,
            width: 140.,
            height: 100.,
        };
        let cooperative = popup.place(anchor, viewport, Some(menu)).unwrap();
        assert!(
            cooperative.x + cooperative.width <= menu.x || cooperative.x >= menu.x + menu.width
        );
        assert!(
            cooperative.x >= viewport.x
                && cooperative.x + cooperative.width <= viewport.x + viewport.width
        );
        popup.flags = flags;
        popup.character_event();
        assert!(!popup.visible);
        popup.visible = true;
        popup.mouse_action();
        assert!(!popup.visible);
    }
    #[test]
    fn multiline_text_baselines_follow_their_own_rectangles() {
        let metrics = Metrics::default();
        let mut html = Html::new("<p>First paragraph</p><p>Second paragraph</p>", false).unwrap();
        html.layout(&LayoutOptions::default(), 0, &metrics);
        let mut scene = Scene::default();
        let bounds = Rect {
            x: 20.,
            y: 40.,
            width: 480.,
            height: 200.,
        };
        html.paint(&mut scene, bounds);
        let expected = html
            .display
            .items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text {
                    rect,
                    baseline,
                    text,
                    ..
                } => Some((text, rect.y + baseline + 40.)),
                _ => None,
            })
            .collect::<Vec<_>>();
        let actual = scene
            .layers
            .iter()
            .flat_map(|l| &l.text)
            .map(|r| (&r.text, r.baseline))
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
        assert!(actual.last().unwrap().1 > actual.first().unwrap().1 + 10.);
    }
    #[test]
    fn image_decode_is_async_then_invalidates_layout_and_paints_real_pixels() {
        let metrics = Metrics::default();
        let mut html=Html::new("<img src='data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7'>",false).unwrap();
        html.layout(&LayoutOptions::default(), 0, &metrics);
        assert!(metrics.images_pending());
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while metrics.images_pending() && std::time::Instant::now() < until {
            metrics.poll_images();
            std::thread::park_timeout(std::time::Duration::from_millis(1));
        }
        html.layout(&LayoutOptions::default(), 0, &metrics);
        let mut scene = Scene::default();
        html.paint(
            &mut scene,
            Rect {
                x: 0.,
                y: 0.,
                width: 300.,
                height: 200.,
            },
        );
        assert!(scene.layers.iter().flat_map(|l|&l.surfaces).any(|s|matches!(s,vel_render::scene::Surface::Image(q) if q.image.width==1 && q.image.height==1)));
    }
}

#[cfg(test)]
mod accessibility_tests {
    use super::*;
    #[test]
    fn visible_popup_links_have_spoken_names_and_clipped_activation_geometry() {
        let mut html = Html::new(
            "<p>Information <a href='https://example.invalid/'>Open help</a></p>",
            false,
        )
        .unwrap();
        html.layout(&LayoutOptions::default(), 0, &Metrics::default());
        let mut scene = Scene::default();
        let bounds = Rect {
            x: 20.,
            y: 30.,
            width: 300.,
            height: 80.,
        };
        html.paint(&mut scene, bounds);
        let nodes = html.accessibility_nodes(100, Point { x: 50., y: 60. });
        let link = nodes
            .iter()
            .find(|n| n.role == vel_platform::accessibility::Role::Link)
            .unwrap();
        assert!(link.label.contains("Open"));
        let point = link.activation.unwrap().0;
        assert!(link.frame.x >= 70. && link.frame.y >= 90.);
        assert!(
            html.link_at(Point {
                x: point.x - 50.,
                y: point.y - 60.
            })
            .is_some()
        );
    }
}
