//! Build-time inventory and on-demand parsing of non-UI language resources.
use std::{collections::BTreeMap, ops::Deref, path::Path, sync::OnceLock};
use vel_config::{
    Value,
    json::Document,
    resources::{Resource, Resources},
};

#[derive(Clone, Copy)]
pub(crate) enum Meta {
    None,
    // Emitted only if the bundled corpus contains a malformed resource.
    #[allow(dead_code)]
    Invalid(&'static str),
    Snippet {
        scope: &'static str,
        trigger: &'static str,
        description: &'static str,
    },
    Completion(&'static [&'static str]),
    Extensions(&'static [&'static str]),
}

struct File {
    path: &'static str,
    data: Option<&'static [u8]>,
    meta: Meta,
}

include!(concat!(env!("OUT_DIR"), "/shipped.rs"));

pub(crate) fn register(root: &Path, resources: &mut Resources) {
    for file in SHIPPED {
        if let Some(bytes) = file.data {
            let relative = file.path.strip_prefix("Packages/").expect("manifest path");
            resources.insert_embedded(file.path, bytes, root.join(relative));
        } else {
            resources.insert_shipped_plugin(file.path);
        }
    }
}

pub(crate) fn grammars() -> impl Iterator<Item = &'static str> {
    SHIPPED.iter().map(|file| file.path).filter(|path| {
        [".sublime-syntax", ".tmLanguage", ".tmLanguage.json"]
            .iter()
            .any(|suffix| path.ends_with(suffix))
    })
}

pub(crate) fn grammar_metadata() -> impl Iterator<Item = (&'static str, &'static [u8])> {
    SHIPPED
        .iter()
        .filter(|file| file.path.ends_with(".tmPreferences"))
        .map(|file| (file.path, file.data.expect("metadata resource")))
}

pub(crate) fn metadata(resource: &Resource) -> Option<(Meta, &'static [u8])> {
    if !resource.shipped() {
        return None;
    }
    let file = &SHIPPED[SHIPPED
        .binary_search_by_key(&resource.path.as_str(), |f| f.path)
        .ok()?];
    Some((file.meta, file.data?))
}

type Values = serde_json::Map<String, Value>;

#[derive(Debug, Default)]
struct SettingsLayer {
    sources: Vec<Resource>,
    extensions: Option<Vec<String>>,
    values: OnceLock<Values>,
}

#[derive(Debug, Default)]
struct SettingsFile {
    package: SettingsLayer,
    user: SettingsLayer,
}

#[derive(Debug, Default)]
pub(crate) struct SyntaxSettings(BTreeMap<String, SettingsFile>);

impl SyntaxSettings {
    pub(crate) fn add(&mut self, name: &str, resource: &Resource, messages: &mut Vec<String>) {
        let file = self.0.entry(name.to_owned()).or_default();
        let file = if resource.path.starts_with("Packages/User/") {
            &mut file.user
        } else {
            &mut file.package
        };
        match metadata(resource).map(|(meta, _)| meta) {
            Some(Meta::Extensions(values)) => {
                file.extensions = Some(values.iter().map(|v| (*v).into()).collect())
            }
            Some(Meta::Invalid(error)) => {
                messages.push(format!("{}: {error}", resource.path));
                return;
            }
            Some(_) => (),
            None => match resource.text().and_then(Document::parse) {
                Ok(doc) if doc.value.is_object() => {
                    if let Some(values) = doc.value.get("extensions") {
                        file.extensions = Some(
                            values
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|v| v.as_str().map(str::to_owned))
                                .collect(),
                        );
                    }
                }
                Ok(_) => {
                    messages.push(format!("{}: settings must be an object", resource.path));
                    return;
                }
                Err(e) => {
                    messages.push(format!("{}: {e}", resource.path));
                    return;
                }
            },
        }
        file.sources.push(resource.clone());
    }

    fn layers(&self, path: &str, display: &str) -> impl DoubleEndedIterator<Item = &SettingsLayer> {
        // Package settings precede User settings. Keep the display-name alias
        // for existing profiles, but prefer the syntax filename within User:
        // https://www.sublimetext.com/docs/settings.html#settings-files
        let stem = crate::workspace::syntax_setting_key(path);
        let name = self.0.get(stem);
        let alias = (display != stem).then(|| self.0.get(display)).flatten();
        [
            name.map(|f| &f.package),
            alias.map(|f| &f.package),
            alias.map(|f| &f.user),
            name.map(|f| &f.user),
        ]
        .into_iter()
        .flatten()
    }

    pub(crate) fn extensions(&self, path: &str, display_name: &str) -> &[String] {
        self.layers(path, display_name)
            .rev()
            .find_map(|layer| layer.extensions.as_deref())
            .unwrap_or_default()
    }

    pub(crate) fn values(&self, path: &str, display_name: &str) -> Values {
        let mut merged = Values::new();
        for layer in self
            .layers(path, display_name)
            .filter(|layer| !layer.sources.is_empty())
        {
            let values = layer.values.get_or_init(|| {
                let mut out = Values::new();
                for source in &layer.sources {
                    if let Ok(doc) = source.text().and_then(Document::parse)
                        && let Some(mut values) = doc.value.as_object().cloned()
                    {
                        if source.shipped() && source.path == "Packages/HTML/HTML.sublime-settings"
                        {
                            // These switches belong to the unported Python HTML provider.
                            values.remove("default_completions_selector");
                            values.remove("disable_default_completions");
                        }
                        out.append(&mut values);
                    }
                }
                out
            });
            merged.extend(values.clone());
        }
        merged
    }

    #[cfg(test)]
    pub(crate) fn loaded(&self, name: &str) -> bool {
        self.0.get(name).is_some_and(|entry| {
            entry.package.values.get().is_some() || entry.user.values.get().is_some()
        })
    }

    #[cfg(test)]
    pub(crate) fn contains_key(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }
}

