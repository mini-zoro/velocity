//! Workspace-local UI translations. English source text is the fallback.
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

/// A case-normalized BCP-47 locale tag, independent of installed catalogs.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Language(Cow<'static, str>);

impl Language {
    /// English source text is built in, not installed as a catalog.
    pub const ENGLISH: Self = Self(Cow::Borrowed("en"));

    /// Checks subtag boundaries and normalizes case without consulting the IANA
    /// registry. This also accepts private-use and grandfathered tags.
    pub fn parse(tag: &str) -> Option<Self> {
        let mut parts = tag.split('-');
        let first = parts.next()?;
        let singleton = first.eq_ignore_ascii_case("x") || first.eq_ignore_ascii_case("i");
        if (!singleton && !(2..=8).contains(&first.len()))
            || !first.bytes().all(|b| b.is_ascii_alphabetic())
            || (singleton && !tag.contains('-'))
            || tag.eq_ignore_ascii_case("auto")
        {
            return None;
        }
        if tag.eq_ignore_ascii_case("en") {
            return Some(Self::ENGLISH);
        }
        let mut tag = first.to_ascii_lowercase();
        let mut core = !singleton;
        for part in parts {
            if part.is_empty() || part.len() > 8 || !part.bytes().all(|b| b.is_ascii_alphanumeric())
            {
                return None;
            }
            tag.push('-');
            let start = tag.len();
            tag.push_str(part);
            tag[start..].make_ascii_lowercase();
            if core && part.bytes().all(|b| b.is_ascii_alphabetic()) {
                match part.len() {
                    4 => tag[start..start + 1].make_ascii_uppercase(),
                    2 => tag[start..].make_ascii_uppercase(),
                    _ => {}
                }
            }
            core &= part.len() > 1;
        }
        // A dangling extension/private-use singleton is not a tag.
        if core || tag.rsplit('-').next()?.len() != 1 || tag.contains("-x-") || singleton {
            Some(Self(Cow::Owned(tag)))
        } else {
            None
        }
    }

    /// The locale's spelling for settings and display.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether all UI text comes from the English source literals.
    pub fn is_english(&self) -> bool {
        self.0 == "en"
    }

    fn parents(&self) -> impl Iterator<Item = &str> {
        // RFC 4647 lookup: trim a trailing singleton along with the removed
        // subtag, e.g. de-DE-x-phonebook -> de-DE -> de.
        std::iter::successors(Some(self.as_str()), |tag| {
            let (parent, _) = tag.rsplit_once('-')?;
            if parent.rsplit('-').next()?.len() == 1 {
                parent.rsplit_once('-').map(|(prefix, _)| prefix)
            } else {
                Some(parent)
            }
        })
    }
}

impl Default for Language {
    fn default() -> Self {
        Self::ENGLISH
    }
}

#[cfg(target_os = "macos")]
fn preferred_languages() -> Vec<String> {
    // https://developer.apple.com/documentation/foundation/nslocale/preferredlanguages
    objc2_foundation::NSLocale::preferredLanguages()
        .iter()
        .map(|value| value.to_string())
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn preferred_languages() -> Vec<String> {
    Vec::new()
}

/// One language's validated English-to-localized string map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Catalog {
    /// The name shown in the language picker, in the language itself.
    pub language_name: String,
    /// Optional English name for the palette's searchable English caption.
    pub english_name: Option<String>,
    /// Separator for palette headings composed from individual menu captions.
    pub palette_separator: Option<String>,
    /// Stable source text and its translation.
    pub strings: HashMap<String, String>,
}

impl Catalog {
    /// Loads one JSON-with-comments layer. Invalid entries reject the whole layer.
    pub fn load(source: &str) -> Result<Self, String> {
        let document = vel_config::json::Document::parse(source)?;
        let name = |key| match document.value.get(key) {
            None => Ok(None),
            Some(value) => value
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .map(|value| Some(value.to_owned()))
                .ok_or_else(|| format!("{key}: expected a nonempty string")),
        };
        let language_name = name("language_name")?.ok_or("Expected a language_name")?;
        let english_name = name("english_name")?;
        let palette_separator = name("palette_separator")?;
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
        Ok(Self {
            language_name,
            english_name,
            palette_separator,
            strings,
        })
    }

    /// Gives a later package layer precedence over the current strings.
    fn merge(&mut self, later: Self) {
        self.language_name = later.language_name;
        if later.english_name.is_some() {
            self.english_name = later.english_name;
        }
        if later.palette_separator.is_some() {
            self.palette_separator = later.palette_separator;
        }
        self.strings.extend(later.strings);
    }
}

