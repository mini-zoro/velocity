//! App-owned UI copy. Keep localized templates separate from paths, commands,
//! diagnostic details and other data that must not be translated.
use std::{borrow::Cow, fmt, ops::Deref};
use vel_i18n::Translator;

#[derive(Clone, Debug)]
struct Source {
    key: &'static str,
    args: Vec<(&'static str, UiText)>,
    revision: Option<u64>,
    suffix: String,
}

/// A message that may outlive a language switch. Raw text from a package,
/// subprocess or user has no source key and never goes through the catalog.
#[derive(Clone, Debug, Default)]
pub(crate) struct UiText {
    text: String,
    source: Option<Box<Source>>,
}

impl UiText {
    /// The command interface still returns strings. Only recognize app-owned
    /// error messages here; underlying diagnostics and paths are opaque.
    pub fn command_error(detail: &str) -> Self {
        match detail {
            "document is read only" => Self::tr("document is read only"),
            "A target has unsaved edits; save it before replacing in files" => {
                Self::tr("A target has unsaved edits; save it before replacing in files")
            }
            "Reopen requires confirmation of unsaved changes" => {
                Self::tr("Reopen requires confirmation of unsaved changes")
            }
            "Unsaved changes require a save decision" => {
                Self::tr("Unsaved changes require a save decision")
            }
            "Enter one file or folder name, without a path" => {
                Self::tr("Enter one file or folder name, without a path")
            }
            "Cannot move a filesystem root to the Trash" => {
                Self::tr("Cannot move a filesystem root to the Trash")
            }
            "Choose a macro file name inside Packages/User" => {
                Self::tr("Choose a macro file name inside Packages/User")
            }
            "No profile directory for saving macros" => {
                Self::tr("No profile directory for saving macros")
            }
            "Selection is too large to copy as HTML" => {
                Self::tr("Selection is too large to copy as HTML")
            }
            "Could not highlight the selection" => Self::tr("Could not highlight the selection"),
            "Syntax highlighting is unavailable" => Self::tr("Syntax highlighting is unavailable"),
            "Could not write to the clipboard" => Self::tr("Could not write to the clipboard"),
            _ => detail.into(),
        }
    }

    pub fn tr(key: &'static str) -> Self {
        Self::tr_fmt(key, [])
    }

    pub fn tr_fmt<const N: usize>(key: &'static str, args: [(&'static str, String); N]) -> Self {
        Self::compose(key, args.map(|(name, value)| (name, value.into())))
    }

    pub fn compose<const N: usize>(key: &'static str, args: [(&'static str, UiText); N]) -> Self {
        Self {
            text: if N == 0 {
                key.into()
            } else {
                vel_i18n::format_named(key, &args)
            },
            source: Some(Box::new(Source {
                key,
                args: Vec::from(args),
                revision: None,
                suffix: String::new(),
            })),
        }
    }

    pub fn refresh(&mut self, tr: &Translator) -> bool {
        if let Some(source) = &mut self.source
            && source.revision != Some(tr.revision())
        {
            let mut text = if source.args.is_empty() {
                tr.tr(source.key).into_owned()
            } else {
                for (_, arg) in &mut source.args {
                    arg.refresh(tr);
                }
                tr.tr_fmt(
                    source.key,
                    &source
                        .args
                        .iter()
                        .map(|(key, value)| (*key, value.to_string()))
                        .collect::<Vec<_>>(),
                )
            };
            text.push_str(&source.suffix);
            source.revision = Some(tr.revision());
            if text != self.text {
                self.text = text;
                return true;
            }
        }
        false
    }

    pub fn with_raw_suffix(mut self, suffix: &str) -> Self {
        self.text.push_str(suffix);
        if let Some(source) = &mut self.source {
            source.suffix.push_str(suffix);
        }
        self
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn clear(&mut self) {
        self.source = None;
        self.text.clear();
    }
}

impl From<String> for UiText {
    fn from(text: String) -> Self {
        Self { text, source: None }
    }
}

impl From<&str> for UiText {
    fn from(text: &str) -> Self {
        text.to_owned().into()
    }
}

impl Deref for UiText {
    type Target = str;

    fn deref(&self) -> &str {
        &self.text
    }
}

impl fmt::Display for UiText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self)
    }
}

impl<T: AsRef<str>> PartialEq<T> for UiText {
    fn eq(&self, other: &T) -> bool {
        self.text == other.as_ref()
    }
}