#[derive(Debug, Default)]
pub(crate) struct Builds {
    files: Vec<Resource>,
    project: Vec<vel_build::BuildSystem>,
    cache: OnceLock<Vec<vel_build::BuildSystem>>,
}

impl Builds {
    pub(crate) fn new(files: Vec<Resource>, project: Vec<vel_build::BuildSystem>) -> Self {
        Self {
            files,
            project,
            cache: OnceLock::new(),
        }
    }

    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.files
            .iter()
            .map(build_name)
            .chain(self.project.iter().map(|b| b.name.as_str()))
    }

    fn get(&self) -> &Vec<vel_build::BuildSystem> {
        self.cache.get_or_init(|| {
            self.files
                .iter()
                .filter_map(|file| {
                    match file
                        .text()
                        .and_then(|s| vel_build::BuildSystem::parse(build_name(file), s))
                    {
                        Ok(build) => Some(build),
                        Err(e) => {
                            eprintln!("{}: {e}", file.path);
                            None
                        }
                    }
                })
                .chain(self.project.iter().cloned())
                .collect()
        })
    }

    #[cfg(test)]
    pub(crate) fn loaded(&self) -> bool {
        self.cache.get().is_some()
    }
}

fn build_name(file: &Resource) -> &str {
    file.path
        .rsplit('/')
        .next()
        .unwrap_or("")
        .trim_end_matches(".sublime-build")
}

impl Deref for Builds {
    type Target = Vec<vel_build::BuildSystem>;
    fn deref(&self) -> &Self::Target {
        self.get()
    }
}

#[cfg(test)]
impl std::ops::DerefMut for Builds {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.get();
        self.cache.get_mut().expect("build cache initialized")
    }
}

#[cfg(test)]
impl From<Vec<vel_build::BuildSystem>> for Builds {
    fn from(builds: Vec<vel_build::BuildSystem>) -> Self {
        Self {
            cache: OnceLock::from(builds),
            ..Self::default()
        }
    }
}

#[cfg(test)]
impl FromIterator<vel_build::BuildSystem> for Builds {
    fn from_iter<T: IntoIterator<Item = vel_build::BuildSystem>>(iter: T) -> Self {
        iter.into_iter().collect::<Vec<_>>().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_matches_the_corpus_without_shipping_ui_or_plugins() {
        let root = crate::language::packages_root();
        assert!(
            SHIPPED
                .windows(2)
                .all(|files| files[0].path < files[1].path)
        );
        for file in SHIPPED {
            assert!(!file.path.starts_with("Packages/Default/"));
            assert!(
                ![".sublime-menu", ".sublime-theme", ".tmTheme"]
                    .iter()
                    .any(|ext| file.path.ends_with(ext))
            );
            let path = root.join(file.path.strip_prefix("Packages/").unwrap());
            if let Some(bytes) = file.data {
                assert_eq!(std::fs::read(&path).unwrap(), bytes, "{}", file.path);
                if let Meta::Snippet {
                    scope,
                    trigger,
                    description,
                } = file.meta
                {
                    let s = vel_snippet::SnippetFile::parse(std::str::from_utf8(bytes).unwrap())
                        .unwrap();
                    assert_eq!(
                        (
                            s.scope.as_str(),
                            s.tab_trigger.as_str(),
                            s.description.as_str()
                        ),
                        (scope, trigger, description)
                    );
                }
                if let Meta::Completion(_) = file.meta {
                    let mut engine = vel_complete::CompletionEngine::new();
                    engine
                        .add_shipped_completions(std::str::from_utf8(bytes).unwrap())
                        .unwrap_or_else(|e| panic!("{}: {e}", file.path));
                }
            } else {
                assert!(file.path.ends_with(".py") && path.is_file());
            }
        }
    }
}
