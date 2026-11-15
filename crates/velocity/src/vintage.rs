//! Editor-owned vi routing, transaction publication and asynchronous host completions.
use crate::editor::Editor;
use vel_buffer::{BufferId, ChangeId};
use vel_config::{
    Value, json,
    keymap::{Invocation, Key, Resolution},
};
use vel_vintage::{Mode, Outcome, Settings, Vintage};

/// One vi engine per view; settings changes do not alter document preferences.
#[derive(Debug)]
pub struct State {
    pub engine: Vintage,
    ctrl: bool,
    clipboard: bool,
    pending: String,
    host: Option<(BufferId, ChangeId)>,
}

impl State {
    /// Discards a stale asynchronous host result.
    pub fn cancel_host(&mut self) {
        self.host = None;
    }
}

impl Editor {
    /// Updates the optional engine only when configuration changes.
    pub fn configure_vintage(&mut self) {
        if !self.settings.bool("vintage_enabled", false) {
            self.vintage = None;
            return;
        }
        let ctrl = self.settings.bool("vintage_ctrl_keys", false);
        let clipboard = self.settings.bool("vintage_use_clipboard", false);
        if self
            .vintage
            .as_ref()
            .is_some_and(|v| v.ctrl == ctrl && v.clipboard == clipboard)
        {
            return;
        }
        let normal = self.vintage.as_ref().map_or_else(
            || self.settings.bool("vintage_start_in_command_mode", false),
            |v| v.engine.mode() != Mode::Insert,
        );
        self.cancel_composition();
        self.vintage = Some(State {
            engine: Vintage::new(Settings {
                vintage_start_in_command_mode: normal,
                vintage_ctrl_keys: ctrl,
                vintage_use_clipboard: clipboard,
            }),
            ctrl,
            clipboard,
            pending: String::new(),
            host: None,
        });
        self.keymap.cancel();
        self.buffer.break_undo_group();
    }
    /// Whether document IME input is bypassed and the caret should be a block.
    pub fn command_mode(&self) -> bool {
        self.vintage
            .as_ref()
            .is_some_and(|v| v.engine.mode() != Mode::Insert)
    }
    /// Mode, count/operator prefix and macro recording indicator for the status bar.
    pub fn vintage_status(&self) -> String {
        let Some(vi) = &self.vintage else {
            return String::new();
        };
        let mode = match vi.engine.mode() {
            Mode::Insert => "INSERT",
            Mode::Normal => "COMMAND",
            Mode::Replace => "REPLACE",
            Mode::VisualChar => "VISUAL",
            Mode::VisualLine => "VISUAL LINE",
            Mode::VisualBlock => "VISUAL BLOCK",
            Mode::CommandLine => "PROMPT",
        };
        format!(
            "{mode} {}{}",
            vi.pending,
            if vi.engine.context("vi_recording_macro") {
                " · recording"
            } else {
                ""
            }
        )
    }
    /// Routes decoded document keys before the ordinary keymap. Printable insert-mode
    /// keys wait for native committed text so input methods retain first refusal.
    pub fn vintage_key(&mut self, key: &Key) -> Option<Resolution> {
        let vi = self.vintage.as_ref()?;
        if self.read_only {
            return None;
        }
        if self.marked.is_some() {
            return None;
        }
        let decoded = decode(key)?;
        if vi.engine.mode() == Mode::Insert && matches!(decoded, vel_vintage::Key::Char(_)) {
            return None;
        }
        Some(self.feed_vintage(decoded, &display_key(key)))
    }
    /// Commits complete IME/paste text through the engine, retaining vi undo/dot history.
    pub fn vintage_text(&mut self, text: &str) -> Option<Invocation> {
        match self.feed_vintage(vel_vintage::Key::Text(text.into()), text) {
            Resolution::Command(command) => Some(command),
            _ => None,
        }
    }
    fn feed_vintage(&mut self, key: vel_vintage::Key, display: &str) -> Resolution {
        let Some(mut vi) = self.vintage.take() else {
            return Resolution::Unbound;
        };
        if vi.host.is_some() && self.reindent_job.is_some() {
            // New input supersedes an unfinished formatter; never swallow a user's key.
            self.reindent_job = None;
            vi.host = None;
            vi.engine
                .complete_host_command(&mut self.buffer, self.id, 0);
        }
        if vi.clipboard {
            vi.engine.set_clipboard(self.clipboard.clone());
        }
        let old = self.buffer.text().clone();
        let version = self.buffer.change_id();
        let outcome = vi
            .engine
            .handle_key(key, &mut self.buffer, self.id, &*self.layout.borrow());
        if !matches!(&outcome, Outcome::PassThrough) {
            if vi.engine.context("vi_has_count")
                || vi.engine.context("vi_has_action")
                || vi.engine.context("vi_has_motion")
            {
                vi.pending.push_str(display);
            } else {
                vi.pending.clear();
            }
        }
        if vi.engine.mode() != Mode::Insert {
            self.completion.hide();
            self.fields = None;
        }
        if self.buffer.change_id() != version {
            self.fields = None;
            if let Some(maps) = self.buffer.changes_since(version)
                && maps.len() == 1
            {
                self.publish_direct_change(old, version, maps[0].1.clone());
            }
            self.html.borrow_mut().character_event();
            self.completion.edited(None);
        }
        self.html.borrow_mut().selection_modified();
        let result = match outcome {
            Outcome::NeedsCommand(command) => {
                if command.name == "reindent" {
                    vi.host = Some((self.buffer.id(), self.buffer.change_id()));
                }
                Resolution::Command(Invocation {
                    command: command.name.into(),
                    args: Value::Object(
                        command
                            .args
                            .into_iter()
                            .map(|(k, v)| {
                                let value = if k == "force" {
                                    json!(v == "true")
                                } else {
                                    json!(v)
                                };
                                (k, value)
                            })
                            .collect(),
                    ),
                })
            }
            Outcome::Handled => Resolution::Pending,
            Outcome::PassThrough => Resolution::Unbound,
        };
        self.vintage = Some(vi);
        self.sync();
        result
    }
    /// Finishes reindent after its accepted asynchronous edit, including no-op results.
    pub fn complete_vintage_host(&mut self) {
        if let Some(vi) = &mut self.vintage
            && let Some((buffer, from)) = vi.host.take()
            && buffer == self.buffer.id()
        {
            let groups = usize::from(self.buffer.change_id() != from);
            vi.engine
                .complete_host_command(&mut self.buffer, self.id, groups);
            self.sync();
        }
    }
    /// Handles vi host operations through the application's ordinary command registry.
    pub fn vintage_command(&mut self, name: &str, args: &Value) -> Result<bool, String> {
        match name {
            "set_clipboard" => self.clipboard = args["text"].as_str().unwrap_or("").into(),
            "exit_insert_mode" if self.vintage.is_some() => {
                self.feed_vintage(vel_vintage::Key::Escape, "");
            }
            "vintage_submit_prompt" if self.vintage.is_some() => {
                let old_chars = self
                    .vintage
                    .as_ref()
                    .and_then(|vi| vi.engine.command_line())
                    .map_or(0, |s| s.chars().count().saturating_sub(1));
                for _ in 0..old_chars {
                    self.feed_vintage(vel_vintage::Key::Backspace, "");
                }
                if let Some(command) = self.vintage_text(args["text"].as_str().unwrap_or("")) {
                    self.vintage_requests.push(command);
                }
                if let Resolution::Command(command) = self.feed_vintage(vel_vintage::Key::Enter, "")
                {
                    self.vintage_requests.push(command);
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

fn display_key(key: &Key) -> String {
    character(key).map(|c| c.to_string()).unwrap_or_default()
}
fn character(key: &Key) -> Option<char> {
    if key.name == "space" {
        return Some(' ');
    }
    let mut chars = key.name.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    if key.modifiers & 4 == 0 {
        return Some(c);
    }
    const BASE: &str = "`1234567890-=[]\\;',./";
    const SHIFT: &str = "~!@#$%^&*()_+{}|:\"<>?";
    Some(
        BASE.chars()
            .position(|b| b == c)
            .and_then(|i| SHIFT.chars().nth(i))
            .unwrap_or_else(|| c.to_ascii_uppercase()),
    )
}
fn decode(key: &Key) -> Option<vel_vintage::Key> {
    use vel_vintage::Key as Vi;
    if key.modifiers & 10 != 0 {
        return None;
    }
    if key.modifiers & 1 != 0 {
        return character(key).map(|c| Vi::Ctrl(c.to_ascii_lowercase()));
    }
    Some(match key.name.as_str() {
        "escape" => Vi::Escape,
        "enter" => Vi::Enter,
        "backspace" => Vi::Backspace,
        "left" => Vi::Left,
        "right" => Vi::Right,
        "up" => Vi::Up,
        "down" => Vi::Down,
        _ => Vi::Char(character(key)?),
    })
}
