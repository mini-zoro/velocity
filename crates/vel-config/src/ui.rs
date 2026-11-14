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
        } else {
            target.push(item);
        }
    }
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
