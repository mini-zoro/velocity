//! Standalone highlighted HTML for clipboard and browser printing.
//!
//! Spans use the line-local UTF-8 offsets from `vel_highlight`. The caller must
//! supply spans for the same text snapshot and palette (never a stale batch).
#![forbid(unsafe_code)]

use std::{fmt::Write as _, io::Write as _, ops::Range, path::PathBuf};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
use vel_buffer::{Region, Selection};
use vel_color_scheme::{Color, ColorScheme, FontStyle, Style};
use vel_highlight::StyledLine;
use vel_text::Text;

/// Presentation settings, independent of editor or platform state.
#[derive(Clone, Debug)]
pub struct Options {
    /// Escaped document title.
    pub title: String,
    /// One literal font family; safely quoted as a CSS string.
    pub font_family: String,
    /// Positive, finite font size in points.
    pub font_size: f32,
    /// Tab stops in display columns, in the range 1..=256.
    pub tab_size: usize,
    /// Include original one-based document line numbers (excluded from plain text).
    pub line_numbers: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            title: "Velocity export".into(),
            font_family: "monospace".into(),
            font_size: 12.,
            tab_size: 4,
            line_numbers: false,
        }
    }
}

/// Both clipboard flavours. Plain text preserves source tabs and Unicode exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HtmlExport {
    /// Complete UTF-8 HTML document, usable as the `text/html` clipboard flavour.
    pub html: String,
    /// Selected text, usable as the `text/plain` clipboard flavour.
    pub plain_text: String,
}

/// Invalid options, source ranges or line-local highlight spans.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    /// A range is reversed, outside the document, or splits a UTF-8 scalar.
    #[error("invalid UTF-8 source range")]
    Range,
    /// Highlights must be sorted unique lines with sorted non-overlapping spans.
    #[error("invalid line-local highlight spans")]
    Highlights,
    /// Font size must be positive and finite; tab size must be 1..=256.
    #[error("invalid export options")]
    Options,
    /// The clipboard representation would exceed the in-memory output limit.
    #[error("HTML clipboard output exceeds its size limit")]
    Size,
}

/// Exports a half-open UTF-8 byte range, including an empty range at EOF.
pub fn export_range(
    text: &Text,
    range: Range<usize>,
    highlighted: &[StyledLine],
    scheme: &ColorScheme,
    options: &Options,
) -> Result<HtmlExport, Error> {
    render(text, &[range], highlighted, scheme, options, false)
}

/// Exports explicit source ranges with stylesheet-based highlighting.
pub fn export_ranges(
    text: &Text,
    ranges: &[Range<usize>],
    highlighted: &[StyledLine],
    scheme: &ColorScheme,
    options: &Options,
) -> Result<HtmlExport, Error> {
    render(text, ranges, highlighted, scheme, options, false)
}

/// Exports explicit source ranges with inline styles: rich paste targets may
/// import the body and discard the document's stylesheet. Caps HTML at 2 MiB;
/// callers must separately bound input size and syntax analysis.
pub fn export_clipboard(
    text: &Text,
    ranges: &[Range<usize>],
    highlighted: &[StyledLine],
    scheme: &ColorScheme,
    options: &Options,
) -> Result<HtmlExport, Error> {
    render(text, ranges, highlighted, scheme, options, true)
}

/// Copies nonempty selections in document order, separated by LF.
///
/// With only carets, copies their complete LF lines, deduplicated, matching the
/// ordinary copy adapter's line-copy convention. Adjacent source ranges join
/// without an extra LF when the previous range already ends in LF. Mixed empty
/// and nonempty selections omit the carets.
pub fn copy_as_html(
    text: &Text,
    selection: &Selection,
    highlighted: &[StyledLine],
    scheme: &ColorScheme,
    options: &Options,
) -> Result<HtmlExport, Error> {
    let nonempty = selection.iter().any(|r| !r.empty());
    let ranges = if nonempty {
        selection.iter().filter(|r| !r.empty()).collect::<Vec<_>>()
    } else {
        let mut lines = Vec::new();
        for r in selection.iter() {
            validate_range(text, &r.to_range())?;
            let line = text.byte_to_line(r.b);
            let range = Region::new(text.line_to_byte(line), full_line_end(text, line));
            if lines.last() != Some(&range) {
                lines.push(range);
            }
        }
        lines
    };
    render(
        text,
        &ranges.iter().map(|r| r.to_range()).collect::<Vec<_>>(),
        highlighted,
        scheme,
        options,
        false,
    )
}

