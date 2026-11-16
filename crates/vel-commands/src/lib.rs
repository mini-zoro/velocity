//! Sublime-named commands over the shared multi-selection buffer model.
#![forbid(unsafe_code)]
mod text_ops;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use vel_buffer::{
    Buffer, Edit, EditKind, MonospaceLayout, Region, Selection, ViewId,
    ops::{self, DeleteBy, ExpandTo, MoveTo, MoveUnit, Stops},
};
use vel_config::settings::EditorSettings;

/// Execution domain, matching the public command API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Operates on the active document/view.
    TextCommand,
    /// Operates on the owning window.
    WindowCommand,
    /// Operates on the application.
    ApplicationCommand,
}
/// Editor adapter. Platform operations are injected so command tests stay headless.
pub trait Host {
    /// Active buffer and view identity.
    fn buffer(&mut self) -> (&mut Buffer, ViewId);
    /// Current view preferences.
    fn settings(&self) -> EditorSettings;
    /// Clipboard contents.
    fn clipboard(&mut self) -> String;
    /// Writes clipboard contents.
    fn set_clipboard(&mut self, text: &str);
    /// Records the regions the last copy or cut joined into the clipboard text, and whether they
    /// were whole lines copied from empty selections, so a paste can distribute them or insert
    /// them above the caret lines.
    fn set_clipboard_regions(&mut self, _parts: Vec<String>, _whole_lines: bool) {}
    /// Executes a window/application action, including scrolling.
    fn action(&mut self, name: &str, args: &Value) -> Result<(), String>;
    /// Allows the view to supply language-aware edits and visual-line movement.
    /// Return true after handling a command; the registry still records macros/repeat.
    fn intercept_text_command(&mut self, _name: &str, _args: &Value) -> Result<bool, String> {
        Ok(false)
    }
    /// Observes each command boundary, including nested chain/macro commands.
    fn command_finished(&mut self) {}
    /// Whether a host action is enabled.
    fn action_enabled(&self, _name: &str, _args: &Value) -> bool {
        true
    }
}
/// Extension command callback.
pub type Handler = fn(&mut dyn Host, &Value) -> Result<(), String>;
#[derive(Clone, Debug)]
struct Entry {
    kind: Kind,
    handler: Option<Handler>,
}
/// Registry, repeat state and optional macro recorder.
#[derive(Debug, Default)]
pub struct Registry {
    entries: BTreeMap<String, Entry>,
    recording: bool,
    macro_commands: Vec<(String, Value)>,
    last: Option<(String, Value)>,
}
const TEXT: &[&str] = &[
    "next_modification",
    "prev_modification",
    "toggle_inline_diff",
    "revert_hunk",
    "revert_modification",
    "next_misspelling",
    "prev_misspelling",
    "add_word",
    "ignore_word",
    "replace_misspelling",
    "auto_complete",
    "hide_auto_complete",
    "commit_completion",
    "replace_completion_with_next_completion",
    "next_field",
    "prev_field",
    "clear_fields",
    "toggle_comment",
    "reindent",
    "fold",
    "unfold",
    "fold_by_level",
    "unfold_all",
    "fold_tag_attributes",
    "insert",
    "paste",
    "copy",
    "cut",
    "left_delete",
    "right_delete",
    "delete_word",
    "move",
    "move_to",
    "select_all",
    "undo",
    "redo",
    "redo_or_repeat",
    "soft_undo",
    "soft_redo",
    "find_under_expand",
    "find_under_expand_skip",
    "find_all_under",
    "single_selection",
    "split_selection_into_lines",
    "select_lines",
    "expand_selection",
    "indent",
    "unindent",
    "insert_best_completion",
    "insert_snippet",
    "join_lines",
    "swap_line_up",
    "swap_line_down",
    "duplicate_line",
    "upper_case",
    "lower_case",
    "title_case",
    "swap_case",
    "transpose",
    "sort_lines",
    "reverse_lines",
    "permute_lines",
    "run_macro_file",
    "paste_and_indent",
    "scroll_lines",
    "show_at_center",
];
const WINDOW: &[&str] = &[
    "show_dictionary",
    "set_dictionary",
    "git_open_repository",
    "git_file_history",
    "git_folder_history",
    "git_line_history",
    "git_blame_file",
    "goto_symbol_in_project",
    "goto_definition",
    "goto_reference",
    "jump_back",
    "jump_forward",
    "show_indexing_status",
    "set_layout",
    "focus_group",
    "move_to_group",
    "focus_neighboring_group",
    "move_to_neighboring_group",
    "focus_next_control",
    "focus_previous_control",
    "new_pane",
    "close_pane",
    "clone_file",
    "select_all_tabs",
    "select_single_tab",
    "select_tab",
    "select_tab_to_left",
    "select_tab_to_right",
    "focus_selected_tab",
    "close_selected_tabs",
    "toggle_distraction_free",
    "toggle_full_screen",
    "open_project",
    "prompt_open_project",
    "save_project_as",
    "close_project",
    "edit_project",
    "quick_switch_project",
    "recent_projects",
    "build",
    "exec",
    "cancel_build",
    "show_build_systems",
    "set_build_system",
    "next_result",
    "prev_result",
    "set_file_type",
    "toggle_minimap",
    "show_overlay",
    "show_panel",
    "hide_panel",
    "show_input_panel",
    "show_output_panel",
    "toggle_side_bar",
    "focus_side_bar",
    "open_dir",
    "add_folder",
    "goto_line",
    "find_next",
    "find_prev",
    "find_all",
    "replace_next",
    "replace_all",
    "find_under",
    "find_under_prev",
    "slurp_find_string",
    "slurp_replace_string",
    "find_in_files",
    "replace_in_files",
    "close_split",
    "toggle_find_option",
    "new_file",
    "prompt_open_file",
    "open_file",
    "save",
    "prompt_save_as",
    "save_all",
    "close",
    "close_file",
    "next_view",
    "prev_view",
    "select_by_index",
    "increase_font_size",
    "decrease_font_size",
    "reset_font_size",
    "set_line_endings",
    "set_encoding",
    "toggle_setting",
];
impl Registry {
    /// Installs our implemented command set; no bundled Default code is used.
    pub fn standard() -> Self {
        let mut registry = Self::default();
        for name in TEXT {
            registry.register(name, Kind::TextCommand, None);
        }
        for name in WINDOW {
            registry.register(name, Kind::WindowCommand, None);
        }
        for name in [
            "chain",
            "new_window",
            "close_window",
            "toggle_record_macro",
            "run_macro",
            "install_cli",
            "open_crash_reports",
            "package_install_file",
            "package_install_url",
            "package_remove",
            "package_toggle",
            "package_list",
            "package_check_upgrades",
            "package_upgrade",
        ] {
            registry.register(name, Kind::ApplicationCommand, None);
        }
        text_ops::register(&mut registry);
        registry
    }
    /// Adds/replaces an extension command.
    pub fn register(&mut self, name: &str, kind: Kind, handler: Option<Handler>) {
        self.entries.insert(name.into(), Entry { kind, handler });
    }
    /// Execution domain of a registered command.
    pub fn kind(&self, name: &str) -> Option<Kind> {
        self.entries.get(name).map(|e| e.kind)
    }
    /// Whether a command is available in the current host.
    pub fn is_enabled(&self, name: &str, args: &Value, host: &dyn Host) -> bool {
        self.entries.contains_key(name) && host.action_enabled(name, args)
    }
    /// Whether commands are being recorded into the current macro.
    pub fn is_recording(&self) -> bool {
        self.recording
    }
    /// Executes a command with a recursion limit for chains/macros/repeat.
    pub fn run(&mut self, name: &str, args: &Value, host: &mut dyn Host) -> Result<(), String> {
        self.run_depth(name, args, host, 0)
    }
    fn run_depth(
        &mut self,
        name: &str,
        args: &Value,
        host: &mut dyn Host,
        depth: usize,
    ) -> Result<(), String> {
        if depth >= 32 {
            return Err("command recursion exceeds 32".into());
        }
        let entry = self
            .entries
            .get(name)
            .cloned()
            .ok_or_else(|| format!("unknown command {name}"))?;
        if !self.is_enabled(name, args, host) {
            return Ok(());
        }
        if !args.is_object() {
            return Err("command args must be an object".into());
        }
        match name {
            "chain" => {
                let commands = args["commands"].as_array().ok_or("chain needs commands")?;
                for command in commands {
                    let pair = command
                        .as_array()
                        .ok_or("chain entry must be [name, args]")?;
                    let name = pair
                        .first()
                        .and_then(Value::as_str)
                        .ok_or("chain command name")?;
                    self.run_depth(name, pair.get(1).unwrap_or(&json!({})), host, depth + 1)?;
                }
                return Ok(());
            }
            "toggle_record_macro" => {
                self.recording = !self.recording;
                if self.recording {
                    self.macro_commands.clear();
                }
                return Ok(());
            }
            "run_macro" => {
                if self.recording {
                    return Err("stop recording before replay".into());
                }
                let commands = if let Some(commands) = args.get("commands") {
                    commands
                        .as_array()
                        .ok_or("macro commands must be an array")?
                        .iter()
                        .map(|v| {
                            Ok((
                                v["command"]
                                    .as_str()
                                    .ok_or("macro command name")?
                                    .to_string(),
                                v.get("args").cloned().unwrap_or(json!({})),
                            ))
                        })
                        .collect::<Result<Vec<_>, String>>()?
                } else {
                    self.macro_commands.clone()
                };
                for (name, args) in commands {
                    self.run_depth(&name, &args, host, depth + 1)?;
                }
                return Ok(());
            }
            "run_macro_file" => {
                let file = args["file"].as_str().ok_or("run_macro_file needs a file")?;
                let name = file
                    .rsplit('/')
                    .next()
                    .and_then(|n| n.strip_suffix(".sublime-macro"))
                    .unwrap_or(file);
                if name == "Add Line Before" {
                    let (b, id) = host.buffer();
                    ops::add_line_before(b, id);
                    host.command_finished();
                    return Ok(());
                }
                let commands = builtin_macro(name)
                    .ok_or_else(|| format!("macro file {file} is not available"))?;
                for (name, args) in commands {
                    self.run_depth(name, &args, host, depth + 1)?;
                }
                return Ok(());
            }
            "redo_or_repeat" => {
                let (b, id) = host.buffer();
                if b.redo(id) {
                    host.command_finished();
                    return Ok(());
                }
                if let Some((name, args)) = self.last.clone() {
                    self.run_depth(&name, &args, host, depth + 1)?;
                }
                return Ok(());
            }
            _ => {}
        }
        if let Some(handler) = entry.handler {
            handler(host, args)?;
        } else if entry.kind != Kind::TextCommand
            || matches!(name, "scroll_lines" | "show_at_center")
        {
            host.action(name, args)?;
        } else {
            text_command(name, args, host)?;
        }
        host.command_finished();
        if self.recording {
            self.macro_commands.push((name.into(), args.clone()));
        }
        if entry.kind == Kind::TextCommand
            && !matches!(
                name,
                "undo"
                    | "redo"
                    | "soft_undo"
                    | "soft_redo"
                    | "copy"
                    | "move"
                    | "move_to"
                    | "select_all"
                    | "single_selection"
                    | "scroll_lines"
                    | "show_at_center"
            )
        {
            self.last = Some((name.into(), args.clone()));
        }
        Ok(())
    }
}
/// Line-editing macros that key bindings name by their Default package file name
/// (`res://Packages/Default/<name>.sublime-macro`).
fn builtin_macro(name: &str) -> Option<Vec<(&'static str, Value)>> {
    Some(match name {
        "Add Line" => vec![
            ("move_to", json!({"to": "hardeol"})),
            ("insert", json!({"characters": "\n"})),
        ],
        "Add Line in Braces" => vec![("insert", json!({"characters": "\n"}))],
        "Delete Line" => vec![
            ("expand_selection", json!({"to": "line"})),
            ("left_delete", json!({})),
        ],
        "Delete to Hard BOL" | "Delete to BOL" => vec![
            ("move_to", json!({"to": "hardbol", "extend": true})),
            ("left_delete", json!({})),
        ],
        "Delete to Hard EOL" | "Delete to EOL" => vec![
            ("move_to", json!({"to": "hardeol", "extend": true})),
            ("right_delete", json!({})),
        ],
        "Delete Left Right" => vec![("left_delete", json!({})), ("right_delete", json!({}))],
        _ => return None,
    })
}
fn boolean(args: &Value, key: &str) -> bool {
    args[key].as_bool().unwrap_or(false)
}
fn string<'a>(args: &'a Value, key: &str, default: &'a str) -> &'a str {
    args[key].as_str().unwrap_or(default)
}
/// The editor settings buffer operations use.
pub fn buffer_settings(settings: &EditorSettings) -> vel_buffer::Settings {
    vel_buffer::Settings {
        tab_size: settings.tab_size,
        translate_tabs_to_spaces: settings.translate_tabs_to_spaces,
        word_separators: settings.word_separators.clone(),
        use_tab_stops: settings.use_tab_stops,
    }
}
/// Mirrors [`buffer_settings`] into the view when they changed; returns whether they did.
pub fn sync_buffer_settings(b: &mut Buffer, id: ViewId, settings: &EditorSettings) -> bool {
    let wanted = buffer_settings(settings);
    let changed = b.view(id).settings != wanted;
    if changed {
        b.set_settings(id, wanted);
    }
    changed
}
fn text_command(name: &str, args: &Value, host: &mut dyn Host) -> Result<(), String> {
    if host.intercept_text_command(name, args)? {
        return Ok(());
    }
    if matches!(name, "copy" | "cut") {
        let (buffer, id) = host.buffer();
        let mut selection = buffer.view(id).sel().clone();
        let whole_lines = selection.iter().all(Region::empty);
        if whole_lines && !host.settings().copy_with_empty_selection {
            return Ok(());
        }
        let (buffer, id) = host.buffer();
        if whole_lines {
            let regions = selection.iter().map(|r| {
                let row = buffer.text().byte_to_line(r.b);
                Region::new(
                    buffer.text().line_to_byte(row),
                    if row + 1 < buffer.text().len_lines() {
                        buffer.text().line_to_byte(row + 1)
                    } else {
                        buffer.text().len_bytes()
                    },
                )
            });
            selection = Selection::from_regions(regions);
        }
        let parts = selection
            .iter()
            .map(|r| buffer.text().slice(r.to_range()).to_string())
            .collect::<Vec<_>>();
        let text = parts.join(if parts.iter().all(|p| p.ends_with('\n')) {
            ""
        } else {
            "\n"
        });
        if name == "cut" {
            buffer.set_selection(id, selection);
            ops::insert(buffer, id, "");
        }
        host.set_clipboard(&text);
        host.set_clipboard_regions(parts, whole_lines);
        return Ok(());
    }
    let clipboard = if name == "paste" {
        Some(host.clipboard())
    } else {
        None
    };
    let settings = host.settings();
    let (b, id) = host.buffer();
    sync_buffer_settings(b, id, &settings);
    let layout = MonospaceLayout {
        tab_size: settings.tab_size,
        page_lines: args["page_lines"].as_u64().unwrap_or(30) as usize,
    };
    let forward = boolean(args, "forward");
    let extend = boolean(args, "extend");
    match name {
        "insert" => {
            let text = args["characters"]
                .as_str()
                .ok_or("insert needs characters")?;
            if text == "\n" {
                let replacements = b
                    .view(id)
                    .sel()
                    .iter()
                    .map(|r| {
                        let row = b.text().byte_to_line(r.begin());
                        let line = b.text().line(row).to_string();
                        let indent = line
                            .chars()
                            .take_while(|c| matches!(c, ' ' | '\t'))
                            .collect::<String>();
                        format!("\n{indent}")
                    })
                    .collect();
                replace(b, id, replacements, EditKind::InsertChars, None);
            } else {
                ops::insert(b, id, text);
            }
        }
        "paste" => {
            ops::paste(b, id, clipboard.as_deref().unwrap_or(""));
        }
        "left_delete" => {
            ops::delete_left(b, id, DeleteBy::Char);
        }
        "right_delete" => {
            ops::delete_right(b, id, DeleteBy::Char);
        }
        "delete_word" => {
            if boolean(args, "sub_words") {
                ops::move_by(b, id, MoveUnit::SubwordEnds, forward, true, None);
                ops::delete_left(b, id, DeleteBy::Char);
            } else {
                ops::delete_word(b, id, forward);
            }
        }
        "move" => {
            let unit = match string(args, "by", "characters") {
                "characters" => MoveUnit::Characters,
                "words" => MoveUnit::Words,
                "word_ends" => MoveUnit::WordEnds,
                "subwords" => MoveUnit::Subwords,
                "subword_ends" => MoveUnit::SubwordEnds,
                "lines" => MoveUnit::Lines,
                "pages" => MoveUnit::Pages,
                "stops" => MoveUnit::Stops(Stops {
                    word_start: boolean(args, "word_begin"),
                    word_end: boolean(args, "word_end"),
                    punctuation: boolean(args, "punct_begin") || boolean(args, "punct_end"),
                    newline: boolean(args, "empty_line"),
                    subword: boolean(args, "subword_begin"),
                }),
                other => return Err(format!("unknown move unit {other}")),
            };
            ops::move_by(b, id, unit, forward, extend, Some(&layout));
        }
        "move_to" => {
            let to = match string(args, "to", "bol") {
                "bol" => MoveTo::Bol,
                "eol" => MoveTo::Eol,
                "hardbol" => MoveTo::HardBol,
                "hardeol" => MoveTo::HardEol,
                "bof" => MoveTo::Bof,
                "eof" => MoveTo::Eof,
                other => return Err(format!("unknown destination {other}")),
            };
            ops::move_to(b, id, to, extend);
        }
        "select_all" => {
            let len = b.text().len_bytes();
            b.set_selection(id, Selection::from_regions([Region::new(0, len)]));
        }
        "undo" => {
            b.undo(id);
        }
        "redo" => {
            b.redo(id);
        }
        "soft_undo" => {
            b.soft_undo(id);
        }
        "soft_redo" => {
            b.soft_redo(id);
        }
        "find_under_expand" => {
            ops::find_under_expand(b, id);
        }
        "find_under_expand_skip" => {
            let old = b.view(id).sel().primary().expect("selection");
            if ops::find_under_expand(b, id) {
                let selection =
                    Selection::from_regions(b.view(id).sel().iter().filter(|r| *r != old));
                b.set_selection(id, selection);
            }
        }
        "find_all_under" => ops::find_all_under(b, id),
        "single_selection" => ops::single_selection(b, id),
        "split_selection_into_lines" => ops::split_selection_into_lines(b, id),
        "select_lines" => ops::select_lines(b, id, forward, &layout),
        "expand_selection" => {
            let to = match string(args, "to", "word") {
                "word" => ExpandTo::Word,
                "line" => ExpandTo::Line,
                "indentation" => ExpandTo::Indentation,
                other => return Err(format!("unsupported expansion {other}")),
            };
            ops::expand_selection(b, id, to);
        }
        "indent" => {
            ops::indent(b, id);
        }
        "unindent" => {
            ops::unindent(b, id);
        }
        "insert_best_completion" => {
            let texts = b
                .view(id)
                .sel()
                .iter()
                .map(|r| {
                    if settings.translate_tabs_to_spaces && !settings.use_tab_stops {
                        " ".repeat(settings.tab_size)
                    } else if settings.translate_tabs_to_spaces {
                        use vel_buffer::VerticalLayout;
                        let col = layout.x_of(b.text(), r.begin()) as usize;
                        " ".repeat(settings.tab_size - col % settings.tab_size)
                    } else {
                        "\t".into()
                    }
                })
                .collect();
            replace(b, id, texts, EditKind::Other, None);
        }
        "insert_snippet" => {
            let snippet =
                Snippet::parse(args["contents"].as_str().ok_or("snippet needs contents")?)?;
            let texts = vec![snippet.text.clone(); b.view(id).sel().len()];
            replace(b, id, texts, EditKind::Other, Some(snippet.selection));
        }
        "join_lines" => {
            ops::join_lines(b, id);
        }
        "swap_line_up" => {
            ops::swap_line_up(b, id);
        }
        "swap_line_down" => {
            ops::swap_line_down(b, id);
        }
        "duplicate_line" => {
            ops::duplicate_line(b, id);
        }
        "upper_case" => {
            ops::upper_case(b, id);
        }
        "lower_case" => {
            ops::lower_case(b, id);
        }
        "title_case" => {
            ops::title_case(b, id);
        }
        "swap_case" => {
            ops::swap_case(b, id);
        }
        "transpose" => {
            ops::transpose(b, id);
        }
        "sort_lines" if !args["case_sensitive"].as_bool().unwrap_or(true) => {
            ops::sort_lines_folded(b, id);
        }
        "sort_lines" => {
            ops::sort_lines(b, id);
        }
        "reverse_lines" => {
            ops::reverse_lines(b, id);
        }
        "permute_lines" => match string(args, "operation", "reverse") {
            "reverse" => {
                ops::reverse_lines(b, id);
            }
            "sort" => {
                ops::sort_lines(b, id);
            }
            other => return Err(format!("unsupported line permutation {other}")),
        },
        _ => return Err(format!("unimplemented text command {name}")),
    }
    Ok(())
}
/// Commits one replacement per selection with optional relative snippet field.
fn replace(
    b: &mut Buffer,
    id: ViewId,
    texts: Vec<String>,
    kind: EditKind,
    field: Option<std::ops::Range<usize>>,
) {
    let selection = b.view(id).sel().clone();
    let mut delta = 0isize;
    let mut regions = Vec::new();
    let edits: Vec<_> = selection
        .iter()
        .zip(&texts)
        .map(|(r, text)| {
            let start = r.begin().wrapping_add_signed(delta);
            let range = field.clone().unwrap_or(text.len()..text.len());
            regions.push(Region::new(start + range.start, start + range.end));
            delta += text.len() as isize - r.size() as isize;
            Edit {
                range: r.to_range(),
                text,
            }
        })
        .collect();
    b.transact(
        id,
        kind,
        &edits,
        selection_with_primary(regions, selection.primary_index()),
    );
}
/// Minimal snippet expansion; fields navigation is intentionally deferred.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snippet {
    /// Expanded text.
    pub text: String,
    /// Initial field, or final `$0` caret, in expanded UTF-8 bytes.
    pub selection: std::ops::Range<usize>,
}
impl Snippet {
    /// Expands `$0`, `$1`… and `${1:default}`, with escaped dollars/backslashes.
    pub fn parse(source: &str) -> Result<Self, String> {
        let mut text = String::new();
        let mut fields = BTreeMap::new();
        let mut defaults = BTreeMap::<usize, String>::new();
        let mut at = 0;
        while at < source.len() {
            let c = source[at..].chars().next().expect("char");
            if c == '\\' {
                at += 1;
                if let Some(c) = source[at..].chars().next() {
                    text.push(c);
                    at += c.len_utf8();
                }
                continue;
            }
            if c != '$' {
                text.push(c);
                at += c.len_utf8();
                continue;
            }
            let start = at;
            at += 1;
            let braced = source.as_bytes().get(at) == Some(&b'{');
            if braced {
                at += 1;
            }
            let digits = at;
            while source.as_bytes().get(at).is_some_and(u8::is_ascii_digit) {
                at += 1;
            }
            if at == digits {
                text.push('$');
                at = start + 1;
                continue;
            }
            let number: usize = source[digits..at]
                .parse()
                .map_err(|_| "snippet field overflow")?;
            let value = if braced && source.as_bytes().get(at) == Some(&b':') {
                at += 1;
                let end = source[at..]
                    .find('}')
                    .ok_or("unclosed snippet placeholder")?
                    + at;
                let value = source[at..end].to_string();
                at = end;
                defaults.insert(number, value.clone());
                value
            } else {
                defaults.get(&number).cloned().unwrap_or_default()
            };
            if braced {
                if source.as_bytes().get(at) != Some(&b'}') {
                    return Err("unclosed snippet field".into());
                }
                at += 1;
            }
            let begin = text.len();
            text.push_str(&value);
            fields.entry(number).or_insert(begin..text.len());
        }
        let selection = fields
            .iter()
            .find(|(n, _)| **n > 0)
            .or_else(|| fields.get_key_value(&0))
            .map(|(_, r)| r.clone())
            .unwrap_or(text.len()..text.len());
        Ok(Self { text, selection })
    }
}

fn selection_with_primary(mut regions: Vec<Region>, primary: usize) -> Selection {
    if primary < regions.len() {
        let region = regions.remove(primary);
        regions.push(region);
    }
    Selection::from_regions(regions)
}
