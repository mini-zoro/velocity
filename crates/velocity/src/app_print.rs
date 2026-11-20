//! Print the committed buffer snapshot without parsing on the window thread.
use super::App;
use crate::ui_text::UiText;
use std::{
    ops::Range,
    sync::mpsc,
    time::{Duration, Instant},
};
use tempfile::TempPath;
use vel_export::Options;
use vel_highlight::{DocumentHighlighter, detect_syntax};
use vel_platform::{BrowserOpener, WindowHandle};

const LOAD_GRACE: Duration = Duration::from_secs(600);
const MAX_PREFIX_BYTES: usize = 8 * 1024 * 1024;
const MAX_PREFIX_LINES: usize = 250_000;

type PrintResult = Result<TempPath, UiText>;

#[derive(Debug, Default)]
pub(super) struct Jobs {
    pending: Option<mpsc::Receiver<PrintResult>>,
    handed_off: Vec<(Instant, TempPath)>,
}

impl Jobs {
    pub(super) fn active(&self) -> bool {
        self.pending.is_some() || !self.handed_off.is_empty()
    }

    pub(super) fn poll(
        &mut self,
        opener: &dyn BrowserOpener,
        now: Instant,
    ) -> Option<Result<(), UiText>> {
        self.handed_off.retain(|(until, _)| *until > now);
        let ready = match self.pending.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err(prepare_error()),
        };
        self.pending = None;
        Some(ready.and_then(|path| {
            if !opener.open_browser_file(&path) {
                return Err(UiText::tr(
                    "Could not open the print document in the default browser",
                ));
            }
            self.handed_off.push((now + LOAD_GRACE, path));
            Ok(())
        }))
    }
}

impl App {
    pub(super) fn start_print(&mut self) -> Result<(), String> {
        if self.print_jobs.pending.is_some() {
            return Ok(());
        }
        let tab = &self.workspace.tabs[self.workspace.active];
        let editor = &tab.editor;
        let text = editor.buffer.text().clone();
        let mut ranges: Vec<Range<usize>> = editor
            .buffer
            .view(editor.id)
            .sel()
            .iter()
            .filter(|region| !region.empty())
            .map(|region| region.to_range())
            .collect();
        if ranges.is_empty() {
            ranges.push(0..text.len_bytes());
        }
        let first = text.byte_to_line(ranges[0].start);
        let last = text.byte_to_line(ranges.last().unwrap().end.saturating_sub(1));
        let end = if last + 1 == text.len_lines() {
            text.len_bytes()
        } else {
            text.line_to_byte(last + 1)
        };
        if end > MAX_PREFIX_BYTES || last >= MAX_PREFIX_LINES {
            return Err("Document or selection is too large to print".into());
        }

        let language = editor.language.borrow();
        let language = language
            .as_ref()
            .ok_or("Syntax highlighting is unavailable")?;
        let syntax_path = language.syntax.clone();
        let assets = language.assets.clone();
        let scheme = language.scheme.clone();
        let version = editor.buffer.change_id();
        let options = Options {
            title: tab.name(),
            font_family: editor.preferences.font_face.clone(),
            font_size: editor.preferences.font_size as f32,
            tab_size: editor.preferences.tab_size.clamp(1, 256),
            line_numbers: false,
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let render = || {
                let syntax = detect_syntax(&assets.syntaxes, "", "", Some(&syntax_path))
                    .ok_or_else(|| UiText::tr("Syntax highlighting is unavailable"))?;
                let prefix = vel_text::Text::from(text.slice(0..end).to_string());
                let mut highlighter = DocumentHighlighter::new(
                    prefix,
                    version,
                    assets.syntaxes.clone(),
                    syntax,
                    scheme.clone(),
                );
                let batch = highlighter
                    .request_lines(version, first..last + 1, Duration::from_secs(30))
                    .map_err(|_| prepare_error())?;
                if !batch.complete {
                    return Err(prepare_error());
                }
                let export = vel_export::export_ranges(
                    &text,
                    &ranges,
                    &batch.highlighted,
                    &scheme,
                    &options,
                )
                .map_err(|_| prepare_error())?;
                vel_export::temporary_print_file(&export).map_err(|_| prepare_error())
            };
            let _ = tx.send(render());
        });
        self.print_jobs.pending = Some(rx);
        Ok(())
    }

    pub(super) fn poll_print(&mut self, window: &WindowHandle, now: Instant) -> bool {
        let result = self.print_jobs.poll(window, now);
        if self.print_jobs.pending.is_some() {
            window.request_frame_at(now + Duration::from_millis(30));
        }
        if let Some((until, _)) = self.print_jobs.handed_off.first() {
            window.request_frame_at(*until);
        }
        if let Some(Err(error)) = result {
            self.panels.set_message(error);
            true
        } else {
            false
        }
    }
}

fn prepare_error() -> UiText {
    UiText::tr("Could not prepare the print document")
}