/// Writes the exact standalone document to a unique `.html` file and returns it.
///
/// The file survives this call so an asynchronously opened browser can read it.
/// The application owns cleanup after the browser has finished. No process is
/// launched and no print dialog is triggered by this library.
pub fn write_print_file(export: &HtmlExport) -> std::io::Result<PathBuf> {
    let mut file = tempfile::Builder::new()
        .prefix("velocity-print-")
        .suffix(".html")
        .tempfile()?;
    file.write_all(export.html.as_bytes())?;
    file.flush()?;
    let (_, path) = file.keep().map_err(|e| e.error)?;
    Ok(path)
}

/// Writes a print page and returns its cleanup guard. Hold it until the browser
/// has had time to load the file. Only this disk export gets an executable script.
pub fn temporary_print_file(export: &HtmlExport) -> std::io::Result<tempfile::TempPath> {
    let (head, body) = export
        .html
        .split_once("</head>")
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "missing HTML head"))?;
    let mut file = tempfile::Builder::new()
        .prefix("velocity-print-")
        .suffix(".html")
        .tempfile()?;
    file.write_all(head.as_bytes())?;
    file.write_all(
        b"<script>addEventListener('load', () => window.print(), {once: true});</script></head>",
    )?;
    file.write_all(body.as_bytes())?;
    file.flush()?;
    Ok(file.into_temp_path())
}

fn full_line_end(text: &Text, line: usize) -> usize {
    if line + 1 < text.len_lines() {
        text.line_to_byte(line + 1)
    } else {
        text.len_bytes()
    }
}
fn validate_range(text: &Text, range: &Range<usize>) -> Result<(), Error> {
    if range.start > range.end
        || range.end > text.len_bytes()
        || !text.is_char_boundary(range.start)
        || !text.is_char_boundary(range.end)
    {
        return Err(Error::Range);
    }
    Ok(())
}
fn escape(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            // CR would otherwise be normalized by the HTML parser.
            '\r' => out.push_str("&#13;"),
            '\0' => out.push_str("&#xfffd;"),
            _ => out.push(c),
        }
    }
}
fn css_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        if matches!(c, '"' | '\\' | '<' | '>' | '&') || c.is_control() {
            write!(out, "\\{:x} ", c as u32).unwrap();
        } else {
            out.push(c);
        }
    }
    out.push('"');
    out
}
fn color(c: Color) -> String {
    if c.a == 255 {
        format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a)
    }
}
fn style_css(style: Style) -> String {
    let mut css = format!(
        "color:{};background-color:{};",
        color(style.foreground),
        color(style.background)
    );
    let font = style.font_style;
    if font.contains(FontStyle::BOLD) {
        css.push_str("font-weight:bold;");
    }
    if font.contains(FontStyle::ITALIC) {
        css.push_str("font-style:italic;");
    }
    if font.contains(FontStyle::SQUIGGLY_UNDERLINE) {
        css.push_str("text-decoration:underline wavy;");
    } else if font.contains(FontStyle::STIPPLED_UNDERLINE) {
        css.push_str("text-decoration:underline dotted;");
    } else if font.contains(FontStyle::UNDERLINE) {
        css.push_str("text-decoration:underline;");
    }
    if font.contains(FontStyle::GLOW) {
        write!(css, "text-shadow:0 0 2px {};", color(style.foreground)).unwrap();
    }
    css
}