/// Merged catalog inventory, in tag order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Catalogs(BTreeMap<Language, Arc<Catalog>>);

impl Catalogs {
    /// Loads Default, package and User resources in their normal precedence.
    pub fn load(resources: &vel_config::resources::Resources, messages: &mut Vec<String>) -> Self {
        let mut catalogs = Self::default();
        for resource in resources.matching(".sublime-localization") {
            let Some((_, relative)) = resource
                .path
                .strip_prefix("Packages/")
                .and_then(|p| p.split_once('/'))
            else {
                continue;
            };
            let Some(tag) = relative
                .strip_prefix("Localization/")
                .and_then(|file| file.strip_suffix(".sublime-localization"))
                .filter(|tag| !tag.contains('/'))
            else {
                continue;
            };
            let result = Language::parse(tag)
                .filter(|tag| !tag.is_english())
                .ok_or_else(|| "Expected a non-English locale tag".to_owned())
                .and_then(|tag| {
                    let catalog = Catalog::load(resource.text()?)?;
                    catalogs.insert(tag, catalog);
                    Ok(())
                });
            if let Err(error) = result {
                messages.push(format!("{}: {error}", resource.path));
            }
        }
        catalogs
    }

    /// Adds a validated layer for a locale.
    pub fn insert(&mut self, language: Language, catalog: Catalog) {
        if language.is_english() {
            return;
        }
        match self.0.entry(language) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(Arc::new(catalog));
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                Arc::make_mut(entry.get_mut()).merge(catalog);
            }
        }
    }

    /// Installed catalogs only. English is always available separately.
    pub fn iter(&self) -> impl Iterator<Item = (&Language, &Catalog)> {
        self.0.iter().map(|(tag, catalog)| (tag, catalog.as_ref()))
    }

    fn get(&self, tag: &str) -> Option<(&Language, &Arc<Catalog>)> {
        self.0.get_key_value(tag)
    }
}

impl std::borrow::Borrow<str> for Language {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

/// Active lookup table and revision. Consumers cache derived menus and palette
/// candidates at this boundary rather than translating in the render loop.
#[derive(Clone, Debug, Default)]
pub struct Translator {
    language: Language,
    catalogs: Option<Arc<Catalogs>>,
    chain: Vec<(Language, Arc<Catalog>)>,
    revision: u64,
}

impl Translator {
    /// Refreshes the active language and returns true only when it changed.
    pub fn select(&mut self, language: Language) -> bool {
        if self.language == language {
            return false;
        }
        self.language = language;
        self.refresh_chain();
        true
    }

    /// Installs a merged resource snapshot, retaining the current language.
    pub fn replace(&mut self, catalogs: Catalogs) {
        if self.catalogs.as_deref() != Some(&catalogs) {
            self.catalogs = Some(Arc::new(catalogs));
            self.refresh_chain();
        }
    }

    fn refresh_chain(&mut self) {
        self.chain = self
            .catalogs
            .iter()
            .flat_map(|catalogs| self.language.parents().filter_map(|tag| catalogs.get(tag)))
            .map(|(tag, catalog)| (tag.clone(), Arc::clone(catalog)))
            .collect();
        self.revision = self.revision.wrapping_add(1);
    }

    /// Resolves a setting or reports that no available locale matches it.
    pub fn resolve(&self, setting: &str) -> Option<Language> {
        if setting == "auto" {
            return Some(self.resolve_preferred(preferred_languages()));
        }
        self.resolve_tag(setting)
    }

    fn resolve_tag(&self, setting: &str) -> Option<Language> {
        let requested = Language::parse(setting)?;
        let matched = requested.parents().find(|&tag| {
            tag == "en"
                || self
                    .catalogs
                    .as_ref()
                    .is_some_and(|all| all.get(tag).is_some())
        })?;
        Some(if matched == "en" {
            Language::ENGLISH
        } else {
            requested
        })
    }