impl AsRef<str> for UiText {
    fn as_ref(&self) -> &str {
        &self.text
    }
}

/// AppKit receives only the resolved presentation; button indices are kept in
/// the caller's existing command order.
pub(crate) struct Dialog {
    pub title: UiText,
    pub detail: UiText,
    pub buttons: Vec<UiText>,
}

#[derive(Debug)]
pub(crate) struct IndexLabels {
    pub title: Cow<'static, str>,
    pub heading: Cow<'static, str>,
    pub preparing: Cow<'static, str>,
    pub inactive: Cow<'static, str>,
    pub progress: Cow<'static, str>,
}

impl IndexLabels {
    pub fn new(tr: &Translator) -> Self {
        Self {
            title: tr.tr("Indexing Status — Velocity"),
            heading: tr.tr("Indexing Status"),
            preparing: tr.tr("Preparing project index…"),
            inactive: tr.tr("No project index. Open a folder and enable index_files."),
            progress: tr.tr(
                "{percent}%  {files} files, {cached} cached, {pending} pending, {skipped} skipped",
            ),
        }
    }
}

impl Default for IndexLabels {
    fn default() -> Self {
        Self::new(&Translator::default())
    }
}

impl Dialog {
    pub fn new(
        title: UiText,
        detail: impl Into<UiText>,
        buttons: impl IntoIterator<Item = UiText>,
    ) -> Self {
        Self {
            title,
            detail: detail.into(),
            buttons: buttons.into_iter().collect(),
        }
    }

    pub fn localize(&mut self, tr: &Translator) {
        self.title.refresh(tr);
        self.detail.refresh(tr);
        for button in &mut self.buttons {
            button.refresh(tr);
        }
    }

    pub fn show(mut self, tr: &Translator, window: &vel_platform::WindowHandle) -> usize {
        self.localize(tr);
        let buttons: Vec<&str> = self.buttons.iter().map(UiText::as_ref).collect();
        window.alert(&self.title, &self.detail, &buttons)
    }
}