fn render(
    text: &Text,
    ranges: &[Range<usize>],
    highlighted: &[StyledLine],
    scheme: &ColorScheme,
    options: &Options,
    clipboard: bool,
) -> Result<HtmlExport, Error> {
    const MAX_CLIPBOARD_HTML: usize = 2 * 1024 * 1024;
    let check_size = |size| {
        if clipboard && size > MAX_CLIPBOARD_HTML {
            Err(Error::Size)
        } else {
            Ok(())
        }
    };
    if !options.font_size.is_finite()
        || options.font_size <= 0.
        || !(1..=256).contains(&options.tab_size)
    {
        return Err(Error::Options);
    }
    for range in ranges {
        validate_range(text, range)?;
    }
    let mut previous = None;
    for line in highlighted {
        if line.line >= text.len_lines() || previous.is_some_and(|n| n >= line.line) {
            return Err(Error::Highlights);
        }
        previous = Some(line.line);
        let base = text.line_to_byte(line.line);
        let len = full_line_end(text, line.line) - base;
        let mut end = 0;
        for span in line.spans.iter() {
            if span.range.start < end
                || span.range.start > span.range.end
                || span.range.end > len
                || !text.is_char_boundary(base + span.range.start)
                || !text.is_char_boundary(base + span.range.end)
            {
                return Err(Error::Highlights);
            }
            end = span.range.end;
        }
    }
    let default = scheme.default_style();
    let mut body = String::new();
    let mut plain_text = String::new();
    // One class per resolved style. Schemes typically have only a few dozen styles.
    let mut styles = Vec::new();
    let mut inline_styles = Vec::new();
    let mut columns = Vec::new();
    for (index, range) in ranges.iter().enumerate() {
        if index > 0 && !plain_text.ends_with('\n') {
            plain_text.push('\n');
            body.push('\n');
        }
        plain_text.push_str(&text.slice(range.clone()).to_string());
        let mut at = range.start;
        loop {
            let row = text.byte_to_line(at);
            let base = text.line_to_byte(row);
            let end = range.end.min(full_line_end(text, row));
            let source = text.slice(base..full_line_end(text, row)).to_string();
            if options.line_numbers {
                write!(
                    body,
                    "<span class=\"line-number\" aria-hidden=\"true\">{}</span>",
                    row + 1
                )
                .unwrap();
            }
            let pos = highlighted.partition_point(|l| l.line < row);
            let spans = highlighted
                .get(pos)
                .filter(|l| l.line == row && l.exact)
                .map(|l| l.spans.as_ref())
                .unwrap_or(&[]);
            // Display columns are measured on complete graphemes, even when a style
            // splits a combining sequence. Range-prefix columns preserve tab stops.
            columns.clear();
            let mut column = 0;
            for (offset, grapheme) in source.grapheme_indices(true) {
                columns.push((offset, column));
                if grapheme == "\t" {
                    column += options.tab_size - column % options.tab_size;
                } else {
                    column += UnicodeWidthStr::width(grapheme);
                }
            }
            let mut local = at - base;
            while local < end - base {
                let span_at = spans.partition_point(|s| s.range.end <= local);
                let span = spans.get(span_at);
                let (style, stop) = if let Some(span) = span.filter(|s| s.range.start <= local) {
                    (span.style, span.range.end.min(end - base))
                } else {
                    (
                        default,
                        span.map_or(end - base, |s| s.range.start.min(end - base)),
                    )
                };
                let style_id = styles.iter().position(|s| *s == style).unwrap_or_else(|| {
                    styles.push(style);
                    if clipboard {
                        inline_styles.push(style_css(style));
                    }
                    styles.len() - 1
                });
                if clipboard {
                    write!(body, "<span style=\"{}\">", inline_styles[style_id]).unwrap();
                } else {
                    write!(body, "<span class=\"s{style_id}\">").unwrap();
                }
                let mut tail = local;
                for (offset, c) in source[local..stop].char_indices() {
                    if c == '\t' {
                        escape(&mut body, &source[tail..local + offset]);
                        let i = columns
                            .partition_point(|(p, _)| *p <= local + offset)
                            .saturating_sub(1);
                        let col = columns[i].1;
                        let width = options.tab_size - col % options.tab_size;
                        check_size(body.len() + width)?;
                        body.extend(std::iter::repeat_n(' ', width));
                        tail = local + offset + 1;
                    }
                }
                escape(&mut body, &source[tail..stop]);
                body.push_str("</span>");
                check_size(body.len())?;
                local = stop;
            }
            if end == range.end {
                break;
            }
            at = end;
        }
    }
    let mut html = String::from("<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>");
    escape(&mut html, &options.title);
    if clipboard {
        html.push_str("</title></head><body><pre style=\"");
        let pre = format!(
            "margin:0;white-space:pre;color:{};background-color:{};font-family:{},monospace;font-size:{}pt;",
            color(default.foreground),
            color(default.background),
            css_string(&options.font_family),
            options.font_size,
        );
        escape(&mut html, &pre);
        html.push_str("\">");
        check_size(html.len() + body.len() + "</pre></body></html>\n".len())?;
        html.push_str(&body);
        html.push_str("</pre></body></html>\n");
        return Ok(HtmlExport { html, plain_text });
    }
    html.push_str("</title><style>\n");
    write!(html, "body{{margin:0;padding:1em;color:{};background-color:{};}}\npre{{margin:0;white-space:pre;font-family:{},monospace;font-size:{}pt;line-height:1.4;tab-size:{};}}\n",color(default.foreground),color(default.background),css_string(&options.font_family),options.font_size,options.tab_size).unwrap();
    let gutter = scheme
        .globals
        .color("gutter_foreground")
        .unwrap_or(default.foreground);
    writeln!(html, ".line-number{{display:inline-block;min-width:4ch;padding-right:1ch;text-align:right;color:{};user-select:none;}}", color(gutter)).unwrap();
    html.push_str("@media print{body{padding:0;}pre{white-space:pre-wrap;overflow-wrap:anywhere;}*{print-color-adjust:exact;-webkit-print-color-adjust:exact;}}\n");
    for (i, style) in styles.into_iter().enumerate() {
        writeln!(html, ".s{i}{{{}}}", style_css(style)).unwrap();
    }
    html.push_str("</style></head><body><pre>");
    html.push_str(&body);
    html.push_str("</pre></body></html>\n");
    Ok(HtmlExport { html, plain_text })
}
