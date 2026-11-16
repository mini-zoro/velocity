//! Declarative menu, mouse and palette formats (no native UI dependencies).
use crate::{
    Value,
    json::Document,
    keymap::{Context, Invocation, empty_args},
};
use serde::{Deserialize, de::DeserializeOwned};
/// Native menu resource entry.
#[derive(Clone, Debug, Deserialize)]
pub struct MenuItem {
    /// Visible text; a dash denotes a separator.
    #[serde(default)]
    pub caption: String,
    /// Stable ID for extending a submenu.
    #[serde(default)]
    pub id: String,
    /// Optional command.
    pub command: Option<String>,
    /// JSON arguments.
    #[serde(default = "empty_args")]
    pub args: Value,
    /// Nested submenu entries.
    #[serde(default)]
    pub children: Vec<MenuItem>,
    /// Platform filter, including `!OSX`.
    pub platform: Option<String>,
    /// Mnemonic metadata for non-Mac ports.
    pub mnemonic: Option<String>,
}
/// Palette command entry, consumed by vel-ui's quick panel.
#[derive(Clone, Debug, Deserialize)]
pub struct PaletteItem {
    /// Display caption.
    pub caption: String,
    /// Command/arguments.
    #[serde(flatten)]
    pub invocation: Invocation,
}
/// Adds menu-derived commands, keeping explicit palette captions/arguments first.
pub fn extend_palette(palette: &mut Vec<PaletteItem>, menu: &[MenuItem]) {
    fn visit(palette: &mut Vec<PaletteItem>, menu: &[MenuItem], prefix: &str) {
        for item in menu {
            if item.caption == "-"
                || matches!(item.platform.as_deref(), Some("!OSX" | "Windows" | "Linux"))
            {
                continue;
            }
            let caption = if prefix.is_empty() {
                item.caption.clone()
            } else {
                format!("{prefix}: {}", item.caption)
            };
            if let Some(command) = &item.command {
                let invocation = Invocation {
                    command: command.clone(),
                    args: item.args.clone(),
                };
                if !palette.iter().any(|p| p.invocation == invocation) {
                    palette.push(PaletteItem {
                        caption: caption.clone(),
                        invocation,
                    });
                }
            }
            visit(palette, &item.children, &caption);
        }
    }
    visit(palette, menu, "");
}
/// Mouse binding; button numbering follows Sublime (1 = primary).
#[derive(Clone, Debug, Deserialize)]
pub struct MouseBinding {
    /// Primary=1, secondary=2, middle=3.
    pub button: usize,
    /// Modifier names.
    #[serde(default)]
    pub modifiers: Vec<String>,
    /// Click count.
    #[serde(default = "one")]
    pub count: usize,
    /// Drag action or release action.
    pub command: Option<String>,
    /// Optional action on press.
    pub press_command: Option<String>,
    /// Arguments to command.
    #[serde(default = "empty_args")]
    pub args: Value,
    /// Arguments to press command.
    #[serde(default = "empty_args")]
    pub press_args: Value,
    /// Optional context conditions.
    #[serde(default)]
    pub context: Vec<Context>,
}
fn one() -> usize {
    1
}
/// Parses a resource using the shared lenient syntax.
pub fn parse<T: DeserializeOwned>(source: &str) -> Result<Vec<T>, String> {
    serde_json::from_value(Document::parse(source)?.value).map_err(|e| e.to_string())
}
/// Merges submenu IDs across later resources while preserving their order.
pub fn merge_menu(target: &mut Vec<MenuItem>, source: Vec<MenuItem>) {
    for mut item in source {
        if !item.id.is_empty()
            && let Some(existing) = target.iter_mut().find(|i| i.id == item.id)
        {
            merge_menu(&mut existing.children, std::mem::take(&mut item.children));
            if !item.caption.is_empty() {
                existing.caption = item.caption;
            }
            if item.command.is_some() {
                existing.command = item.command;
                existing.args = item.args;
            } else if item.args != empty_args() {
                existing.args = item.args;
            }
            if item.platform.is_some() {
                existing.platform = item.platform;
            }
            if item.mnemonic.is_some() {
                existing.mnemonic = item.mnemonic;
            }
        } else {
            target.push(item);
        }
    }
}

/// Loads one menu filename through the effective Default/package/User overlay.
///
/// Loose resources already replace archives at the same resource path; entries with
/// stable IDs then merge across enabled packages in deterministic package order.
/// A malformed layer returns its resource path and never publishes a partial menu.
pub fn load_menu(
    resources: &crate::resources::Resources,
    filename: &str,
) -> Result<Vec<MenuItem>, String> {
    let mut items = Vec::new();
    for resource in resources.matching(&format!("/{filename}")) {
        let layer = parse(resource.text()?).map_err(|e| format!("{}: {e}", resource.path))?;
        merge_menu(&mut items, layer);
    }
    Ok(items)
}

/// Current command state, evaluated with the invocation's complete context args.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandState {
    /// Whether selecting the command is allowed.
    pub enabled: bool,
    /// Whether the row is included in the menu.
    pub visible: bool,
    /// Whether the native row displays a checkmark.
    pub checked: bool,
}
impl Default for CommandState {
    fn default() -> Self {
        Self {
            enabled: true,
            visible: true,
            checked: false,
        }
    }
}

