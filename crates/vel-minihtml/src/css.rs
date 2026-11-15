//! CSS subset, selectors, cascade, units and computed styles.
use crate::{Color, LayoutOptions, dom::Document};
use std::{collections::BTreeMap, sync::Arc};

/// CSS length, retained until its containing dimensions are known.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub enum Length {
    /// Automatic dimension.
    #[default]
    Auto,
    /// Device-independent pixels.
    Px(f32),
    /// Multiple of the current font size.
    Em(f32),
    /// Multiple of the root font size.
    Rem(f32),
    /// Percentage of the containing dimension.
    Percent(f32),
}
impl Length {
    /// Evaluates the unit; automatic lengths return None. CSS points convert at 96/72 DPI.
    pub fn resolve(self, font: f32, root: f32, reference: f32) -> Option<f32> {
        match self {
            Self::Auto => None,
            Self::Px(n) => Some(n),
            Self::Em(n) => Some(n * font),
            Self::Rem(n) => Some(n * root),
            Self::Percent(n) => Some(n * reference / 100.),
        }
    }
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text == "auto" {
            return Some(Self::Auto);
        }
        for (suffix, unit) in [
            ("rem", 2),
            ("em", 1),
            ("px", 0),
            ("pt", 3),
            ("%", 4),
            ("", 0),
        ] {
            if let Some(n) = text.strip_suffix(suffix)
                && let Ok(n) = n.trim().parse::<f32>()
                && n.is_finite()
            {
                return Some(match unit {
                    1 => Self::Em(n),
                    2 => Self::Rem(n),
                    3 => Self::Px(n * 4. / 3.),
                    4 => Self::Percent(n),
                    _ => Self::Px(n),
                });
            }
        }
        None
    }
}
/// Formatting context selected by CSS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Display {
    /// Removed from layout.
    None,
    /// Participates in a line.
    Inline,
    /// Starts a new block.
    Block,
    /// Atomic inline box with its own layout context.
    InlineBlock,
    /// Block with a list marker.
    ListItem,
}
/// Whitespace collapsing and wrapping mode.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum WhiteSpace {
    /// Collapse and wrap.
    #[default]
    Normal,
    /// Collapse without wrapping.
    NoWrap,
    /// Preserve whitespace and explicit line breaks, without wrapping.
    Pre,
    /// Preserve whitespace and explicit line breaks, with wrapping.
    PreWrap,
}
/// Horizontal line alignment.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    /// Align to the start.
    #[default]
    Left,
    /// Center each line.
    Center,
    /// Align to the end.
    Right,
}
/// Inline alignment relative to the current line.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub enum VerticalAlign {
    /// Shared text baseline.
    #[default]
    Baseline,
    /// Top edge.
    Top,
    /// Middle of the line.
    Middle,
    /// Bottom edge.
    Bottom,
    /// Raised by one third of the font size.
    Super,
    /// Lowered by one fifth of the font size.
    Sub,
    /// Baseline raised by this length.
    Offset(Length),
}
/// Supported CSS border styles.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum BorderStyle {
    /// No border width is consumed.
    #[default]
    None,
    /// Solid stroke.
    Solid,
}
/// Font and text paint parameters passed to the caller's shaping engine.
#[derive(Debug, Clone, PartialEq)]
pub struct TextStyle {
    /// Comma-separated CSS font fallback list.
    pub font_family: Arc<str>,
    /// Size in logical pixels.
    pub font_size: f32,
    /// Numeric weight, normally 400 or 700.
    pub font_weight: u16,
    /// Italic face.
    pub italic: bool,
    /// Underline decoration.
    pub underline: bool,
    /// Foreground color.
    pub color: Color,
}
#[derive(Debug, Clone)]
pub(crate) struct Declaration {
    pub name: String,
    pub value: String,
    pub important: bool,
}
#[derive(Debug, Clone)]
pub(crate) struct Rule {
    selector: Selector,
    pub declarations: Vec<Declaration>,
}
#[derive(Debug, Clone)]
struct Simple {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
}
#[derive(Debug, Clone, Copy)]
enum Combinator {
    Descendant,
    Child,
}
#[derive(Debug, Clone)]
struct Selector {
    parts: Vec<Simple>,
    between: Vec<Combinator>,
    specificity: u32,
}

