//! Highlight a bounded, current text snapshot for the rich pasteboard.
use super::App;
use std::{ops::Range, time::Duration};
use vel_export::{HtmlExport, Options};
use vel_highlight::{DocumentHighlighter, detect_syntax};
use vel_platform::RichClipboard;

// A selection still needs its syntax context (e.g. a string started above it).
// Cap the parse prefix, not just the selected bytes, and never parse a 10 MB
// document or write a partial result on the UI thread.
const MAX_PREFIX_BYTES: usize = 256 * 1024;
const MAX_PREFIX_LINES: usize = 4096;

impl App {
    pub(super) fn copy_as_html(
        &mut self,
        clipboard: Option<&dyn RichClipboard>,
    ) -> Result<(), String> {
        let tab = &self.workspace.tabs[self.workspace.active];
        let editor = &tab.editor;
        // Selection offsets belong to the committed buffer, not an IME preedit
        // preview that may have replaced those bytes for display.
        let text = editor.buffer.text();
        let selection = editor.buffer.view(editor.id).sel();
        let limit = "Selection is too large to copy as HTML";
        let mut ranges: Vec<Range<usize>> = Vec::new();
        for region in selection.iter().filter(|region| !region.empty()) {
            if region.end() > MAX_PREFIX_BYTES || ranges.len() >= MAX_PREFIX_LINES {
                return Err(limit.into());
            }
            ranges.push(region.to_range());
        }
        if ranges.is_empty() {
            if text.len_bytes() > MAX_PREFIX_BYTES {
                return Err(limit.into());
            }
            ranges.push(0..text.len_bytes());
        }
        let first = text.byte_to_line(ranges[0].start);
        let last = text.byte_to_line(ranges.last().unwrap().end.saturating_sub(1));
        let prefix_end = if last + 1 == text.len_lines() {
            text.len_bytes()
        } else {
            text.line_to_byte(last + 1)
        };
        if prefix_end > MAX_PREFIX_BYTES || last >= MAX_PREFIX_LINES {
            return Err(limit.into());
        }
        // Only give the parser the required prefix. DocumentHighlighter allocates
        // one checkpoint slot per line in its input at construction.
        let snapshot = vel_text::Text::from(text.slice(0..prefix_end).to_string());
        let language_slot = editor.language.borrow();
        let language = language_slot
            .as_ref()
            .ok_or("Syntax highlighting is unavailable")?;
        let assets = &language.assets;
        let syntax = detect_syntax(&assets.syntaxes, "", "", Some(&language.syntax))
            .ok_or("Syntax highlighting is unavailable")?;
        let version = editor.buffer.change_id();
        let mut highlighter = DocumentHighlighter::new(
            snapshot,
            version,
            assets.syntaxes.clone(),
            syntax,
            language.scheme.clone(),
        )
        .with_metadata(assets.metadata.clone());
        let batch = highlighter
            .request_lines(version, first..last + 1, Duration::from_millis(250))
            .map_err(|_| "Could not highlight the selection")?;
        if !batch.complete {
            return Err("Could not highlight the selection".into());
        }
        let HtmlExport {
            html,
            mut plain_text,
        } = vel_export::export_clipboard(
            text,
            &ranges,
            &batch.highlighted,
            &language.scheme,
            &Options {
                title: tab.name(),
                font_family: editor.preferences.font_face.clone(),
                font_size: editor.preferences.font_size as f32,
                tab_size: editor.preferences.tab_size.clamp(1, 256),
                line_numbers: false,
            },
        )
        .map_err(|e| match e {
            vel_export::Error::Size => limit,
            _ => "Could not highlight the selection",
        })?;
        if tab.format.line_ending != vel_fileio::LineEnding::Lf {
            plain_text = plain_text.replace('\n', tab.format.line_ending.as_str());
        }
        if clipboard.is_some_and(|clipboard| !clipboard.set_clipboard_html(&html, &plain_text)) {
            return Err("Could not write to the clipboard".into());
        }
        drop(language_slot);
        // Do not reuse the multicaret/whole-line metadata of an earlier plain Copy.
        crate::editing::forget_copy();
        self.workspace.editor_mut().clipboard = plain_text;
        Ok(())
    }
}
