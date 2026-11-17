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
    /// Primary=1, secondary=2, middle=3; wheel directions use the constants below.
    #[serde(deserialize_with = "mouse_button")]
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
/// Wheel-up binding identity (outside the physical button range).
pub const SCROLL_UP: usize = 17;
/// Wheel-down binding identity (outside the physical button range).
pub const SCROLL_DOWN: usize = 18;
fn mouse_button<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<usize, D::Error> {
    let value = Value::deserialize(deserializer)?;
    let button = value
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .or_else(|| {
            let text = value.as_str()?;
            match text {
                "scroll_up" => Some(SCROLL_UP),
                "scroll_down" => Some(SCROLL_DOWN),
                _ => text
                    .strip_prefix("button")?
                    .parse()
                    .ok()
                    .filter(|n| (1..=16).contains(n)),
            }
        })
        .filter(|n| (1..=SCROLL_DOWN).contains(n));
    button.ok_or_else(|| {
        serde::de::Error::custom("expected button1..button16, scroll_up/down or a button number")
    })
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
    let (bindings, warnings) = parse_mousemap_with_warnings(source)?;
    for warning in warnings {
        eprintln!("mousemap: {warning}");
    }
    Ok(bindings)
}
/// Parses entries independently so one malformed user binding cannot hide its neighbors.
pub fn parse_mousemap_with_warnings(
    source: &str,
) -> Result<(Vec<MouseBinding>, Vec<String>), String> {
    let document = Document::parse(source)?;
    let array = document
        .value
        .as_array()
        .ok_or("mousemap must be an array")?;
    let mut bindings = Vec::new();
    let mut warnings = Vec::new();
    for (index, value) in array.iter().enumerate() {
        let result = serde_json::from_value::<MouseBinding>(value.clone())
            .map_err(|e| e.to_string())
            .and_then(|mut binding| {
                if binding.count == 0 {
                    return Err("count must be positive".into());
                }
                if binding
                    .modifiers
                    .iter()
                    .any(|m| !matches!(m.as_str(), "ctrl" | "alt" | "shift" | "super"))
                {
                    return Err("unknown modifier".into());
                }
                for context in &mut binding.context {
                    context.prepare()?;
                }
                Ok(binding)
            });
        match result {
            Ok(binding) => bindings.push(binding),
            Err(error) => warnings.push(format!("binding {index}: {error}")),
        }
    }
    Ok((bindings, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mousemap_buttons_and_malformed_neighbors_are_independent() {
        let mut entries = Vec::new();
        for number in 1..=16 {
            entries.push(serde_json::json!({"button":format!("button{number}"),"command":"noop"}));
            entries.push(serde_json::json!({"button":number,"command":"noop"}));
        }
        entries.extend([
            serde_json::json!({"button":"scroll_up","command":"noop"}),
            serde_json::json!({"button":"scroll_down","command":"noop"}),
        ]);
        for bad in [
            serde_json::json!({"button":"button0"}),
            serde_json::json!({"button":"button17"}),
            serde_json::json!({"button":0}),
            serde_json::json!({"button":false}),
            serde_json::json!({"button":1,"count":0}),
            serde_json::json!({"button":1,"modifiers":["unknown"]}),
            serde_json::json!({"button":1,"context":[{"key":"setting.x","operator":"regex_match","operand":"("}]}),
        ] {
            entries.insert(1, bad);
        }
        let (bindings, warnings) =
            parse_mousemap_with_warnings(&serde_json::to_string(&entries).unwrap()).unwrap();
        assert_eq!(bindings.len(), 34);
        assert_eq!(warnings.len(), 7);
        assert_eq!(bindings.last().unwrap().button, SCROLL_DOWN);
        assert!(parse_mousemap("[").is_err());
    }
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