/// Splits CSS only outside parentheses, strings and brackets.
pub(crate) fn split_top(text: &str, delimiter: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    let mut quote = None;
    let mut escape = false;
    for (i, c) in text.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if c == '\\' {
            escape = true;
            continue;
        }
        if quote == Some(c) {
            quote = None;
            continue;
        }
        if quote.is_some() {
            continue;
        }
        if c == '\'' || c == '"' {
            quote = Some(c);
            continue;
        }
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if c == delimiter && depth == 0 {
            out.push(text[start..i].trim());
            start = i + c.len_utf8();
        }
    }
    out.push(text[start..].trim());
    out
}
fn without_comments(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    // Quote-aware scanning keeps data URLs and literal comment markers intact.
    let mut quote = None;
    let mut escape = false;
    while !rest.is_empty() {
        if quote.is_none() && rest.starts_with("/*") {
            rest = rest.find("*/").map_or("", |at| &rest[at + 2..]);
            out.push(' ');
            continue;
        }
        let c = rest.chars().next().unwrap();
        rest = &rest[c.len_utf8()..];
        out.push(c);
        if escape {
            escape = false;
            continue;
        }
        if c == '\\' {
            escape = true;
            continue;
        }
        if quote == Some(c) {
            quote = None;
        } else if quote.is_none() && (c == '\'' || c == '"') {
            quote = Some(c);
        }
    }
    out
}
pub(crate) fn parse_declarations(text: &str) -> Vec<Declaration> {
    let text = without_comments(text);
    split_top(&text, ';')
        .into_iter()
        .filter_map(|s| {
            let (name, value) = s.split_once(':')?;
            let name = name.trim();
            let name = if name.starts_with("--") {
                name.into()
            } else {
                name.to_ascii_lowercase()
            };
            let value = value.trim();
            let important = value.ends_with("!important");
            let value = if important {
                value[..value.len() - 10].trim()
            } else {
                value
            };
            (!value.is_empty()).then(|| Declaration {
                name,
                value: value.into(),
                important,
            })
        })
        .collect()
}
pub(crate) fn parse_stylesheet(text: &str) -> Vec<Rule> {
    let text = without_comments(text);
    let mut rest = text.as_str();
    let mut out = Vec::new();
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open + 1..].find('}').map(|n| n + open + 1) else {
            break;
        };
        let declarations = parse_declarations(&rest[open + 1..close]);
        for selector in split_top(&rest[..open], ',') {
            if let Some(selector) = Selector::parse(selector) {
                out.push(Rule {
                    selector,
                    declarations: declarations.clone(),
                });
            }
        }
        rest = &rest[close + 1..];
    }
    out
}
impl Selector {
    fn parse(mut text: &str) -> Option<Self> {
        text = text.trim();
        if text.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        let mut between = Vec::new();
        let mut specificity = 0u32;
        while !text.is_empty() {
            let end = text
                .find(|c: char| c.is_whitespace() || c == '>')
                .unwrap_or(text.len());
            let token = &text[..end];
            if token.is_empty() {
                return None;
            }
            let mut simple = Simple {
                tag: None,
                id: None,
                classes: Vec::new(),
            };
            let mut token = token;
            while !token.is_empty() {
                let (kind, body) = if let Some(t) = token.strip_prefix('.') {
                    ('.', t)
                } else if let Some(t) = token.strip_prefix('#') {
                    ('#', t)
                } else {
                    ('t', token)
                };
                let end = body.find(['.', '#']).unwrap_or(body.len());
                let value = &body[..end];
                if value.is_empty()
                    || !value
                        .chars()
                        .all(|c| c.is_alphanumeric() || ['_', '-', '*'].contains(&c))
                {
                    return None;
                }
                match kind {
                    '.' => {
                        simple.classes.push(value.into());
                        specificity = specificity.saturating_add(1u32 << 10);
                    }
                    '#' => {
                        simple.id = Some(value.into());
                        specificity = specificity.saturating_add(1u32 << 20);
                    }
                    _ => {
                        if value != "*" {
                            simple.tag = Some(value.to_ascii_lowercase());
                            specificity = specificity.saturating_add(1);
                        }
                    }
                }
                token = &body[end..];
            }
            parts.push(simple);
            text = text[end..].trim_start();
            if let Some(t) = text.strip_prefix('>') {
                between.push(Combinator::Child);
                text = t.trim_start();
                if text.is_empty() {
                    return None;
                }
            } else if !text.is_empty() {
                between.push(Combinator::Descendant);
            }
        }
        if parts.len() > 64 {
            return None;
        }
        Some(Self {
            parts,
            between,
            specificity,
        })
    }
    fn matches(&self, doc: &Document, id: usize, dark: bool) -> bool {
        fn walk(s: &Selector, doc: &Document, id: usize, part: usize, dark: bool) -> bool {
            let p = &s.parts[part];
            let node = &doc.nodes[id];
            if p.tag.as_ref().is_some_and(|tag| tag != &node.tag)
                || p.id
                    .as_ref()
                    .is_some_and(|v| node.attrs.get("id") != Some(v))
            {
                return false;
            }
            if !p.classes.iter().all(|class| {
                (id == 0 && class == if dark { "dark" } else { "light" })
                    || node
                        .attrs
                        .get("class")
                        .is_some_and(|classes| classes.split_whitespace().any(|c| c == class))
            }) {
                return false;
            }
            if part == 0 {
                return true;
            }
            let Some(mut parent) = node.parent else {
                return false;
            };
            match s.between[part - 1] {
                Combinator::Child => walk(s, doc, parent, part - 1, dark),
                Combinator::Descendant => loop {
                    if walk(s, doc, parent, part - 1, dark) {
                        return true;
                    }
                    let Some(next) = doc.nodes[parent].parent else {
                        return false;
                    };
                    parent = next;
                },
            }
        }
        walk(self, doc, id, self.parts.len() - 1, dark)
    }
}
#[derive(Debug, Clone, Copy)]
pub(crate) enum LineHeight {
    Normal,
    Multiple(f32),
    Length(Length),
}
#[derive(Debug, Clone)]
pub(crate) struct Style {
    pub text: TextStyle,
    pub display: Display,
    pub background: Color,
    pub margin: [Length; 4],
    pub padding: [Length; 4],
    pub border_width: [Length; 4],
    pub border_style: [BorderStyle; 4],
    pub border_color: [Color; 4],
    pub radii: [Length; 4],
    pub width: Length,
    pub height: Length,
    pub min_width: Length,
    pub max_width: Length,
    pub min_height: Length,
    pub max_height: Length,
    pub white_space: WhiteSpace,
    pub align: Align,
    pub vertical: VerticalAlign,
    pub line_height: LineHeight,
    pub relative: bool,
    pub offsets: [Length; 4],
    pub list_style: &'static str,
    pub vars: Arc<BTreeMap<String, String>>,
}
impl Style {
    fn initial(options: &LayoutOptions, vars: Arc<BTreeMap<String, String>>) -> Self {
        let fg = options
            .palette
            .color("var(--foreground)")
            .unwrap_or(Color::rgb(0, 0, 0));
        Self {
            text: TextStyle {
                font_family: "system-ui".into(),
                font_size: options.root_font_size,
                font_weight: 400,
                italic: false,
                underline: false,
                color: fg,
            },
            display: Display::Inline,
            background: Color {
                r: 0,
                g: 0,
                b: 0,
                a: 0,
            },
            margin: [Length::Px(0.); 4],
            padding: [Length::Px(0.); 4],
            border_width: [Length::Px(3.); 4],
            border_style: [BorderStyle::None; 4],
            border_color: [fg; 4],
            radii: [Length::Px(0.); 4],
            width: Length::Auto,
            height: Length::Auto,
            min_width: Length::Auto,
            max_width: Length::Auto,
            min_height: Length::Auto,
            max_height: Length::Auto,
            white_space: WhiteSpace::Normal,
            align: Align::Left,
            vertical: VerticalAlign::Baseline,
            line_height: LineHeight::Normal,
            relative: false,
            offsets: [Length::Auto; 4],
            list_style: "disc",
            vars,
        }
    }
    fn inherit(parent: &Self, options: &LayoutOptions) -> Self {
        Self {
            text: parent.text.clone(),
            white_space: parent.white_space,
            align: parent.align,
            line_height: parent.line_height,
            list_style: parent.list_style,
            vars: parent.vars.clone(),
            ..Self::initial(options, parent.vars.clone())
        }
    }
    pub fn line_pixels(&self, root: f32) -> f32 {
        match self.line_height {
            LineHeight::Normal => self.text.font_size * 1.2,
            LineHeight::Multiple(n) => self.text.font_size * n,
            LineHeight::Length(n) => n
                .resolve(self.text.font_size, root, self.text.font_size)
                .unwrap_or(self.text.font_size * 1.2),
        }
    }
    fn defaults(&mut self, tag: &str, parent_font: f32, options: &LayoutOptions) {
        self.display = match tag {
            "html" | "body" | "div" | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul"
            | "ol" => Display::Block,
            "li" => Display::ListItem,
            "head" | "style" | "script" => Display::None,
            _ => Display::Inline,
        };
        match tag {
            "html" => {
                self.background = options
                    .palette
                    .color("var(--background)")
                    .unwrap_or(Color::rgb(255, 255, 255))
            }
            "p" => {
                self.margin[0] = Length::Em(1.);
                self.margin[2] = Length::Em(1.);
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.text.font_size = parent_font
                    * match tag {
                        "h1" => 2.,
                        "h2" => 1.5,
                        "h3" => 1.17,
                        "h4" => 1.,
                        "h5" => 0.83,
                        _ => 0.67,
                    };
                self.text.font_weight = 700;
                self.margin[0] = Length::Em(0.67);
                self.margin[2] = Length::Em(0.67);
            }
            "ul" | "ol" => {
                self.list_style = if tag == "ol" { "decimal" } else { "disc" };
                self.padding[3] = Length::Em(2.);
                self.margin[0] = Length::Em(1.);
                self.margin[2] = Length::Em(1.);
            }
            "b" | "strong" => self.text.font_weight = 700,
            "i" | "em" | "var" => self.text.italic = true,
            "u" => self.text.underline = true,
            "big" => self.text.font_size = parent_font * 1.2,
            "small" => self.text.font_size = parent_font / 1.2,
            "a" => {
                self.text.underline = true;
                self.text.color = options
                    .palette
                    .color("var(--accent)")
                    .unwrap_or(self.text.color);
            }
            "tt" | "code" => self.text.font_family = "monospace".into(),
            _ => {}
        }
    }
    fn apply(&mut self, name: &str, value: &str, parent_font: f32, root: f32) {
        let positive = !matches!(name, "top" | "right" | "bottom" | "left" | "vertical-align");
        let len = || {
            Length::parse(value).filter(|length| {
                !positive
                    || match length {
                        Length::Px(n) | Length::Em(n) | Length::Rem(n) | Length::Percent(n) => {
                            *n >= 0.
                        }
                        Length::Auto => true,
                    }
            })
        };
        let color = || {
            if value == "currentColor" || value == "currentcolor" {
                Some(self.text.color)
            } else {
                Color::parse(value, &self.vars).ok()
            }
        };
        match name {
            "display" => {
                if let Some(d) = match value {
                    "none" => Some(Display::None),
                    "inline" => Some(Display::Inline),
                    "block" => Some(Display::Block),
                    "inline-block" => Some(Display::InlineBlock),
                    "list-item" => Some(Display::ListItem),
                    _ => None,
                } {
                    self.display = d
                }
            }
            "position" => {
                if ["static", "relative"].contains(&value) {
                    self.relative = value == "relative"
                }
            }
            "top" | "right" | "bottom" | "left" => {
                if let Some(n) = len() {
                    self.offsets[side(name).unwrap()] = n
                }
            }
            "width" => {
                if let Some(n) = len() {
                    self.width = n
                }
            }
            "height" => {
                if let Some(n) = len() {
                    self.height = n
                }
            }
            "min-width" => {
                if let Some(n) = len() {
                    self.min_width = n
                }
            }
            "max-width" => {
                if let Some(n) = len() {
                    self.max_width = n
                }
            }
            "min-height" => {
                if let Some(n) = len() {
                    self.min_height = n
                }
            }
            "max-height" => {
                if let Some(n) = len() {
                    self.max_height = n
                }
            }
            "color" => {
                if let Some(c) = color() {
                    self.text.color = c
                }
            }
            "background-color" => {
                if let Some(c) = color() {
                    self.background = c
                }
            }
            "font-family" => self.text.font_family = value.into(),
            "font-size" => {
                if let Some(n) = len()
                    .and_then(|l| l.resolve(parent_font, root, parent_font))
                    .filter(|n| *n > 0.)
                {
                    self.text.font_size = n
                }
            }
            "font-weight" => {
                if let Some(n) = match value {
                    "normal" => Some(400),
                    "bold" => Some(700),
                    _ => value
                        .parse::<u16>()
                        .ok()
                        .filter(|n| (100..=900).contains(n)),
                } {
                    self.text.font_weight = n
                }
            }
            "font-style" => {
                if ["normal", "italic"].contains(&value) {
                    self.text.italic = value == "italic"
                }
            }
            "text-decoration" => {
                if ["none", "underline"].contains(&value) {
                    self.text.underline = value == "underline"
                }
            }
            "white-space" => {
                if let Some(w) = match value {
                    "normal" => Some(WhiteSpace::Normal),
                    "nowrap" => Some(WhiteSpace::NoWrap),
                    "pre" => Some(WhiteSpace::Pre),
                    "pre-wrap" => Some(WhiteSpace::PreWrap),
                    _ => None,
                } {
                    self.white_space = w
                }
            }
            "text-align" => {
                if let Some(a) = match value {
                    "left" => Some(Align::Left),
                    "center" => Some(Align::Center),
                    "right" => Some(Align::Right),
                    _ => None,
                } {
                    self.align = a
                }
            }
            "vertical-align" => {
                if let Some(v) = match value {
                    "baseline" => Some(VerticalAlign::Baseline),
                    "top" | "text-top" => Some(VerticalAlign::Top),
                    "middle" => Some(VerticalAlign::Middle),
                    "bottom" | "text-bottom" => Some(VerticalAlign::Bottom),
                    "super" => Some(VerticalAlign::Super),
                    "sub" => Some(VerticalAlign::Sub),
                    _ => len().map(VerticalAlign::Offset),
                } {
                    self.vertical = v
                }
            }
            "line-height" => {
                if value == "normal" {
                    self.line_height = LineHeight::Normal
                } else if let Ok(n) = value.parse::<f32>() {
                    if n.is_finite() && n > 0. {
                        self.line_height = LineHeight::Multiple(n)
                    }
                } else if let Some(n) = len() {
                    self.line_height = LineHeight::Length(n)
                }
            }
            "list-style-type" => {
                if let Some(s) = match value {
                    "disc" => Some("disc"),
                    "circle" => Some("circle"),
                    "square" => Some("square"),
                    "decimal" => Some("decimal"),
                    "none" => Some("none"),
                    _ => None,
                } {
                    self.list_style = s
                }
            }
            "margin" | "padding" => {
                if let Some(v) = quad_lengths(value) {
                    if name == "margin" {
                        self.margin = v
                    } else {
                        self.padding = v
                    }
                }
            }
            "border-width" => {
                if let Some(v) = quad_lengths(value) {
                    self.border_width = v
                }
            }
            "border-radius" => {
                if let Some(v) = quad_lengths(value) {
                    self.radii = v
                }
            }
            "border-style" => {
                if let Some(v) = quad(value, |s| match s {
                    "none" => Some(BorderStyle::None),
                    "solid" => Some(BorderStyle::Solid),
                    _ => None,
                }) {
                    self.border_style = v
                }
            }
            "border-color" => {
                if let Some(v) = quad(value, |s| Color::parse(s, &self.vars).ok()) {
                    self.border_color = v
                }
            }
            "border" | "border-top" | "border-right" | "border-bottom" | "border-left" => {
                let mut width = Length::Px(3.);
                let mut style = BorderStyle::None;
                let mut color = self.text.color;
                let mut valid = true;
                for part in words(value) {
                    if let Some(n) = Length::parse(part) {
                        width = n
                    } else if part == "solid" {
                        style = BorderStyle::Solid
                    } else if part == "none" {
                        style = BorderStyle::None
                    } else if let Ok(c) = Color::parse(part, &self.vars) {
                        color = c
                    } else {
                        valid = false;
                    }
                }
                if valid {
                    for i in 0..4 {
                        if name == "border"
                            || name.strip_prefix("border-").and_then(side) == Some(i)
                        {
                            self.border_width[i] = width;
                            self.border_style[i] = style;
                            self.border_color[i] = color;
                        }
                    }
                }
            }
            _ => {
                if let Some(s) = name.strip_prefix("margin-").and_then(side)
                    && let Some(n) = len()
                {
                    self.margin[s] = n;
                }
                if let Some(s) = name.strip_prefix("padding-").and_then(side)
                    && let Some(n) = len()
                {
                    self.padding[s] = n;
                }
                if let Some(rest) = name.strip_prefix("border-") {
                    if let Some((s, key)) = rest.split_once('-')
                        && let Some(i) = side(s)
                    {
                        match key {
                            "width" => {
                                if let Some(n) = len() {
                                    self.border_width[i] = n
                                }
                            }
                            "style" => {
                                if let Some(s) = match value {
                                    "solid" => Some(BorderStyle::Solid),
                                    "none" => Some(BorderStyle::None),
                                    _ => None,
                                } {
                                    self.border_style[i] = s
                                }
                            }
                            "color" => {
                                if let Some(c) = color() {
                                    self.border_color[i] = c
                                }
                            }
                            _ => {}
                        }
                    }
                    if let Some(corner) = match rest {
                        "top-left-radius" => Some(0),
                        "top-right-radius" => Some(1),
                        "bottom-right-radius" => Some(2),
                        "bottom-left-radius" => Some(3),
                        _ => None,
                    } && let Some(n) = len()
                    {
                        self.radii[corner] = n;
                    }
                }
            }
        }
    }
}
fn side(name: &str) -> Option<usize> {
    match name {
        "top" => Some(0),
        "right" => Some(1),
        "bottom" => Some(2),
        "left" => Some(3),
        _ => None,
    }
}
pub(crate) fn words(value: &str) -> Vec<&str> {
    let mut depth = 0usize;
    let mut start = None;
    let mut out = Vec::new();
    let mut quote = None;
    for (i, c) in value.char_indices() {
        if quote == Some(c) {
            quote = None;
        } else if quote.is_none() && (c == '\'' || c == '"') {
            quote = Some(c);
        }
        if quote.is_none() {
            match c {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        if c.is_whitespace() && depth == 0 && quote.is_none() {
            if let Some(s) = start.take() {
                out.push(&value[s..i]);
            }
        } else {
            start.get_or_insert(i);
        }
    }
    if let Some(s) = start {
        out.push(&value[s..]);
    }
    out
}
fn quad<T: Copy>(text: &str, parse: impl Fn(&str) -> Option<T>) -> Option<[T; 4]> {
    let values: Option<Vec<_>> = words(text).into_iter().map(parse).collect();
    let v = values?;
    match v.as_slice() {
        [a] => Some([*a; 4]),
        [a, b] => Some([*a, *b, *a, *b]),
        [a, b, c] => Some([*a, *b, *c, *b]),
        [a, b, c, d] => Some([*a, *b, *c, *d]),
        _ => None,
    }
}
fn quad_lengths(value: &str) -> Option<[Length; 4]> {
    quad(value, |text| {
        Length::parse(text).filter(|l| match l {
            Length::Px(n) | Length::Em(n) | Length::Rem(n) | Length::Percent(n) => *n >= 0.,
            Length::Auto => true,
        })
    })
}
fn substitute(value: &str, vars: &BTreeMap<String, String>, depth: usize) -> Option<String> {
    if depth > 32 || value.len() > 65536 {
        return None;
    }
    let Some(at) = value.find("var(") else {
        return Some(value.into());
    };
    let tail = &value[at + 4..];
    let mut level = 1;
    let mut end = None;
    for (i, c) in tail.char_indices() {
        if c == '(' {
            level += 1;
        } else if c == ')' {
            level -= 1;
            if level == 0 {
                end = Some(i);
                break;
            }
        }
    }
    let end = end?;
    let body = &tail[..end];
    let parts = split_top(body, ',');
    let key = parts.first()?.trim();
    let resolved = if let Some(v) = vars.get(key) {
        substitute(v, vars, depth + 1)?
    } else {
        substitute(parts.get(1)?, vars, depth + 1)?
    };
    substitute(
        &format!("{}{}{}", &value[..at], resolved, &tail[end + 1..]),
        vars,
        depth + 1,
    )
}
pub(crate) fn compute(doc: &Document, options: &LayoutOptions) -> Vec<Style> {
    let variables = Arc::new(options.palette.variables.clone());
    let extra = parse_stylesheet(&options.extra_css);
    let dark = options.palette.is_dark();
    let mut out: Vec<Style> = Vec::with_capacity(doc.nodes.len());
    for (id, node) in doc.nodes.iter().enumerate() {
        let parent_font = node
            .parent
            .map_or(options.root_font_size, |p| out[p].text.font_size);
        let mut style = node.parent.map_or_else(
            || Style::initial(options, variables.clone()),
            |p| Style::inherit(&out[p], options),
        );
        if node.tag.is_empty() {
            out.push(style);
            continue;
        }
        style.defaults(&node.tag, parent_font, options);
        if doc.inline && id == 0 {
            style.background.a = 0;
        }
        let mut declarations = Vec::new();
        for (order, rule) in doc.rules.iter().chain(&extra).enumerate() {
            if rule.selector.matches(doc, id, dark) {
                for (index, d) in rule.declarations.iter().enumerate() {
                    declarations.push((d.important, rule.selector.specificity, order, index, d));
                }
            }
        }
        for (index, d) in node.declarations.iter().enumerate() {
            declarations.push((d.important, u32::MAX, usize::MAX, index, d));
        }
        declarations.sort_by_key(|(important, specificity, order, index, _)| {
            (*important, *specificity, *order, *index)
        });
        for (_, _, _, _, d) in &declarations {
            if d.name.starts_with("--") {
                Arc::make_mut(&mut style.vars).insert(d.name.clone(), d.value.clone());
            }
        }
        // Resolve font-size before other em-based properties; each declaration uses the parent size.
        for (_, _, _, _, d) in &declarations {
            if d.name == "font-size"
                && let Some(value) = substitute(&d.value, &style.vars, 0)
            {
                style.apply(&d.name, &value, parent_font, options.root_font_size);
            }
        }
        for (_, _, _, _, d) in declarations {
            if !d.name.starts_with("--")
                && d.name != "font-size"
                && let Some(value) = substitute(&d.value, &style.vars, 0)
            {
                style.apply(&d.name, &value, parent_font, options.root_font_size);
            }
        }
        if doc.inline {
            style.display = if id <= 1 {
                Display::Block
            } else {
                Display::Inline
            };
            style.margin = [Length::Px(0.); 4];
            style.padding = [Length::Px(0.); 4];
        }
        out.push(style);
    }
    out
}
