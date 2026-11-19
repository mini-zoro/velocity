//! Workspace-local UI translations. English source text is the fallback.
use std::{
    borrow::Cow,
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

/// A supported presentation language.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    /// English, also used for unknown language settings.
    #[default]
    En,
    /// Simplified Chinese.
    ZhHans,
}

impl Language {
    /// Resolves a preference. Auto uses the user's first macOS preferred language.
    pub fn from_setting(setting: &str) -> Self {
        match setting {
            "zh-Hans" => Self::ZhHans,
            "auto" => Self::from_preferred(&preferred_language()),
            _ => Self::En,
        }
    }

    fn from_preferred(language: &str) -> Self {
        if language.starts_with("zh") {
            Self::ZhHans
        } else {
            Self::En
        }
    }
}

#[cfg(target_os = "macos")]
fn preferred_language() -> String {
    // https://developer.apple.com/documentation/foundation/nslocale/preferredlanguages
    objc2_foundation::NSLocale::preferredLanguages()
        .firstObject()
        .map(|value| value.to_string())
        .unwrap_or_default()
}

#[cfg(not(target_os = "macos"))]
fn preferred_language() -> String {
    String::new()
}

/// One language's validated English-to-localized string map.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Catalog {
    /// Stable source text and its translation.
    pub strings: HashMap<String, String>,
}

impl Catalog {
    /// Loads one JSON-with-comments layer. Invalid entries reject the whole layer.
    pub fn load(source: &str) -> Result<Self, String> {
        let document = vel_config::json::Document::parse(source)?;
        let entries = document.value["strings"]
            .as_object()
            .ok_or("Expected a strings object")?;
        let mut strings = HashMap::with_capacity(entries.len());
        for (key, value) in entries {
            let translation = value
                .as_str()
                .ok_or_else(|| format!("{key}: expected a string"))?;
            if placeholders(key) != placeholders(translation) {
                return Err(format!("{key}: translation changes the named placeholders"));
            }
            strings.insert(key.clone(), translation.to_owned());
        }
        Ok(Self { strings })
    }

    /// Gives a later package layer precedence over the current strings.
    pub fn merge(&mut self, later: Self) {
        self.strings.extend(later.strings);
    }
}

/// Active lookup table and revision. Consumers cache derived menus and palette
/// candidates at this boundary rather than translating in the render loop.
#[derive(Clone, Debug, Default)]
pub struct Translator {
    language: Language,
    chinese: Arc<Catalog>,
    revision: u64,
}

impl Translator {
    /// Refreshes the active language and returns true only when it changed.
    pub fn select(&mut self, language: Language) -> bool {
        if self.language == language {
            return false;
        }
        self.language = language;
        self.revision = self.revision.wrapping_add(1);
        true
    }

    /// Installs a merged resource snapshot, retaining the current language.
    pub fn replace(&mut self, catalog: Catalog) {
        if *self.chinese != catalog {
            self.chinese = Arc::new(catalog);
            self.revision = self.revision.wrapping_add(1);
        }
    }

    /// The cache key for derived translations.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Looks up a resource label without allocating.
    pub fn caption<'a>(&'a self, source: &'a str) -> &'a str {
        if self.language == Language::ZhHans {
            self.chinese
                .strings
                .get(source)
                .map_or(source, String::as_str)
        } else {
            source
        }
    }

    /// A localized palette label. Explicit rows have a full-source entry;
    /// menu-derived rows can also reuse the individual menu captions.
    pub fn palette_caption(&self, source: &str) -> Option<String> {
        if self.language == Language::En {
            return None;
        }
        let direct = self.caption(source);
        if direct != source {
            return Some(direct.to_owned());
        }
        let mut parts = source.split(": ");
        let first = parts.next()?;
        let first_localized = self.caption(first);
        let mut output = first_localized.to_owned();
        let mut changed = first != first_localized;
        for part in parts {
            let caption = self.caption(part);
            changed |= caption != part;
            output.push('：');
            output.push_str(caption);
        }
        changed.then_some(output)
    }

    /// Looks up a UI literal, borrowing the English fallback.
    pub fn tr(&self, source: &'static str) -> Cow<'static, str> {
        let caption = self.caption(source);
        if caption == source {
            Cow::Borrowed(source)
        } else {
            Cow::Owned(caption.to_owned())
        }
    }

    /// Translates, then interpolates named arguments without reprocessing their values.
    pub fn tr_fmt(&self, source: &'static str, args: &[(&str, String)]) -> String {
        let caption = self.caption(source);
        let mut out = String::with_capacity(caption.len());
        let mut at = 0;
        for (start, name) in tokens(caption) {
            if let Some((_, value)) = args.iter().find(|(key, _)| *key == name) {
                out.push_str(&caption[at..start]);
                out.push_str(value);
                at = start + name.len() + 2;
            }
        }
        out.push_str(&caption[at..]);
        out
    }
}

fn tokens(s: &str) -> impl Iterator<Item = (usize, &str)> {
    s.match_indices('{').filter_map(|(start, _)| {
        let (name, _) = s[start + 1..].split_once('}')?;
        let valid =
            !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        valid.then_some((start, name))
    })
}

fn placeholders(s: &str) -> BTreeSet<&str> {
    tokens(s).map(|(_, name)| name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogs_load_merge_validate_and_fall_back() {
        let mut catalog = Catalog::load(
            r#"{/* default */ "strings":{"Save":"保存","Open {name}":"打开 {name}",},}"#,
        )
        .unwrap();
        catalog.merge(Catalog::load(r#"{"strings":{"Save":"存储","New":"新建"}}"#).unwrap());
        catalog.merge(Catalog::load(r#"{"strings":{"Save":"保存文件"}}"#).unwrap());
        assert!(Catalog::load(r#"{"strings":{"Open {name}":"打开 {path}"}}"#).is_err());
        let mut tr = Translator::default();
        tr.replace(catalog);
        assert!(tr.select(Language::from_setting("zh-Hans")));
        assert_eq!(tr.tr("Save"), "保存文件");
        assert_eq!(tr.tr("New"), "新建");
        assert_eq!(
            tr.tr_fmt("Open {name}", &[("name", "{path}".into())]),
            "打开 {path}"
        );
        assert!(matches!(tr.tr("Unknown"), Cow::Borrowed("Unknown")));
        let revision = tr.revision();
        assert!(!tr.select(Language::ZhHans));
        assert_eq!(tr.revision(), revision);
        assert!(tr.select(Language::from_setting("invalid")));
        assert_eq!(tr.tr("Save"), "Save");
        assert_eq!(Language::from_preferred("zh-CN"), Language::ZhHans);
        assert_eq!(Language::from_preferred("ja-JP"), Language::En);
    }
}
