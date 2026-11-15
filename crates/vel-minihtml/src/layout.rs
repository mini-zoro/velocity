//! Block/inline flow with baseline alignment and renderer-independent geometry.
use crate::{
    Border, BorderStyle, DisplayItem, DisplayList, FontMetrics, ImageSource, LayoutOptions,
    LinkRegion, LinkTarget, Rect, Size, TextMetrics, TooltipRegion,
    css::{self, Align, Display, Length, Style, VerticalAlign, WhiteSpace},
    dom::Document,
};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

/// Lays out a parsed popup/annotation. The metrics provider supplies shaped advances and baselines.
pub fn layout(
    document: &Document,
    options: &LayoutOptions,
    metrics: &impl FontMetrics,
) -> DisplayList {
    let options = if options.max_width.is_finite()
        && options.max_width >= 1.
        && options.root_font_size.is_finite()
        && (1. ..=512.).contains(&options.root_font_size)
    {
        std::borrow::Cow::Borrowed(options)
    } else {
        let mut options = options.clone();
        options.max_width = finite(options.max_width, 1.).max(1.);
        options.root_font_size = finite(options.root_font_size, 13.).clamp(1., 512.);
        std::borrow::Cow::Owned(options)
    };
    let styles = css::compute(document, &options);
    let mut engine = Engine {
        doc: document,
        styles: &styles,
        options: &options,
        metrics,
        out: DisplayList::default(),
    };
    let size = engine.block(
        0,
        [0., 0.],
        options.max_width,
        None,
        false,
        &Context::default(),
    );
    let width = engine
        .out
        .items
        .iter()
        .map(|i| i.rect().x + i.rect().width)
        .fold(size.width, f32::max);
    let height = engine
        .out
        .items
        .iter()
        .map(|i| i.rect().y + i.rect().height)
        .fold(size.height, f32::max);
    engine.out.size = Size { width, height };
    engine.out.scroll_height = height;
    engine.out
}
fn finite(n: f32, fallback: f32) -> f32 {
    if n.is_finite() { n } else { fallback }
}
fn clean(m: TextMetrics) -> TextMetrics {
    TextMetrics {
        advance: finite(m.advance, 0.).max(0.),
        ascent: finite(m.ascent, 0.).max(0.),
        descent: finite(m.descent, 0.).max(0.),
    }
}
fn html_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}')
}
#[derive(Debug, Default, Clone)]
struct Context {
    link: Option<LinkTarget>,
    title: Option<String>,
    decorations: Vec<usize>,
}
impl Context {
    fn child(&self, doc: &Document, id: usize) -> Self {
        let n = &doc.nodes[id];
        let mut c = self.clone();
        if n.tag == "a"
            && let Some(href) = n.attrs.get("href")
        {
            c.link = LinkTarget::parse(href).ok();
        }
        if let Some(title) = n.attrs.get("title") {
            c.title = Some(title.clone());
        }
        c
    }
}
#[derive(Debug)]
struct Fragment {
    items: Range<usize>,
    links: Range<usize>,
    tips: Range<usize>,
    ascent: f32,
    height: f32,
    vertical: VerticalAlign,
    font: f32,
}
#[derive(Debug)]
struct InlineBox {
    node: usize,
    item: usize,
    left: f32,
    right: f32,
}
#[derive(Debug, Clone)]
struct Space {
    style: Style,
    context: Context,
}
#[derive(Debug)]
struct Flow {
    x: f32,
    y: f32,
    width: f32,
    cursor: f32,
    line: Vec<Fragment>,
    boxes: Vec<InlineBox>,
    minimum: f32,
    align: Align,
    pending: Option<Space>,
    max_width: f32,
    containing_height: Option<f32>,
    last_baseline: f32,
}
impl Flow {
    fn new(x: f32, y: f32, width: f32, style: &Style, root: f32) -> Self {
        Self {
            x,
            y,
            width,
            cursor: 0.,
            line: Vec::new(),
            boxes: Vec::new(),
            minimum: style.line_pixels(root),
            align: style.align,
            pending: None,
            max_width: 0.,
            containing_height: None,
            last_baseline: 0.,
        }
    }
}
#[derive(Debug, Default)]
struct BoxSize {
    width: f32,
    height: f32,
    baseline: f32,
}
struct Engine<'a, M> {
    doc: &'a Document,
    styles: &'a [Style],
    options: &'a LayoutOptions,
    metrics: &'a M,
    out: DisplayList,
}
impl<M: FontMetrics> Engine<'_, M> {
    fn length(&self, n: Length, font: f32, reference: f32) -> Option<f32> {
        n.resolve(font, self.options.root_font_size, reference)
    }
    fn edges(&self, edges: [Length; 4], style: &Style, width: f32) -> [f32; 4] {
        edges.map(|l| {
            self.length(l, style.text.font_size, width)
                .unwrap_or(0.)
                .max(0.)
        })
    }
    fn borders(&self, style: &Style, width: f32) -> [Border; 4] {
        std::array::from_fn(|i| Border {
            width: if style.border_style[i] == BorderStyle::None {
                0.
            } else {
                self.length(style.border_width[i], style.text.font_size, width)
                    .unwrap_or(0.)
                    .max(0.)
            },
            style: style.border_style[i],
            color: style.border_color[i],
        })
    }
    fn radii(&self, style: &Style, rect: Rect) -> [f32; 4] {
        let mut radii = style.radii.map(|r| {
            self.length(r, style.text.font_size, rect.width.min(rect.height))
                .unwrap_or(0.)
                .max(0.)
        });
        // Scale all corner radii together when their sums exceed an edge.
        let factor = [
            (rect.width, radii[0] + radii[1]),
            (rect.width, radii[2] + radii[3]),
            (rect.height, radii[0] + radii[3]),
            (rect.height, radii[1] + radii[2]),
        ]
        .into_iter()
        .filter(|(_, sum)| *sum > 0.)
        .map(|(edge, sum)| edge / sum)
        .fold(1., f32::min)
        .clamp(0., 1.);
        for r in &mut radii {
            *r *= factor;
        }
        radii
    }
    fn paint_box(&mut self, style: &Style, rect: Rect) -> Option<usize> {
        let borders = self.borders(style, rect.width);
        if style.background.a == 0 && borders.iter().all(|b| b.width == 0.) {
            return None;
        }
        let radii = self.radii(style, rect);
        let at = self.out.items.len();
        self.out.items.push(DisplayItem::Box {
            rect,
            background: style.background,
            borders,
            radii,
        });
        Some(at)
    }
    fn block(
        &mut self,
        id: usize,
        origin: [f32; 2],
        available: f32,
        parent_height: Option<f32>,
        shrink: bool,
        context: &Context,
    ) -> BoxSize {
        let [x, y] = origin;
        let style = &self.styles[id];
        if style.display == Display::None {
            return BoxSize::default();
        }
        let context = context.child(self.doc, id);
        if self.doc.nodes[id].tag == "img" {
            let mut flow = Flow::new(x, y, available, style, self.options.root_font_size);
            flow.containing_height = parent_height;
            self.image(id, &mut flow, &context);
            self.finish_line(&mut flow, false);
            return BoxSize {
                width: flow.max_width,
                height: flow.y - y,
                baseline: flow.last_baseline - y,
            };
        }
        let margin = self.edges(style.margin, style, available);
        let padding = self.edges(style.padding, style, available);
        let borders = self.borders(style, available);
        let horizontal = padding[1] + padding[3] + borders[1].width + borders[3].width;
        let vertical = padding[0] + padding[2] + borders[0].width + borders[2].width;
        let natural = (available - margin[1] - margin[3] - horizontal).max(0.);
        let mut width = self
            .length(style.width, style.text.font_size, available)
            .unwrap_or_else(|| {
                if shrink {
                    self.intrinsic(id, available).min(natural)
                } else {
                    natural
                }
            });
        if let Some(max) = self.length(style.max_width, style.text.font_size, available) {
            width = width.min(max.max(0.));
        }
        if let Some(min) = self.length(style.min_width, style.text.font_size, available) {
            width = width.max(min.max(0.));
        }
        width = width.max(0.);
        let explicit_height = match style.height {
            Length::Percent(_) => {
                parent_height.and_then(|h| self.length(style.height, style.text.font_size, h))
            }
            _ => self.length(
                style.height,
                style.text.font_size,
                parent_height.unwrap_or(0.),
            ),
        };
        let offset = self.relative(style, available, parent_height.unwrap_or(0.));
        let bx = x + margin[3] + offset[0];
        let by = y + margin[0] + offset[1];

        let tip_placeholder = context.title.as_ref().map(|title| {
            let at = self.out.tooltips.len();
            self.out.tooltips.push(TooltipRegion {
                rect: Rect::default(),
                title: title.clone(),
            });
            at
        });
        let link_placeholder = context.link.as_ref().map(|target| {
            let at = self.out.links.len();
            self.out.links.push(LinkRegion {
                rect: Rect::default(),
                target: target.clone(),
            });
            at
        });
        let placeholder = self.paint_box(
            style,
            Rect {
                x: bx,
                y: by,
                width: width + horizontal,
                height: explicit_height.unwrap_or(0.) + vertical,
            },
        );
        let cx = bx + padding[3] + borders[3].width;
        let cy = by + padding[0] + borders[0].width;
        let mut flow = Flow::new(cx, cy, width, style, self.options.root_font_size);
        flow.containing_height = explicit_height;
        let mut previous_bottom: f32 = 0.;
        let mut count = 0;
        for child in &self.doc.nodes[id].children {
            let child_style = &self.styles[*child];
            if child_style.display == Display::None {
                continue;
            }
            if [Display::Block, Display::ListItem].contains(&child_style.display) {
                self.finish_line(&mut flow, false);
                flow.pending = None;
                let top = self.edges(child_style.margin, child_style, width)[0];
                let collapse = if previous_bottom > 0. {
                    previous_bottom.min(top)
                } else {
                    0.
                };
                if child_style.display == Display::ListItem {
                    count += 1;
                    self.marker(*child, count, cx, flow.y - collapse, width, &context);
                }
                let child_size = self.block(
                    *child,
                    [cx, flow.y - collapse],
                    width,
                    explicit_height,
                    false,
                    &context,
                );
                flow.y += child_size.height - collapse;
                flow.max_width = flow.max_width.max(child_size.width);
                previous_bottom = self.edges(child_style.margin, child_style, width)[2];
            } else {
                previous_bottom = 0.;
                self.inline(*child, &mut flow, &context);
            }
        }
        self.finish_line(&mut flow, false);
        let mut height = explicit_height.unwrap_or((flow.y - cy).max(0.));
        let reference = parent_height.unwrap_or(0.);
        if let Some(max) = self.length(style.max_height, style.text.font_size, reference) {
            height = height.min(max.max(0.));
        }
        if let Some(min) = self.length(style.min_height, style.text.font_size, reference) {
            height = height.max(min.max(0.));
        }
        let rect = Rect {
            x: bx,
            y: by,
            width: width + horizontal,
            height: height + vertical,
        };
        if let Some(at) = placeholder {
            // Rebuild once to normalize radii against final auto height.
            let mut item = self.out.items[at].clone();
            *item.rect_mut() = rect;
            if let DisplayItem::Box { radii, .. } = &mut item {
                *radii = self.radii(style, rect);
            }
            self.out.items[at] = item;
        }
        if let Some(at) = tip_placeholder {
            self.out.tooltips[at].rect = rect;
        }
        if let Some(at) = link_placeholder {
            self.out.links[at].rect = rect;
        }
        BoxSize {
            width: width + horizontal + margin[1] + margin[3],
            height: height + vertical + margin[0] + margin[2],
            baseline: if flow.last_baseline > 0. {
                flow.last_baseline - y
            } else {
                rect.height + margin[0]
            },
        }
    }
    fn relative(&self, style: &Style, width: f32, height: f32) -> [f32; 2] {
        if !style.relative {
            return [0., 0.];
        }
        let x = self
            .length(style.offsets[3], style.text.font_size, width)
            .or_else(|| {
                self.length(style.offsets[1], style.text.font_size, width)
                    .map(|n| -n)
            })
            .unwrap_or(0.);
        let y = self
            .length(style.offsets[0], style.text.font_size, height)
            .or_else(|| {
                self.length(style.offsets[2], style.text.font_size, height)
                    .map(|n| -n)
            })
            .unwrap_or(0.);
        [x, y]
    }
    fn marker(&mut self, id: usize, count: usize, x: f32, y: f32, width: f32, context: &Context) {
        let style = &self.styles[id];
        if style.list_style == "none" {
            return;
        }
        let marker = if style.list_style == "decimal" {
            format!("{count}.")
        } else {
            match style.list_style {
                "circle" => "◦",
                "square" => "▪",
                _ => "•",
            }
            .into()
        };
        let metrics = clean(self.metrics.measure(&marker, &style.text));
        let margin = self.edges(style.margin, style, width);
        let padding = self.edges(style.padding, style, width);
        let borders = self.borders(style, width);
        let height = style
            .line_pixels(self.options.root_font_size)
            .max(metrics.ascent + metrics.descent);
        let rect = Rect {
            x: x + margin[3] + padding[3] + borders[3].width
                - metrics.advance
                - style.text.font_size * 0.5,
            y: y + margin[0] + padding[0] + borders[0].width,
            width: metrics.advance,
            height: style.line_pixels(self.options.root_font_size),
        };
        self.out.items.push(DisplayItem::Text {
            rect,
            baseline: metrics.ascent + (height - metrics.ascent - metrics.descent) / 2.,
            text: marker,
            style: style.text.clone(),
        });
        if let Some(title) = &context.title {
            self.out.tooltips.push(TooltipRegion {
                rect,
                title: title.clone(),
            });
        }
    }
    fn intrinsic(&self, id: usize, available: f32) -> f32 {
        let node = &self.doc.nodes[id];
        let style = &self.styles[id];
        if let Some(w) = self.length(style.width, style.text.font_size, available) {
            return w.max(0.);
        }
        if node.tag.is_empty() {
            return clean(self.metrics.measure(&node.text, &style.text)).advance;
        }
        if node.tag == "img" {
            return node
                .attrs
                .get("src")
                .and_then(|s| ImageSource::parse(s))
                .and_then(|s| self.metrics.image_size(&s))
                .map_or(16., |s| s.width);
        }
        let mut width = 0f32;
        let mut line = 0f32;
        for child in &node.children {
            let s = &self.styles[*child];
            if s.display == Display::None {
                continue;
            }
            let w = self.intrinsic(*child, available);
            if [Display::Block, Display::ListItem].contains(&s.display) {
                width = width.max(line).max(w);
                line = 0.;
            } else {
                line += w;
            }
        }
        width.max(line)
    }
    fn inline(&mut self, id: usize, flow: &mut Flow, context: &Context) {
        let node = &self.doc.nodes[id];
        let style = &self.styles[id];
        if style.display == Display::None {
            return;
        }
        if node.tag.is_empty() {
            self.text(&node.text, style, flow, context);
            return;
        }
        let mut context = context.child(self.doc, id);
        if node.tag == "br" {
            self.finish_line(flow, true);
            flow.pending = None;
            return;
        }
        if node.tag == "img" {
            self.image(id, flow, &context);
            return;
        }
        if style.display == Display::InlineBlock {
            let margin = self.edges(style.margin, style, flow.width);
            let padding = self.edges(style.padding, style, flow.width);
            let borders = self.borders(style, flow.width);
            let edges = margin[1]
                + margin[3]
                + padding[1]
                + padding[3]
                + borders[1].width
                + borders[3].width;
            let mut hint = self
                .intrinsic(id, flow.width)
                .min((flow.width - edges).max(0.));
            if let Some(max) = self.length(style.max_width, style.text.font_size, flow.width) {
                hint = hint.min(max);
            }
            if let Some(min) = self.length(style.min_width, style.text.font_size, flow.width) {
                hint = hint.max(min);
            }
            let space = flow.pending.as_ref().map_or(0., |s| {
                clean(self.metrics.measure(" ", &s.style.text)).advance
            });
            if flow.cursor > 0.
                && flow.cursor + space + hint + edges > flow.width
                && !matches!(style.white_space, WhiteSpace::NoWrap | WhiteSpace::Pre)
            {
                self.finish_line(flow, false);
            }
            self.flush_space(flow, false);
            let items = self.out.items.len();
            let links = self.out.links.len();
            let tips = self.out.tooltips.len();
            let size = self.block(
                id,
                [flow.x + flow.cursor, flow.y],
                flow.width,
                flow.containing_height,
                true,
                &context,
            );
            flow.line.push(Fragment {
                items: items..self.out.items.len(),
                links: links..self.out.links.len(),
                tips: tips..self.out.tooltips.len(),
                ascent: size.baseline,
                height: size.height,
                vertical: style.vertical,
                font: style.text.font_size,
            });
            flow.cursor += size.width;
            return;
        }
        if [Display::Block, Display::ListItem].contains(&style.display) {
            self.finish_line(flow, false);
            let size = self.block(id, [flow.x, flow.y], flow.width, None, false, &context);
            flow.y += size.height;
            flow.max_width = flow.max_width.max(size.width);
            return;
        }
        let padding = self.edges(style.padding, style, flow.width);
        let borders = self.borders(style, flow.width);
        if style.background.a > 0 || borders.iter().any(|b| b.width > 0.) {
            context.decorations.push(id);
        }
        let margin = self.edges(style.margin, style, flow.width);
        let left = padding[3] + borders[3].width + margin[3];
        let right = padding[1] + borders[1].width + margin[1];
        let start = (
            self.out.items.len(),
            self.out.links.len(),
            self.out.tooltips.len(),
        );
        if left > 0. {
            self.flush_space(flow, false);
            flow.cursor += left;
        }
        for child in &node.children {
            self.inline(*child, flow, &context);
        }
        flow.cursor += right;
        if style.relative {
            let offset = self.relative(
                style,
                flow.width,
                style.line_pixels(self.options.root_font_size),
            );
            self.translate(
                start.0..self.out.items.len(),
                start.1..self.out.links.len(),
                start.2..self.out.tooltips.len(),
                offset[0],
                offset[1],
            );
        }
    }
    fn text(&mut self, text: &str, style: &Style, flow: &mut Flow, context: &Context) {
        let preserve = matches!(style.white_space, WhiteSpace::Pre | WhiteSpace::PreWrap);
        let wrap = matches!(style.white_space, WhiteSpace::Normal | WhiteSpace::PreWrap);
        for word in text.split_word_bounds() {
            if preserve && word.chars().any(|c| c == '\n' || c == '\r') {
                let normalized = word.replace("\r\n", "\n").replace('\r', "\n");
                for (i, part) in normalized.split('\n').enumerate() {
                    if i > 0 {
                        self.finish_line(flow, true);
                    }
                    if !part.is_empty() {
                        self.put_text(&part.replace('\t', "    "), style, flow, context);
                    }
                }
                continue;
            }
            if !preserve && word.chars().all(html_space) {
                if flow.cursor > 0. {
                    flow.pending = Some(Space {
                        style: style.clone(),
                        context: context.clone(),
                    });
                }
                continue;
            }
            let word = if preserve {
                word.replace('\t', "    ")
            } else {
                word.into()
            };
            let advance = clean(self.metrics.measure(&word, &style.text)).advance;
            let space = flow.pending.as_ref().map_or(0., |s| {
                clean(self.metrics.measure(" ", &s.style.text)).advance
            });
            if wrap && flow.cursor > 0. && flow.cursor + space + advance > flow.width {
                self.finish_line(flow, false);
                flow.pending = None;
            }
            self.flush_space(flow, false);
            self.put_text(&word, style, flow, context);
        }
    }
    fn flush_space(&mut self, flow: &mut Flow, _wrap: bool) {
        if let Some(space) = flow.pending.take()
            && flow.cursor > 0.
        {
            self.put_text(" ", &space.style, flow, &space.context);
        }
    }
    fn put_text(&mut self, text: &str, style: &Style, flow: &mut Flow, context: &Context) {
        let metrics = clean(self.metrics.measure(text, &style.text));
        let height = style
            .line_pixels(self.options.root_font_size)
            .max(metrics.ascent + metrics.descent);
        let leading = (height - metrics.ascent - metrics.descent) / 2.;
        let rect = Rect {
            x: flow.x + flow.cursor,
            y: flow.y,
            width: metrics.advance,
            height,
        };
        self.inline_backgrounds(flow, context, rect);
        let items = self.out.items.len();
        let links = self.out.links.len();
        let tips = self.out.tooltips.len();
        self.out.items.push(DisplayItem::Text {
            rect,
            baseline: metrics.ascent + leading,
            text: text.into(),
            style: style.text.clone(),
        });
        if let Some(target) = &context.link {
            self.out.links.push(LinkRegion {
                rect,
                target: target.clone(),
            });
        }
        if let Some(title) = &context.title {
            self.out.tooltips.push(TooltipRegion {
                rect,
                title: title.clone(),
            });
        }
        flow.line.push(Fragment {
            items: items..self.out.items.len(),
            links: links..self.out.links.len(),
            tips: tips..self.out.tooltips.len(),
            ascent: metrics.ascent + leading,
            height,
            vertical: style.vertical,
            font: style.text.font_size,
        });
        flow.cursor += metrics.advance;
    }
    fn inline_backgrounds(&mut self, flow: &mut Flow, context: &Context, rect: Rect) {
        for id in &context.decorations {
            let style = &self.styles[*id];
            let padding = self.edges(style.padding, style, flow.width);
            let borders = self.borders(style, flow.width);
            let left = rect.x - padding[3] - borders[3].width;
            let right = rect.x + rect.width + padding[1] + borders[1].width;
            if let Some(b) = flow.boxes.iter_mut().find(|b| b.node == *id) {
                b.left = b.left.min(left);
                b.right = b.right.max(right);
            } else if let Some(item) = self.paint_box(style, rect) {
                flow.boxes.push(InlineBox {
                    node: *id,
                    item,
                    left,
                    right,
                });
            }
        }
    }
    fn image(&mut self, id: usize, flow: &mut Flow, context: &Context) {
        let node = &self.doc.nodes[id];
        let style = &self.styles[id];
        let Some(source) = node.attrs.get("src").and_then(|s| ImageSource::parse(s)) else {
            if let Some(alt) = node.attrs.get("alt") {
                self.text(alt, style, flow, context);
            }
            return;
        };
        let intrinsic = self.metrics.image_size(&source).unwrap_or(Size {
            width: 16.,
            height: 16.,
        });
        let iw = finite(intrinsic.width, 16.).max(1.);
        let ih = finite(intrinsic.height, 16.).max(1.);
        let width = self
            .length(style.width, style.text.font_size, flow.width)
            .or_else(|| node.attrs.get("width").and_then(|s| s.parse::<f32>().ok()));
        let height = (match style.height {
            Length::Percent(_) => flow
                .containing_height
                .and_then(|h| self.length(style.height, style.text.font_size, h)),
            _ => self.length(
                style.height,
                style.text.font_size,
                flow.containing_height.unwrap_or(0.),
            ),
        })
        .or_else(|| node.attrs.get("height").and_then(|s| s.parse::<f32>().ok()));
        let mut w = width.unwrap_or_else(|| height.map_or(iw, |h| h * iw / ih));
        if let Some(max) = self.length(style.max_width, style.text.font_size, flow.width) {
            w = w.min(max);
        }
        if let Some(min) = self.length(style.min_width, style.text.font_size, flow.width) {
            w = w.max(min);
        }
        let mut h = height.unwrap_or(w * ih / iw);
        let limit = |length: Length| match length {
            Length::Percent(_) => flow
                .containing_height
                .and_then(|n| self.length(length, style.text.font_size, n)),
            _ => self.length(
                length,
                style.text.font_size,
                flow.containing_height.unwrap_or(0.),
            ),
        };
        if let Some(max) = limit(style.max_height) {
            h = h.min(max);
        }
        if let Some(min) = limit(style.min_height) {
            h = h.max(min);
        }
        if width.is_none() {
            w = h * iw / ih;
        }
        w = finite(w, 0.).max(0.);
        h = finite(h, 0.).max(0.);
        let margin = self.edges(style.margin, style, flow.width);
        let padding = self.edges(style.padding, style, flow.width);
        let border = self.borders(style, flow.width);
        let width =
            w + margin[1] + margin[3] + padding[1] + padding[3] + border[1].width + border[3].width;
        let height =
            h + margin[0] + margin[2] + padding[0] + padding[2] + border[0].width + border[2].width;
        let space = flow.pending.as_ref().map_or(0., |s| {
            clean(self.metrics.measure(" ", &s.style.text)).advance
        });
        if flow.cursor > 0.
            && flow.cursor + space + width > flow.width
            && !matches!(style.white_space, WhiteSpace::NoWrap | WhiteSpace::Pre)
        {
            self.finish_line(flow, false);
            flow.pending = None;
        }
        self.flush_space(flow, false);
        self.inline_backgrounds(
            flow,
            context,
            Rect {
                x: flow.x + flow.cursor,
                y: flow.y,
                width,
                height,
            },
        );
        let items = self.out.items.len();
        let links = self.out.links.len();
        let tips = self.out.tooltips.len();
        let outer = Rect {
            x: flow.x + flow.cursor + margin[3],
            y: flow.y + margin[0],
            width: width - margin[1] - margin[3],
            height: height - margin[0] - margin[2],
        };
        self.paint_box(style, outer);
        let rect = Rect {
            x: outer.x + padding[3] + border[3].width,
            y: outer.y + padding[0] + border[0].width,
            width: w,
            height: h,
        };
        self.out.items.push(DisplayItem::Image { rect, source });
        if let Some(target) = &context.link {
            self.out.links.push(LinkRegion {
                rect: outer,
                target: target.clone(),
            });
        }
        if let Some(title) = &context.title {
            self.out.tooltips.push(TooltipRegion {
                rect: outer,
                title: title.clone(),
            });
        }
        flow.line.push(Fragment {
            items: items..self.out.items.len(),
            links: links..self.out.links.len(),
            tips: tips..self.out.tooltips.len(),
            ascent: height,
            height,
            vertical: style.vertical,
            font: style.text.font_size,
        });
        flow.cursor += width;
    }
    fn finish_line(&mut self, flow: &mut Flow, force: bool) {
        flow.pending = None;
        if flow.line.is_empty() {
            if force {
                flow.y += flow.minimum;
            }
            flow.cursor = 0.;
            return;
        }
        let ascent = flow
            .line
            .iter()
            .map(|f| {
                f.ascent
                    + match f.vertical {
                        VerticalAlign::Super => f.font / 3.,
                        VerticalAlign::Offset(l) => self.length(l, f.font, f.height).unwrap_or(0.),
                        _ => 0.,
                    }
            })
            .fold(0., f32::max);
        let descent = flow
            .line
            .iter()
            .map(|f| {
                (f.height - f.ascent)
                    + if f.vertical == VerticalAlign::Sub {
                        f.font / 5.
                    } else {
                        0.
                    }
            })
            .fold(0., f32::max);
        let height = flow.minimum.max(ascent + descent);
        let align = match flow.align {
            Align::Left => 0.,
            Align::Center => (flow.width - flow.cursor).max(0.) / 2.,
            Align::Right => (flow.width - flow.cursor).max(0.),
        };
        for f in flow.line.drain(..) {
            let dy = match f.vertical {
                VerticalAlign::Top => 0.,
                VerticalAlign::Middle => (height - f.height) / 2.,
                VerticalAlign::Bottom => height - f.height,
                VerticalAlign::Super => ascent - f.ascent - f.font / 3.,
                VerticalAlign::Sub => ascent - f.ascent + f.font / 5.,
                VerticalAlign::Offset(l) => {
                    ascent - f.ascent - self.length(l, f.font, f.height).unwrap_or(0.)
                }
                VerticalAlign::Baseline => ascent - f.ascent,
            };
            self.translate(f.items, f.links, f.tips, align, dy);
        }
        for b in flow.boxes.drain(..) {
            let style = &self.styles[b.node];
            let padding = self.edges(style.padding, style, flow.width);
            let offset = self.relative(style, flow.width, height);
            let rect = Rect {
                x: b.left + align + offset[0],
                y: flow.y - padding[0] + offset[1],
                width: b.right - b.left,
                height: height + padding[0] + padding[2],
            };
            let radii = self.radii(style, rect);
            *self.out.items[b.item].rect_mut() = rect;
            if let DisplayItem::Box { radii: corners, .. } = &mut self.out.items[b.item] {
                *corners = radii;
            }
        }
        flow.max_width = flow.max_width.max(flow.cursor);
        flow.last_baseline = flow.y + ascent;
        flow.y += height;
        flow.cursor = 0.;
    }
    fn translate(
        &mut self,
        items: Range<usize>,
        links: Range<usize>,
        tips: Range<usize>,
        dx: f32,
        dy: f32,
    ) {
        for item in &mut self.out.items[items] {
            let r = item.rect_mut();
            r.x += dx;
            r.y += dy;
        }
        for region in &mut self.out.links[links] {
            region.rect.x += dx;
            region.rect.y += dy;
        }
        for region in &mut self.out.tooltips[tips] {
            region.rect.x += dx;
            region.rect.y += dy;
        }
    }
}
