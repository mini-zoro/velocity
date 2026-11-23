//! Curated pages of the Settings window: the commonly changed options, grouped into titled
//! sections without raw keys. Parsed on first use, so nothing loads before the window opens.
use serde::Deserialize;
use std::{collections::BTreeMap, sync::LazyLock};
use vel_config::{Value, json::Document};

const LAYOUT: &str = include_str!("../resources/Packages/Default/settings-pages.json");

/// A `show_when`/`enabled_when` operand, as written in the JSON resources.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum Condition {
    Not { not: Value },
    Any(Vec<Value>),
    Equals(Value),
}

impl Condition {
    pub(super) fn matches(&self, value: &Value) -> bool {
        match self {
            Self::Not { not } => not != value,
            Self::Any(values) => values.contains(value),
            Self::Equals(expected) => expected == value,
        }
    }

    #[cfg(test)]
    pub(super) fn operands(&self) -> &[Value] {
        match self {
            Self::Not { not } => std::slice::from_ref(not),
            Self::Any(values) => values,
            Self::Equals(value) => std::slice::from_ref(value),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Style {
    Radio,
    /// A popup of the installed font families, listed when it first opens.
    Font,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Platform {
    Macos,
    Windows,
}

impl Platform {
    pub(super) fn current(self) -> bool {
        match self {
            Self::Macos => cfg!(target_os = "macos"),
            Self::Windows => cfg!(windows),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Action {
    Json,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Preview {
    Font,
    Indentation,
}

#[derive(Debug, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub(crate) enum Item {
    Setting {
        key: String,
        label: String,
        #[serde(default)]
        control: Option<Style>,
        #[serde(default)]
        choices: Option<Vec<Value>>,
        #[serde(default)]
        enabled_when: BTreeMap<String, Condition>,
        #[serde(default)]
        platform: Option<Platform>,
        #[serde(default)]
        indent: bool,
    },
    Button {
        button: String,
        #[serde(default)]
        command: Option<String>,
        #[serde(default)]
        args: Value,
        #[serde(default)]
        action: Option<Action>,
    },
    Preview {
        preview: Preview,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Section {
    pub(super) title: String,
    pub(super) items: Vec<Item>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Spec {
    pub(crate) id: String,
    pub(super) name: String,
    pub(crate) icon: String,
    pub(crate) sections: Vec<Section>,
}

/// A curated key, its popup choices, and the operands of its `enabled_when`.
#[cfg(test)]
pub(crate) type CuratedSetting<'a> = (&'a str, &'a [Value], Vec<(String, &'a [Value])>);

#[cfg(test)]
impl Spec {
    /// Each curated key with its popup choices and the operands of its `enabled_when`.
    pub(crate) fn settings(&self) -> Vec<CuratedSetting<'_>> {
        self.sections
            .iter()
            .flat_map(|section| &section.items)
            .filter_map(|item| match item {
                Item::Setting {
                    key,
                    choices,
                    enabled_when,
                    ..
                } => Some((
                    key.as_str(),
                    choices.as_deref().unwrap_or_default(),
                    enabled_when
                        .iter()
                        .map(|(key, condition)| (key.clone(), condition.operands()))
                        .collect(),
                )),
                _ => None,
            })
            .collect()
    }

    /// Each push button's command and arguments.
    pub(crate) fn buttons(&self) -> Vec<(&str, &Value)> {
        self.sections
            .iter()
            .flat_map(|section| &section.items)
            .filter_map(|item| match item {
                Item::Button {
                    command: Some(command),
                    args,
                    ..
                } => Some((command.as_str(), args)),
                _ => None,
            })
            .collect()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Layout {
    pages: Vec<Spec>,
}

/// One push button, with its id stable for the life of the process.
#[derive(Debug)]
pub(super) struct Button {
    pub(super) caption: &'static str,
    pub(super) command: Option<&'static str>,
    pub(super) args: &'static Value,
    pub(super) action: Option<Action>,
}

static PAGES: LazyLock<Vec<Spec>> = LazyLock::new(|| {
    let layout: Layout = serde_json::from_value(
        Document::parse(LAYOUT)
            .expect("checked Settings pages layout")
            .value,
    )
    .expect("checked Settings pages schema");
    layout.pages
});

static BUTTONS: LazyLock<Vec<Button>> = LazyLock::new(|| {
    PAGES
        .iter()
        .flat_map(|page| &page.sections)
        .flat_map(|section| &section.items)
        .filter_map(|item| match item {
            Item::Button {
                button,
                command,
                args,
                action,
            } => Some(Button {
                caption: button.as_str(),
                command: command.as_deref(),
                args,
                action: *action,
            }),
            _ => None,
        })
        .collect()
});

pub(crate) fn pages() -> &'static [Spec] {
    &PAGES
}

pub(super) fn buttons() -> &'static [Button] {
    &BUTTONS
}

/// The id of the `ordinal`-th button on a page, counted across that page's sections.
pub(super) fn button_id(page: usize, ordinal: usize) -> usize {
    pages()[..page]
        .iter()
        .flat_map(|page| &page.sections)
        .flat_map(|section| &section.items)
        .filter(|item| matches!(item, Item::Button { .. }))
        .count()
        + ordinal
}

/// Every English source string the pages draw, for the catalog completeness check.
#[cfg(test)]
pub(crate) fn source_text() -> impl Iterator<Item = &'static str> {
    pages().iter().flat_map(|page| {
        std::iter::once(page.name.as_str()).chain(page.sections.iter().flat_map(|section| {
            std::iter::once(section.title.as_str()).chain(section.items.iter().filter_map(|item| {
                match item {
                    Item::Setting { label, .. } => Some(label.as_str()),
                    Item::Button { button, .. } => Some(button.as_str()),
                    Item::Preview { .. } => None,
                }
            }))
        }))
    })
}

/// A choice caption from `settings-ui.json`'s optional `labels`, with platform wording.
pub(super) fn expand_label(label: &'static str, tr: &vel_i18n::Translator) -> String {
    if label.contains("{file_manager}") {
        let manager = if cfg!(windows) {
            tr.tr("File Explorer")
        } else {
            tr.tr("Finder")
        };
        tr.tr_fmt(label, &[("file_manager", manager.into_owned())])
    } else {
        tr.tr(label).into_owned()
    }
}

/// The display name of a color scheme or theme value: its file name without the extension.
pub(super) fn resource_label(value: &str) -> String {
    let name = value.rsplit('/').next().unwrap_or(value);
    [".sublime-color-scheme", ".sublime-theme", ".tmTheme"]
        .into_iter()
        .find_map(|suffix| name.strip_suffix(suffix))
        .unwrap_or(name)
        .to_owned()
}

/// Keys whose curated popup lists the installed themes (`Some(false)`) or color schemes.
pub(super) fn catalog(key: &str) -> Option<bool> {
    match key {
        "theme" | "light_theme" | "dark_theme" => Some(false),
        "color_scheme" | "light_color_scheme" | "dark_color_scheme" => Some(true),
        _ => None,
    }
}

/// Windows names its default key bindings file for the platform, like the menu does.
pub(crate) fn platform_args(args: &Value) -> Value {
    #[cfg(windows)]
    {
        let mut args = args.clone();
        crate::workspace::use_windows_keymap_paths(&mut args);
        args
    }
    #[cfg(not(windows))]
    args.clone()
}