    /// Uses the first preferred tag with an exact or parent match.
    pub fn resolve_preferred(
        &self,
        preferred: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Language {
        preferred
            .into_iter()
            .find_map(|tag| self.resolve_tag(tag.as_ref()))
            .unwrap_or_default()
    }

    /// The closest installed tag, for the checked language-picker row.
    pub fn matched_language(&self) -> &Language {
        self.chain
            .first()
            .map_or(&Language::ENGLISH, |(tag, _)| tag)
    }

    /// An exact catalog, for looking up a row's native name.
    pub fn catalog(&self, tag: &str) -> Option<&Catalog> {
        self.catalogs
            .as_ref()?
            .get(tag)
            .map(|(_, catalog)| catalog.as_ref())
    }

    /// The cache key for derived translations.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The active presentation language.
    pub fn language(&self) -> &Language {
        &self.language
    }

    fn translated(&self, source: &str) -> Option<&str> {
        self.chain
            .iter()
            .find_map(|(_, catalog)| catalog.strings.get(source).map(String::as_str))
    }

    /// The separator between localized palette segments.
    pub fn palette_separator(&self) -> &str {
        self.chain
            .iter()
            .find_map(|(_, catalog)| catalog.palette_separator.as_deref())
            .unwrap_or(": ")
    }

    /// Looks up a resource label without allocating.
    pub fn caption<'a>(&'a self, source: &'a str) -> &'a str {
        self.translated(source).unwrap_or(source)
    }

    /// A localized palette label. Explicit rows have a full-source entry;
    /// menu-derived rows can also reuse the individual menu captions.
    pub fn palette_caption(&self, source: &str) -> Option<String> {
        if let Some(direct) = self.translated(source) {
            return Some(direct.to_owned());
        }
        let mut parts = source.split(": ");
        let mut output = self.translated(parts.next()?)?.to_owned();
        let separator = self.palette_separator();
        for part in parts {
            output.push_str(separator);
            if let Some(translation) = self.translated(part) {
                output.push_str(translation);
            } else if !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()) {
                output.push_str(part);
            } else {
                return None;
            }
        }
        Some(output)
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
        format_named(self.caption(source), args)
    }
}

/// Interpolates a template already resolved for the current language revision.
pub fn format_named(caption: &str, args: &[(&str, impl AsRef<str>)]) -> String {
    let mut out = String::with_capacity(caption.len());
    let mut at = 0;
    for (start, name) in tokens(caption) {
        if let Some((_, value)) = args.iter().find(|(key, _)| *key == name) {
            out.push_str(&caption[at..start]);
            out.push_str(value.as_ref());
            at = start + name.len() + 2;
        }
    }
    out.push_str(&caption[at..]);
    out
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

    fn catalog(name: &str, entries: &str) -> Catalog {
        Catalog::load(&format!(
            r#"{{"language_name":"{name}", "strings":{entries}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn catalogs_load_merge_validate_and_fall_back() {
        let mut catalogs = Catalogs::default();
        let zh = Language::parse("zh-Hans").unwrap();
        catalogs.insert(
            zh.clone(),
            catalog(
                "简体中文",
                r#"{/* default */ "Save":"保存","Open {name}":"打开 {name}",}"#,
            ),
        );
        catalogs.insert(
            zh.clone(),
            catalog("简体中文", r#"{"Save":"存储","New":"新建"}"#),
        );
        catalogs.insert(zh.clone(), catalog("简体中文", r#"{"Save":"保存文件"}"#));
        assert!(
            Catalog::load(
                r#"{"language_name":"简体中文","strings":{"Open {name}":"打开 {path}"}}"#
            )
            .is_err()
        );
        assert!(Catalog::load(r#"{"strings":{}}"#).is_err());
        let mut tr = Translator::default();
        tr.replace(catalogs.clone());
        assert!(tr.select(tr.resolve("ZH-hans").unwrap()));
        assert_eq!(tr.tr("Save"), "保存文件");
        assert_eq!(tr.tr("New"), "新建");
        assert_eq!(
            tr.tr_fmt("Open {name}", &[("name", "{path}".into())]),
            "打开 {path}"
        );
        assert!(matches!(tr.tr("Unknown"), Cow::Borrowed("Unknown")));
        let revision = tr.revision();
        assert!(!tr.select(zh));
        tr.replace(catalogs);
        assert_eq!(tr.revision(), revision);
    }

    #[test]
    fn locale_lookup_uses_exact_parents_and_first_supported_preference() {
        let mut catalogs = Catalogs::default();
        for (tag, entries) in [
            ("zh-Hans-CN", r#"{"Region":"region","Save":"region save"}"#),
            ("zh-Hans", r#"{"Script":"script","Save":"script save"}"#),
            ("zh", r#"{"Parent":"parent","Save":"parent save"}"#),
            ("x-test", r#"{"Save":"[Ŝåvé]"}"#),
        ] {
            catalogs.insert(Language::parse(tag).unwrap(), catalog(tag, entries));
        }
        let mut tr = Translator::default();
        tr.replace(catalogs.clone());
        tr.select(tr.resolve("zh-Hans-CN-x-custom").unwrap());
        assert_eq!(tr.language().as_str(), "zh-Hans-CN-x-custom");
        assert_eq!(tr.matched_language().as_str(), "zh-Hans-CN");
        for (source, expected) in [
            ("Save", "region save"),
            ("Region", "region"),
            ("Script", "script"),
            ("Parent", "parent"),
            ("Missing", "Missing"),
        ] {
            assert_eq!(tr.caption(source), expected);
        }

        catalogs.0.remove("zh-Hans-CN");
        catalogs.0.remove("zh");
        tr.replace(catalogs);
        assert_eq!(tr.caption("Save"), "script save");
        tr.select(tr.resolve("zh-hans-cn").unwrap());
        assert_eq!(tr.matched_language().as_str(), "zh-Hans");
        assert_eq!(tr.resolve("zh-TW"), None);
        assert_eq!(tr.resolve("en-GB"), Some(Language::ENGLISH));
        for (preferred, expected) in [
            (["x-test", "zh-Hans-CN"], "x-test"),
            (["zh-TW", "zh-Hans-CN"], "zh-Hans-CN"),
            (["zh-TW", "en-GB"], "en"),
            (["en-GB", "zh-Hans"], "en"),
            (["zh-TW", "ja-JP"], "en"),
        ] {
            assert_eq!(tr.resolve_preferred(preferred).as_str(), expected);
        }
    }

    #[test]
    fn resources_discover_only_catalogs_and_merge_in_package_order() {
        let mut resources = vel_config::resources::Resources::default();
        for (path, json) in [
            (
                "Default/Localization/de.sublime-localization",
                r#"{"language_name":"Deutsch","english_name":"German","strings":{"Save":"Sichern","Open":"Öffnen"}}"#,
            ),
            (
                "Pack/Localization/DE.sublime-localization",
                r#"{"language_name":"Deutsch","strings":{"Save":"Speichern","New":"Neu"}}"#,
            ),
            (
                "User/Localization/de.sublime-localization",
                r#"{"language_name":"Deutsch (User)","strings":{"Save":"Speichern!"}}"#,
            ),
            (
                "User/Localization/subdir/ja.sublime-localization",
                r#"{"language_name":"日本語","strings":{}}"#,
            ),
            (
                "User/Other/fr.sublime-localization",
                r#"{"language_name":"français","strings":{}}"#,
            ),
            (
                "User/Localization/en.sublime-localization",
                r#"{"language_name":"English","strings":{}}"#,
            ),
            (
                "User/Localization/pt.sublime-localization",
                r#"{"language_name":"Português","strings":{"{name}":"{wrong}"}}"#,
            ),
        ] {
            resources.insert(
                &format!("Packages/{path}"),
                json.as_bytes().to_vec(),
                "<test>".into(),
            );
        }
        let mut messages = Vec::new();
        let catalogs = Catalogs::load(&resources, &mut messages);
        assert_eq!(messages.len(), 2, "{messages:?}");
        let all = catalogs.iter().collect::<Vec<_>>();
        assert_eq!(all.len(), 1);
        let (tag, de) = all[0];
        assert_eq!(tag.as_str(), "de");
        assert_eq!(de.english_name.as_deref(), Some("German"));
        assert_eq!(de.language_name, "Deutsch (User)");
        assert_eq!(de.strings["Save"], "Speichern!");
        assert_eq!(de.strings["Open"], "Öffnen");
        assert_eq!(de.strings["New"], "Neu");
    }

    #[test]
    fn palette_uses_a_whole_entry_or_only_complete_compositions() {
        let mut catalogs = Catalogs::default();
        let tag = Language::parse("zh-Hans").unwrap();
        catalogs.insert(
            tag.clone(),
            Catalog::load(
                r#"{"language_name":"简体中文", "palette_separator":"：", "strings":{
            "Goto":"前往", "Bookmarks":"书签", "Clear":"清除",
            "Goto: Lines: Clear":"清除行", "Goto: Bookmarks: Plain":"Goto: Bookmarks: Plain"
        }}"#,
            )
            .unwrap(),
        );
        let mut tr = Translator::default();
        tr.replace(catalogs);
        tr.select(tag);
        assert_eq!(
            tr.palette_caption("Goto: Bookmarks: Clear").as_deref(),
            Some("前往：书签：清除")
        );
        for missing in [
            "Other: Bookmarks: Clear",
            "Goto: Scope: Clear",
            "Goto: Bookmarks: Clear All",
        ] {
            assert_eq!(tr.palette_caption(missing), None, "{missing}");
        }
        assert_eq!(
            tr.palette_caption("Goto: Lines: Clear").as_deref(),
            Some("清除行")
        );
        assert_eq!(
            tr.palette_caption("Goto: Bookmarks: Plain").as_deref(),
            Some("Goto: Bookmarks: Plain")
        );
    }
}
