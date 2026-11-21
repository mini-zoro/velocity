//! Headless minihtml: a forgiving HTML/CSS subset and a renderer-independent display list.
//!
//! [`Document::parse`] compiles markup and styles once. [`layout`] accepts caller-provided
//! shaping/metrics, a global color palette and a width constraint. URLs are returned as data;
//! this crate never opens a browser, executes a command, reads an image or initializes a GPU.
#![forbid(unsafe_code)]
mod css;
mod dom;
mod layout;
pub use css::{Align, BorderStyle, Display, Length, TextStyle, VerticalAlign, WhiteSpace};
pub use dom::{Document, TAGS};
pub use layout::layout;
use serde_json::Value;
pub use vel_color_scheme::Color;
pub use vel_theme::Palette;

/// Malformed command URL or oversized document. Invalid CSS declarations are ignored.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Input or DOM nesting exceeds bounded parser limits.
    #[error("minihtml input limit: {0}")]
    Limit(&'static str),
    /// Malformed `subl:` command payload.
    #[error("invalid command URL: {0}")]
    Command(String),
}
/// Logical dimensions in device-independent pixels.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Size {
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}
/// Logical rectangle, with top-left coordinates.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}
impl Rect {
    /// Tests a point, including left/top and excluding right/bottom boundaries.
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}
/// Metrics of a shaped string, supplied by the application's font engine.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct TextMetrics {
    /// Horizontal advance.
    pub advance: f32,
    /// Baseline distance from the top.
    pub ascent: f32,
    /// Distance below the baseline.
    pub descent: f32,
}
/// The caller shapes complete candidate runs; the engine never estimates glyph widths.
pub trait FontMetrics {
    /// Shape/measure `text` with the requested font, size and decorations. Cache shaping here.
    fn measure(&self, text: &str, style: &TextStyle) -> TextMetrics;
    /// Intrinsic logical image dimensions. Unsupported or pending images return None.
    fn image_size(&self, _source: &ImageSource) -> Option<Size> {
        None
    }
}
/// Link action description. Navigation and permission decisions stay with the host.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkTarget {
    /// HTTP(S) URL for the host's browser callback.
    Browser(String),
    /// Parsed command and JSON-object arguments for a `subl:` URL.
    Command {
        /// Command name.
        name: String,
        /// Argument object; absent arguments become `{}`.
        args: Value,
    },
    /// Other URL, delivered to the application's on_navigate callback.
    Callback(String),
}
impl LinkTarget {
    /// Parses documented `subl:name {args}` URLs without invoking anything.
    pub fn parse(href: &str) -> Result<Self, Error> {
        let href = href.trim();
        if href
            .get(..5)
            .is_some_and(|s| s.eq_ignore_ascii_case("subl:"))
        {
            let command = &href[5..];
            let command = command.trim();
            let at = command.find(char::is_whitespace).unwrap_or(command.len());
            let name = &command[..at];
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(Error::Command(href.into()));
            }
            let args = command[at..].trim();
            let args = if args.is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str::<Value>(args).map_err(|e| Error::Command(e.to_string()))?
            };
            if !args.is_object() {
                return Err(Error::Command("arguments must be object".into()));
            }
            Ok(Self::Command {
                name: name.into(),
                args,
            })
        } else if href
            .get(..7)
            .is_some_and(|s| s.eq_ignore_ascii_case("http://"))
            || href
                .get(..8)
                .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
        {
            Ok(Self::Browser(href.into()))
        } else {
            Ok(Self::Callback(href.into()))
        }
    }
}
/// Supported image protocols. Decoding PNG/JPEG/GIF is a host responsibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageSource {
    /// `file://` URL.
    File(String),
    /// `res://` package resource URL.
    Resource(String),
    /// `data:image/png`, `image/jpeg` or `image/gif` URL.
    Data(String),
    /// HTTPS URL. The host decides whether to fetch it.
    Https(String),
}
impl ImageSource {
    /// Rejects unsupported protocols and MIME types; does not perform I/O.
    pub fn parse(source: &str) -> Option<Self> {
        if source.starts_with("file://") {
            Some(Self::File(source.into()))
        } else if source.starts_with("res://") {
            Some(Self::Resource(source.into()))
        } else if source.len() <= 4096
            && source
                .get(..8)
                .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
        {
            // Normalize the scheme only; preserve case-sensitive paths and queries.
            Some(Self::Https(format!("https://{}", &source[8..])))
        } else if [
            "data:image/png;",
            "data:image/png,",
            "data:image/jpeg;",
            "data:image/jpeg,",
            "data:image/gif;",
            "data:image/gif,",
        ]
        .iter()
        .any(|p| source.starts_with(p))
        {
            Some(Self::Data(source.into()))
        } else {
            None
        }
    }
}
/// One painted border edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Border {
    /// Width in logical pixels.
    pub width: f32,
    /// Solid or absent.
    pub style: BorderStyle,
    /// Edge color.
    pub color: Color,
}
/// Renderable output in painter's order, with parent backgrounds before children.
#[derive(Debug, Clone, PartialEq)]
pub enum DisplayItem {
    /// CSS border box (also emitted for transparent boxes with borders).
    Box {
        /// Outer border bounds.
        rect: Rect,
        /// Background fill.
        background: Color,
        /// Edges in CSS order: top, right, bottom, left.
        borders: [Border; 4],
        /// Radii in clockwise order starting at top-left.
        radii: [f32; 4],
    },
    /// Text run to shape/paint with the same metrics provider.
    Text {
        /// Run bounds; `baseline` is relative to this rectangle's top.
        rect: Rect,
        /// Baseline offset.
        baseline: f32,
        /// Unicode text.
        text: String,
        /// Computed font and paint style.
        style: TextStyle,
    },
    /// Image bounds and source; the host resolves/decodes the resource.
    Image {
        /// Image content bounds.
        rect: Rect,
        /// Supported URL.
        source: ImageSource,
    },
}
impl DisplayItem {
    /// Geometry shared by all display-list items.
    pub fn rect(&self) -> Rect {
        match self {
            Self::Box { rect, .. } | Self::Text { rect, .. } | Self::Image { rect, .. } => *rect,
        }
    }
    pub(crate) fn rect_mut(&mut self) -> &mut Rect {
        match self {
            Self::Box { rect, .. } | Self::Text { rect, .. } | Self::Image { rect, .. } => rect,
        }
    }
}
/// Clickable portion of a link; wrapped links have one region per fragment.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkRegion {
    /// Hit rectangle.
    pub rect: Rect,
    /// Parsed action.
    pub target: LinkTarget,
}
/// Tooltip region, inherited by descendants until overridden.
#[derive(Debug, Clone, PartialEq)]
pub struct TooltipRegion {
    /// Hit rectangle.
    pub rect: Rect,
    /// Entity-decoded tooltip text.
    pub title: String,
}
/// Layout result. All geometry uses logical pixels.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DisplayList {
    /// Painter-order boxes, text and images.
    pub items: Vec<DisplayItem>,
    /// Link fragments in document order.
    pub links: Vec<LinkRegion>,
    /// Tooltip fragments in document order.
    pub tooltips: Vec<TooltipRegion>,
    /// Actual scrollable extent, including overflowing unbreakable content.
    pub size: Size,
    /// Height needed to show the whole document.
    pub scroll_height: f32,
}
impl DisplayList {
    /// Topmost clickable link at a logical point.
    pub fn link_at(&self, x: f32, y: f32) -> Option<&LinkTarget> {
        self.links
            .iter()
            .rev()
            .find(|r| r.rect.contains(x, y))
            .map(|r| &r.target)
    }
    /// Innermost tooltip at a logical point.
    pub fn tooltip_at(&self, x: f32, y: f32) -> Option<&str> {
        self.tooltips
            .iter()
            .rev()
            .find(|r| r.rect.contains(x, y))
            .map(|r| r.title.as_str())
    }
}
/// Width, root font and color-scheme context. Reuse these across popup relayouts.
#[derive(Debug, Clone)]
pub struct LayoutOptions {
    /// Available width (finite, positive; invalid input is clamped to one pixel).
    pub max_width: f32,
    /// Root font size in logical pixels.
    pub root_font_size: f32,
    /// Global scheme variables and implicit dark/light class.
    pub palette: Palette,
    /// Additional scheme popup/phantom CSS, appended after plugin styles.
    pub extra_css: String,
}
impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            max_width: 480.,
            root_font_size: 13.,
            palette: Palette::default(),
            extra_css: String::new(),
        }
    }
}
