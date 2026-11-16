//! Application resource loading and shared UI-thread theme state.
use std::{cell::RefCell, collections::BTreeMap, path::Path, rc::Rc, sync::Arc};
use vel_config::{resources::Resources, settings::Settings};
use vel_theme::{Appearance, FileIcons, Palette, Theme};
use vel_ui::theme::{SettingsGuard, ThemeUi};
type Compiled = (Arc<Theme>, Option<Arc<Theme>>);
type Sources = Vec<(String, String)>;
thread_local! { static DEFAULT:Rc<RefCell<ThemeUi>>=Rc::new(RefCell::new(ThemeUi::default())); }
pub fn default_ui() -> Rc<RefCell<ThemeUi>> {
    DEFAULT.with(Clone::clone)
}
#[derive(Debug)]
pub struct Themes {
    pub ui: Rc<RefCell<ThemeUi>>,
    sources: Sources,
    compiled: BTreeMap<(String, bool, String), Compiled>,
    unconditional: Option<Sources>,
    guards: Vec<SettingsGuard>,
    palettes: BTreeMap<String, Palette>,
    mappings: Vec<(String, FileIcons)>,
    icon_paths: BTreeMap<String, String>,
    pub icons: FileIcons,
    selected: Option<(String, bool, String, String)>,
}
/// Send-only resource preparation, including compiled appearance variants and decoded PNGs.
#[derive(Debug)]
pub struct Prepared {
    ui: vel_ui::theme::PreparedUi,
    sources: Sources,
    compiled: BTreeMap<(String, bool, String), Compiled>,
    unconditional: Option<Sources>,
    guards: Vec<SettingsGuard>,
    palettes: BTreeMap<String, Palette>,
    mappings: Vec<(String, FileIcons)>,
    icon_paths: BTreeMap<String, String>,
    icons: FileIcons,
    selected: Option<(String, bool, String, String)>,
    images: BTreeMap<String, Arc<[u8]>>,
}
impl Themes {
    pub fn prepare(
        resources: &Resources,
        schemes: &BTreeMap<String, Arc<vel_color_scheme::ColorScheme>>,
        settings: &Settings,
        messages: &mut Vec<String>,
    ) -> Prepared {
        let mut themes = Self::load(resources, schemes, messages);
        // Both OS appearance variants are warm before publication. Changing an
        // installed theme via preferences creates another worker configuration.
        for dark in [false, true] {
            if let Err(error) = themes.select(settings, dark) {
                messages.push(error);
            }
        }
        let images = [".png", ".jpg", ".jpeg", ".gif"]
            .into_iter()
            .flat_map(|suffix| resources.matching(suffix))
            .map(|r| (r.path.clone(), Arc::from(r.bytes.clone())))
            .collect();
        let ui = Rc::try_unwrap(themes.ui)
            .expect("worker theme has a single owner")
            .into_inner()
            .into_prepared();
        Prepared {
            ui,
            sources: themes.sources,
            compiled: themes.compiled,
            unconditional: themes.unconditional,
            guards: themes.guards,
            palettes: themes.palettes,
            mappings: themes.mappings,
            icon_paths: themes.icon_paths,
            icons: themes.icons,
            selected: themes.selected,
            images,
        }
    }
    pub fn publish(prepared: Prepared) -> Self {
        let ui = ThemeUi::from_prepared(prepared.ui);
        ui.metrics.set_image_resources(prepared.images);
        Self {
            ui: Rc::new(RefCell::new(ui)),
            sources: prepared.sources,
            compiled: prepared.compiled,
            unconditional: prepared.unconditional,
            guards: prepared.guards,
            palettes: prepared.palettes,
            mappings: prepared.mappings,
            icon_paths: prepared.icon_paths,
            icons: prepared.icons,
            selected: prepared.selected,
        }
    }
    pub fn load(
        resources: &Resources,
        schemes: &BTreeMap<String, Arc<vel_color_scheme::ColorScheme>>,
        messages: &mut Vec<String>,
    ) -> Self {
        let sources = resources
            .matching(".sublime-theme")
            .into_iter()
            .filter_map(|r| match r.text() {
                Ok(text) => Some((r.path.clone(), text.to_owned())),
                Err(e) => {
                    messages.push(e);
                    None
                }
            })
            .collect::<Vec<_>>();
        let mut mappings = Vec::new();
        let (unconditional, guards) = settings_free_sources(&sources);
        for mapping in resources.matching(".sublime-file-icons") {
            match mapping
                .text()
                .map_err(vel_theme::Error::Format)
                .and_then(FileIcons::parse)
            {
                Ok(icons) => mappings.push((mapping.path.clone(), icons)),
                Err(e) => messages.push(e.to_string()),
            }
        }
        let mut icon_paths = BTreeMap::new();
        let mut texture_names = std::collections::BTreeSet::new();
        for r in resources.matching(".png") {
            let relative = r.path.trim_start_matches("Packages/");
            let relative = relative
                .replace("@2x.png", ".png")
                .replace("@3x.png", ".png");
            icon_paths.insert(relative.trim_end_matches(".png").into(), relative.clone());
            if let Some(name) = relative.rsplit('/').next() {
                icon_paths.insert(name.trim_end_matches(".png").into(), relative.clone());
            }
            texture_names.insert(relative);
        }
        let mut ui = ThemeUi::default();
        let mut keys = Vec::new();
        let mut literal_settings = true;
        for (_, source) in &sources {
            if let Ok(doc) = vel_config::json::Document::parse(source) {
                let rules = doc
                    .value
                    .as_array()
                    .or_else(|| doc.value.get("rules").and_then(serde_json::Value::as_array));
                for rule in rules.into_iter().flatten() {
                    if let Some(values) = rule.get("settings") {
                        if !values.is_array() && !values.is_object() {
                            literal_settings = false;
                        }
                        if let Some(array) = values.as_array() {
                            literal_settings &= array
                                .iter()
                                .all(|v| v.as_str().is_some_and(|s| !s.contains("var(")));
                            keys.extend(
                                array
                                    .iter()
                                    .filter_map(serde_json::Value::as_str)
                                    .map(|s| s.trim_start_matches('!').to_owned()),
                            );
                        }
                        if let Some(object) = values.as_object() {
                            keys.extend(object.keys().cloned());
                        }
                    }
                }
            }
        }
        if literal_settings {
            ui.set_setting_keys(keys);
        }

        // Decode off the frame path. Density variants remain in resource storage for later screens.
        for name in texture_names {
            for density in [1., 2., 3.] {
                match vel_theme::load_texture(
                    &name,
                    density,
                    Default::default(),
                    true,
                    false,
                    |p| {
                        Ok(resources
                            .get(&format!("Packages/{p}"))
                            .map(|r| r.bytes.clone()))
                    },
                ) {
                    Ok(image) => ui.insert_texture(name.clone(), image),
                    Err(e) => messages.push(e.to_string()),
                }
            }
        }
        let mut image_resources = BTreeMap::new();
        for suffix in [".png", ".jpg", ".jpeg", ".gif"] {
            for r in resources.matching(suffix) {
                image_resources.insert(r.path.clone(), Arc::from(r.bytes.clone()));
            }
        }
        ui.metrics.set_image_resources(image_resources);
        let mut palettes: BTreeMap<String, Palette> = schemes
            .iter()
            .map(|(name, scheme)| (name.clone(), Palette::from_scheme(scheme)))
            .collect();
        for (name, palette) in &mut palettes {
            let mut vars = palette.variables.clone();
            let mut explicit = BTreeMap::new();
            for resource in resources
                .matching(".sublime-color-scheme")
                .into_iter()
                .filter(|r| r.path.rsplit('/').next() == Some(name.as_str()))
            {
                if let Ok(source) = resource.text().and_then(vel_config::json::Document::parse)
                    && let Some(variables) = source
                        .value
                        .get("variables")
                        .and_then(serde_json::Value::as_object)
                {
                    for (key, value) in variables {
                        if let Some(value) = value.as_str() {
                            vars.insert(key.clone(), value.into());
                            explicit.insert(key.clone(), value.to_owned());
                        }
                    }
                }
            }
            for key in palette.variables.keys().cloned().collect::<Vec<_>>() {
                if let Some(value) = explicit
                    .get(&key)
                    .or_else(|| explicit.get(key.trim_start_matches("--")))
                    && let Ok(color) = vel_theme::Color::parse(value, &vars)
                {
                    palette.set(&key, color);
                }
            }
            for (key, value) in explicit.into_iter().filter(|(k, _)| k.starts_with("--")) {
                if let Ok(color) = vel_theme::Color::parse(&value, &vars) {
                    palette.set(&key, color);
                } else {
                    palette.variables.insert(key, value);
                }
            }
        }
        Self {
            ui: Rc::new(RefCell::new(ui)),
            sources,
            unconditional,
            guards,
            compiled: BTreeMap::new(),
            palettes,
            mappings,
            icon_paths,
            icons: FileIcons::default(),
            selected: None,
        }
    }
    pub fn select(&mut self, settings: &Settings, dark: bool) -> Result<(), String> {
        let request = settings.string("theme", "auto");
        let name = if request == "auto" {
            settings.string(
                if dark { "dark_theme" } else { "light_theme" },
                if dark {
                    "Velocity.sublime-theme"
                } else {
                    "Velocity Light.sublime-theme"
                },
            )
        } else {
            request
        };
        let scheme = settings.string("color_scheme", "auto");
        let scheme = if scheme == "auto" {
            settings.string(
                if dark {
                    "dark_color_scheme"
                } else {
                    "light_color_scheme"
                },
                if dark {
                    "Velocity Dark.sublime-color-scheme"
                } else {
                    "Velocity Light.sublime-color-scheme"
                },
            )
        } else {
            scheme
        };
        let global_icons = settings.string("file_icon_theme", "Velocity.sublime-file-icons");
        if self
            .selected
            .as_ref()
            .is_some_and(|(old, appearance, old_scheme, icons)| {
                old == name && *appearance == dark && old_scheme == scheme && icons == global_icons
            })
        {
            return Ok(());
        }
        let key = (name.to_owned(), dark, scheme.to_owned());
        let palette = self
            .palettes
            .get(scheme.rsplit('/').next().unwrap_or(scheme))
            .or_else(|| {
                self.palettes.get(if dark {
                    "Velocity Dark.sublime-color-scheme"
                } else {
                    "Velocity Light.sublime-color-scheme"
                })
            })
            .cloned()
            .unwrap_or_default();
        let mut icons = FileIcons::default();
        let theme_icons = format!(
            "{}.sublime-file-icons",
            name.trim_end_matches(".sublime-theme")
        );
        for wanted in [global_icons, theme_icons.as_str()] {
            for (path, mapping) in &self.mappings {
                if path == wanted || path.rsplit('/').next() == Some(wanted) {
                    icons.merge(mapping);
                }
            }
        }
        self.icons = icons;
        let mut error = None;
        let theme = self
            .compiled
            .entry(key.clone())
            .or_insert_with(|| {
                let resources = self
                    .sources
                    .iter()
                    .map(|(path, contents)| vel_theme::Resource { path, contents })
                    .collect::<Vec<_>>();
                let full = Arc::new(
                    match vel_theme::load_resources(&resources, name, &palette) {
                        Ok(theme) => theme,
                        Err(e) => {
                            error = Some(e.to_string());
                            Theme::velocity(
                                if dark {
                                    Appearance::Dark
                                } else {
                                    Appearance::Light
                                },
                                &palette,
                            )
                        }
                    },
                );
                let unconditional = self
                    .unconditional
                    .as_ref()
                    .filter(|_| error.is_none())
                    .and_then(|sources| {
                        let resources = sources
                            .iter()
                            .map(|(path, contents)| vel_theme::Resource { path, contents })
                            .collect::<Vec<_>>();
                        vel_theme::load_resources(&resources, name, &palette)
                            .ok()
                            .map(Arc::new)
                    });
                (full, unconditional)
            })
            .clone();
        let mut ui = self.ui.borrow_mut();
        ui.replace(theme.0, palette);
        if let Some(unconditional) = theme.1 {
            ui.set_unconditional(unconditional, self.guards.clone());
        }
        self.selected = Some((key.0, key.1, key.2, global_icons.into()));
        error.map_or(Ok(()), Err)
    }
    pub fn file_icon(&self, path: &Path) -> Option<&str> {
        self.icons
            .resolve(path)
            .and_then(|icon| {
                self.icon_paths.get(
                    icon.trim_start_matches("Packages/")
                        .trim_end_matches(".png"),
                )
            })
            .map(String::as_str)
            .or(Some("Theme - Velocity/file.png"))
    }
}
fn settings_free_sources(sources: &[(String, String)]) -> (Option<Sources>, Vec<SettingsGuard>) {
    let mut guards = Vec::new();
    let mut filtered = Vec::new();
    for (path, source) in sources {
        let Ok(mut doc) = vel_config::json::Document::parse(source) else {
            return (None, Vec::new());
        };
        let rules = if doc.value.is_array() {
            doc.value.as_array_mut()
        } else {
            doc.value
                .get_mut("rules")
                .and_then(serde_json::Value::as_array_mut)
        };
        if let Some(rules) = rules {
            for rule in rules.iter().filter(|r| r.get("settings").is_some()) {
                let Some(guard) = SettingsGuard::from_rule(rule) else {
                    return (None, Vec::new());
                };
                if !guards.contains(&guard) {
                    guards.push(guard);
                }
            }
            rules.retain(|r| r.get("settings").is_none());
        }
        filtered.push((path.clone(), doc.value.to_string()));
    }
    (Some(filtered), guards)
}
/// Inserts only original, redistributable Velocity resources; installed packages/User load later.
pub fn bundled(resources: &mut Resources) {
    let origin = std::path::PathBuf::from("<embedded>");
    for (name, bytes) in BUNDLED {
        resources.insert(
            &format!("Packages/Theme - Velocity/{name}"),
            bytes.to_vec(),
            origin.clone(),
        );
    }
}
const BUNDLED: &[(&str, &[u8])] = &[
    (
        "Velocity.sublime-theme",
        include_bytes!("../resources/Packages/Theme - Velocity/Velocity.sublime-theme"),
    ),
    (
        "Velocity Light.sublime-theme",
        include_bytes!("../resources/Packages/Theme - Velocity/Velocity Light.sublime-theme"),
    ),
    (
        "Velocity.sublime-file-icons",
        include_bytes!("../resources/Packages/Theme - Velocity/Velocity.sublime-file-icons"),
    ),
    (
        "branch.png",
        include_bytes!("../resources/Packages/Theme - Velocity/branch.png"),
    ),
    (
        "branch@2x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/branch@2x.png"),
    ),
    (
        "branch@3x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/branch@3x.png"),
    ),
    (
        "buffer.png",
        include_bytes!("../resources/Packages/Theme - Velocity/buffer.png"),
    ),
    (
        "buffer@2x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/buffer@2x.png"),
    ),
    (
        "buffer@3x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/buffer@3x.png"),
    ),
    (
        "case.png",
        include_bytes!("../resources/Packages/Theme - Velocity/case.png"),
    ),
    (
        "case@2x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/case@2x.png"),
    ),
    (
        "case@3x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/case@3x.png"),
    ),
    (
        "chevron-down.png",
        include_bytes!("../resources/Packages/Theme - Velocity/chevron-down.png"),
    ),
    (
        "chevron-down@2x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/chevron-down@2x.png"),
    ),
    (
        "chevron-down@3x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/chevron-down@3x.png"),
    ),
    (
        "chevron-left.png",
        include_bytes!("../resources/Packages/Theme - Velocity/chevron-left.png"),
    ),
    (
        "chevron-left@2x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/chevron-left@2x.png"),
    ),
    (
        "chevron-left@3x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/chevron-left@3x.png"),
    ),
    (
        "chevron.png",
        include_bytes!("../resources/Packages/Theme - Velocity/chevron.png"),
    ),
    (
        "chevron@2x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/chevron@2x.png"),
    ),
    (
        "chevron@3x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/chevron@3x.png"),
    ),
    (
        "close.png",
        include_bytes!("../resources/Packages/Theme - Velocity/close.png"),
    ),
    (
        "close@2x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/close@2x.png"),
    ),
    (
        "close@3x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/close@3x.png"),
    ),
    (
        "context.png",
        include_bytes!("../resources/Packages/Theme - Velocity/context.png"),
    ),
    (
        "context@2x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/context@2x.png"),
    ),
    (
        "context@3x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/context@3x.png"),
    ),
    (
        "dot.png",
        include_bytes!("../resources/Packages/Theme - Velocity/dot.png"),
    ),
    (
        "dot@2x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/dot@2x.png"),
    ),
    (
        "dot@3x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/dot@3x.png"),
    ),
    (
        "file.png",
        include_bytes!("../resources/Packages/Theme - Velocity/file.png"),
    ),
    (
        "file@2x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/file@2x.png"),
    ),
    (
        "file@3x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/file@3x.png"),
    ),
    (
        "folder.png",
        include_bytes!("../resources/Packages/Theme - Velocity/folder.png"),
    ),
    (
        "folder@2x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/folder@2x.png"),
    ),
    (
        "folder@3x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/folder@3x.png"),
    ),
    (
        "gitignore.png",
        include_bytes!("../resources/Packages/Theme - Velocity/gitignore.png"),
    ),
    (
        "gitignore@2x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/gitignore@2x.png"),
    ),
    (
        "gitignore@3x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/gitignore@3x.png"),
    ),
    (
        "highlight.png",
        include_bytes!("../resources/Packages/Theme - Velocity/highlight.png"),
    ),
    (
        "highlight@2x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/highlight@2x.png"),
    ),
    (
        "highlight@3x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/highlight@3x.png"),
    ),
    (
        "plus.png",
        include_bytes!("../resources/Packages/Theme - Velocity/plus.png"),
    ),
    (
        "plus@2x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/plus@2x.png"),
    ),
    (
        "plus@3x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/plus@3x.png"),
    ),
    (
        "preserve-case.png",
        include_bytes!("../resources/Packages/Theme - Velocity/preserve-case.png"),
    ),
    (
        "preserve-case@2x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/preserve-case@2x.png"),
    ),
    (
        "preserve-case@3x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/preserve-case@3x.png"),
    ),
    (
        "regex.png",
        include_bytes!("../resources/Packages/Theme - Velocity/regex.png"),
    ),
    (
        "regex@2x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/regex@2x.png"),
    ),
    (
        "regex@3x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/regex@3x.png"),
    ),
    (
        "selection.png",
        include_bytes!("../resources/Packages/Theme - Velocity/selection.png"),
    ),
    (
        "selection@2x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/selection@2x.png"),
    ),
    (
        "selection@3x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/selection@3x.png"),
    ),
    (
        "sidebar-right.png",
        include_bytes!("../resources/Packages/Theme - Velocity/sidebar-right.png"),
    ),
    (
        "sidebar-right@2x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/sidebar-right@2x.png"),
    ),
    (
        "sidebar-right@3x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/sidebar-right@3x.png"),
    ),
    (
        "sidebar.png",
        include_bytes!("../resources/Packages/Theme - Velocity/sidebar.png"),
    ),
    (
        "sidebar@2x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/sidebar@2x.png"),
    ),
    (
        "sidebar@3x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/sidebar@3x.png"),
    ),
    (
        "whole-word.png",
        include_bytes!("../resources/Packages/Theme - Velocity/whole-word.png"),
    ),
    (
        "whole-word@2x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/whole-word@2x.png"),
    ),
    (
        "whole-word@3x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/whole-word@3x.png"),
    ),
    (
        "wrap.png",
        include_bytes!("../resources/Packages/Theme - Velocity/wrap.png"),
    ),
    (
        "wrap@2x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/wrap@2x.png"),
    ),
    (
        "wrap@3x.png",
        include_bytes!("../resources/Packages/Theme - Velocity/wrap@3x.png"),
    ),
];