/// Labels reused by the painted chrome. Rebuilt only when the active catalog
/// or language changes, including a package/user catalog reload.
#[derive(Debug)]
pub(crate) struct Chrome {
    pub revision: u64,
    pub find_fields: [Cow<'static, str>; 3],
    pub find_buttons: [Cow<'static, str>; 5],
    pub find_options: [Cow<'static, str>; 6],
    pub close: Cow<'static, str>,
    pub where_menu: Cow<'static, str>,
    pub open_files: Cow<'static, str>,
    pub folders: Cow<'static, str>,
    pub no_folders: Cow<'static, str>,
    pub add_folder_hint: Cow<'static, str>,
    pub find_results: Cow<'static, str>,
    pub console: Cow<'static, str>,
    pub output: Cow<'static, str>,
    pub no_matches: Cow<'static, str>,
    pub palette_footer: Cow<'static, str>,
    pub goto_footer: Cow<'static, str>,
    pub choice_footer: Cow<'static, str>,
    pub appearance_footer: Cow<'static, str>,
    pub jump_footer: Cow<'static, str>,
    pub indexing: Cow<'static, str>,
    pub large_file_hints: [Cow<'static, str>; 6],
    pub vintage_modes: [Cow<'static, str>; 7],
    pub recording: Cow<'static, str>,
    pub hex: Cow<'static, str>,
    pub loading: Cow<'static, str>,
    pub preparing: Cow<'static, str>,
    pub compact_position: Cow<'static, str>,
    pub full_position: Cow<'static, str>,
    pub character: Cow<'static, str>,
    pub characters: Cow<'static, str>,
    pub regions: Cow<'static, str>,
    pub indent_spaces: Cow<'static, str>,
    pub indent_tabs: Cow<'static, str>,
    pub untitled: Cow<'static, str>,
    pub deleted: Cow<'static, str>,
    pub new_file: Cow<'static, str>,
    pub hex_tab: Cow<'static, str>,
    pub read_only: Cow<'static, str>,
    pub default_settings: Cow<'static, str>,
    pub user_settings: Cow<'static, str>,
    pub settings_window: Cow<'static, str>,
    pub project_title: Cow<'static, str>,
    pub close_find: Cow<'static, str>,
    pub palette_prompt: Cow<'static, str>,
    pub goto_prompt: Cow<'static, str>,
    pub line_prompt: Cow<'static, str>,
    pub symbol_prompt: Cow<'static, str>,
    pub choice_prompt: Cow<'static, str>,
    pub syntax_prompt: Cow<'static, str>,
    pub appearance_prompt: Cow<'static, str>,
    pub project_symbol_prompt: Cow<'static, str>,
    pub location_prompt: Cow<'static, str>,
}

impl Chrome {
    pub fn new(tr: &Translator) -> Self {
        Self {
            revision: tr.revision(),
            find_fields: [tr.tr("Find"), tr.tr("Replace"), tr.tr("Where")],
            find_buttons: [
                tr.tr("Find"),
                tr.tr("Find Prev"),
                tr.tr("Find All"),
                tr.tr("Replace"),
                tr.tr("Replace All"),
            ],
            find_options: [
                tr.tr("Regular expression"),
                tr.tr("Match case"),
                tr.tr("Whole word"),
                tr.tr("Wrap around"),
                tr.tr("In selection"),
                tr.tr("Preserve case"),
            ],
            close: tr.tr("Close"),
            where_menu: tr.tr("Where…"),
            open_files: tr.tr("Open Files"),
            folders: tr.tr("Folders"),
            no_folders: tr.tr("No folders open"),
            add_folder_hint: tr.tr("Project › Add Folder…"),
            find_results: tr.tr("Find Results"),
            console: tr.tr("Console"),
            output: tr.tr("Output"),
            no_matches: tr.tr("No matches"),
            palette_footer: tr.tr("↩ Run · ⎋ Cancel"),
            goto_footer: tr.tr("↩ Open · ⌘↩ / ⌃↩ Open in Split · ⎋ Cancel"),
            choice_footer: tr.tr("↩ Select · ⎋ Cancel"),
            appearance_footer: tr.tr("↑↓ Preview · ↩ Save · ⎋ Cancel"),
            jump_footer: tr.tr("↩ Jump · ⎋ Cancel"),
            indexing: tr.tr("Indexing {percent}%"),
            large_file_hints: [
                tr.tr("viewport syntax"),
                tr.tr("indexing off"),
                tr.tr("wrap off"),
                tr.tr("spelling off"),
                tr.tr("disk diff off"),
                tr.tr("sampled minimap"),
            ],
            vintage_modes: [
                tr.tr("INSERT"),
                tr.tr("COMMAND"),
                tr.tr("REPLACE"),
                tr.tr("VISUAL"),
                tr.tr("VISUAL LINE"),
                tr.tr("VISUAL BLOCK"),
                tr.tr("PROMPT"),
            ],
            recording: tr.tr("recording"),
            hex: tr.tr("Read-only hex · Offset 0x{offset} / {bytes} bytes"),
            loading: tr.tr("Loading {percent}% · Read-only prefix"),
            preparing: tr.tr("Preparing line geometry"),
            compact_position: tr.tr("Ln {line}, Col {column}"),
            full_position: tr.tr("Line {line}, Column {column}"),
            character: tr.tr("{count} character selected"),
            characters: tr.tr("{count} characters selected"),
            regions: tr.tr("{count} selection regions"),
            indent_spaces: tr.tr("Spaces: {width}"),
            indent_tabs: tr.tr("Tab Size: {width}"),
            untitled: tr.tr("Untitled"),
            deleted: tr.tr(" [deleted]"),
            new_file: tr.tr(" [new]"),
            hex_tab: tr.tr(" [hex]"),
            read_only: tr.tr(" (Read Only)"),
            default_settings: tr.tr("{name} (Default)"),
            user_settings: tr.tr("{name} (User)"),
            settings_window: tr.tr("Settings — Velocity"),
            project_title: tr.tr("{title} — {project}"),
            close_find: tr.tr("Close find panel"),
            palette_prompt: tr.tr("Search commands"),
            goto_prompt: tr.tr("Go to file, symbol, or line"),
            line_prompt: tr.tr("Enter a line number"),
            symbol_prompt: tr.tr("Search symbols"),
            choice_prompt: tr.tr("Filter choices"),
            syntax_prompt: tr.tr("Filter syntaxes"),
            appearance_prompt: tr.tr("Filter appearances"),
            project_symbol_prompt: tr.tr("Search project symbols"),
            location_prompt: tr.tr("Filter locations"),
        }
    }

    pub fn indent(&self, spaces: bool, width: usize) -> String {
        let template = if spaces {
            &self.indent_spaces
        } else {
            &self.indent_tabs
        };
        vel_i18n::format_named(template, &[("width", width.to_string())])
    }
}