/// Platform-independent menu row ready for conversion to a native menu.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedMenuItem {
    /// Display text, or `-` for a separator.
    pub caption: String,
    /// Resource identity, retained for dynamic menu extensions.
    pub id: String,
    /// Command and effective resource/context args, absent for submenus/separators.
    pub invocation: Option<Invocation>,
    /// Visible, recursively evaluated submenu rows.
    pub children: Vec<ResolvedMenuItem>,
    /// Whether the row can be selected.
    pub enabled: bool,
    /// Whether the native row displays a checkmark.
    pub checked: bool,
}

/// Adds context keywords to resource args.
///
/// Internal `_`-prefixed keys always come from the click. Public keys (`paths`, `group`,
/// `index`, `event`, …) fill in only what the resource leaves open: an absent key or the
/// placeholders `null`, `-1` and `[]` (menu files write `{"group": -1, "index": -1}` or
/// `{"paths": []}` to ask for the clicked location). A concrete resource value, such as
/// `{"group": 1}` for `move_to_group`, is kept. Non-object resource/context values are
/// treated as empty keyword dictionaries.
pub fn context_invocation(invocation: &Invocation, context: &Value) -> Invocation {
    let mut args = invocation.args.as_object().cloned().unwrap_or_default();
    if let Some(context) = context.as_object() {
        for (key, value) in context {
            merge_context_arg(&mut args, key, value);
        }
    }
    Invocation {
        command: invocation.command.clone(),
        args: Value::Object(args),
    }
}

/// Inserts one click-context keyword following the rules of [`context_invocation`].
pub fn merge_context_arg(args: &mut serde_json::Map<String, Value>, key: &str, value: &Value) {
    let open = |v: &Value| {
        v.is_null() || v.as_i64() == Some(-1) || v.as_array().is_some_and(Vec::is_empty)
    };
    if key.starts_with('_') || args.get(key).is_none_or(open) {
        args.insert(key.into(), value.clone());
    }
}

/// Evaluates command state, platform filters and nested menus without a UI host.
///
/// Hidden commands are omitted, empty submenus disappear, and redundant separators
/// are removed. The state callback sees exactly the invocation later dispatched.
pub fn resolve_menu(
    items: &[MenuItem],
    platform: &str,
    context: &Value,
    mut command_state: impl FnMut(&Invocation) -> CommandState,
) -> Vec<ResolvedMenuItem> {
    fn visit(
        items: &[MenuItem],
        platform: &str,
        context: &Value,
        command_state: &mut impl FnMut(&Invocation) -> CommandState,
    ) -> Vec<ResolvedMenuItem> {
        let mut rows: Vec<ResolvedMenuItem> = Vec::new();
        for item in items {
            if let Some(filter) = item.platform.as_deref() {
                let matches = filter
                    .strip_prefix('!')
                    .map_or(filter == platform, |excluded| excluded != platform);
                if !matches {
                    continue;
                }
            }
            if item.caption == "-" {
                if rows.last().is_some_and(|row| row.caption != "-") {
                    rows.push(ResolvedMenuItem {
                        caption: "-".into(),
                        id: item.id.clone(),
                        invocation: None,
                        children: Vec::new(),
                        enabled: false,
                        checked: false,
                    });
                }
                continue;
            }
            let invocation = item.command.as_ref().map(|command| {
                context_invocation(
                    &Invocation {
                        command: command.clone(),
                        args: item.args.clone(),
                    },
                    context,
                )
            });
            let state = invocation
                .as_ref()
                .map_or_else(CommandState::default, &mut *command_state);
            if !state.visible {
                continue;
            }
            let children = visit(&item.children, platform, context, command_state);
            if invocation.is_none() && children.is_empty() {
                continue;
            }
            rows.push(ResolvedMenuItem {
                caption: item.caption.clone(),
                id: item.id.clone(),
                enabled: state.enabled
                    && (invocation.is_some() || children.iter().any(|child| child.enabled)),
                checked: state.checked,
                invocation,
                children,
            });
        }
        if rows.last().is_some_and(|row| row.caption == "-") {
            rows.pop();
        }
        rows
    }
    visit(items, platform, context, &mut command_state)
}

/// Parses mouse bindings and compiles optional context regular expressions.
pub fn parse_mousemap(source: &str) -> Result<Vec<MouseBinding>, String> {
    let mut bindings: Vec<MouseBinding> = parse(source)?;
    for binding in &mut bindings {
        for context in &mut binding.context {
            context.prepare()?;
        }
    }
    Ok(bindings)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn palette_menu_deduplication_platform_filter_and_binding_hints() {
        let mut palette: Vec<PaletteItem> =
            parse(r#"[{"caption":"Find","command":"show_panel","args":{"panel":"find"}}]"#)
                .unwrap();
        let menus: Vec<MenuItem> = parse(r#"[{"caption":"Edit","children":[{"caption":"Find","command":"show_panel","args":{"panel":"find"}},{"caption":"-","command":"noop"},{"caption":"Save","command":"save"},{"caption":"Linux","platform":"Linux","command":"new_window"}]}]"#).unwrap();
        extend_palette(&mut palette, &menus);
        assert_eq!(palette.len(), 2);
        assert_eq!(palette[1].caption, "Edit: Save");
        let mut keys = crate::keymap::Keymap::default();
        keys.append(r#"[{"keys":["super+f"],"command":"show_panel","args":{"panel":"find"}}]"#)
            .unwrap();
        assert_eq!(keys.hint(&palette[0].invocation).as_deref(), Some("Cmd+F"));
        keys.append(r#"[{"keys":["super+f"],"command":"save"}]"#)
            .unwrap();
        assert!(keys.hint(&palette[0].invocation).is_none());
    }
}
