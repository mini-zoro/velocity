//! File-backed Windows fonts, with collection members selected by PostScript name.
use super::{FontDescriptor, cluster_characters};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};
use swash::{FontDataRef, FontRef, StringId, Style};

#[derive(Debug)]
struct Entry {
    face: FontDescriptor,
    names: Vec<String>,
    families: Vec<String>,
    weight: u16,
    italic: bool,
}
impl Entry {
    fn score(&self, bold: bool, italic: bool) -> (bool, u16) {
        (
            self.italic != italic,
            self.weight.abs_diff(if bold { 700 } else { 400 }),
        )
    }
    fn covers(&self, text: &str) -> bool {
        let Ok(data) = fs::read(&self.face.path) else {
            return false;
        };
        FontDataRef::new(&data).is_some_and(|collection| {
            collection.fonts().any(|font| {
                postscript(font).as_deref() == Some(&self.face.postscript_name)
                    && cluster_characters(text).all(|c| font.charmap().map(c) != 0)
            })
        })
    }
}

fn postscript(font: FontRef<'_>) -> Option<String> {
    font.localized_strings()
        .find(|name| name.id() == StringId::PostScript)
        .map(|name| name.to_string())
}

fn read_faces(path: &Path) -> Vec<Entry> {
    if !path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            ["ttf", "otf", "ttc", "otc"]
                .iter()
                .any(|valid| ext.eq_ignore_ascii_case(valid))
        })
        || !fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() <= 128 * 1024 * 1024)
    {
        return vec![];
    }
    let Ok(data) = fs::read(path) else {
        return vec![];
    };
    let Some(collection) = FontDataRef::new(&data) else {
        return vec![];
    };
    collection
        .fonts()
        .filter_map(|font| {
            let postscript = postscript(font)?;
            let mut names = vec![postscript.to_lowercase()];
            let mut families = vec![];
            for name in font.localized_strings() {
                match name.id() {
                    StringId::Full => names.push(name.to_string().to_lowercase()),
                    StringId::Family | StringId::TypographicFamily => {
                        families.push(name.to_string())
                    }
                    _ => {}
                }
            }
            let attrs = font.attributes();
            Some(Entry {
                face: FontDescriptor::file(path.to_owned(), postscript),
                names,
                families,
                weight: attrs.weight().0,
                italic: attrs.style() != Style::Normal,
            })
        })
        .collect()
}

fn database() -> &'static [Entry] {
    static DATABASE: OnceLock<Vec<Entry>> = OnceLock::new();
    DATABASE.get_or_init(|| {
        let user = std::env::var_os("LOCALAPPDATA")
            .map(|root| PathBuf::from(root).join("Microsoft/Windows/Fonts"));
        let system = std::env::var_os("SystemRoot")
            .or_else(|| std::env::var_os("WINDIR"))
            .map(|root| PathBuf::from(root).join("Fonts"));
        user.into_iter()
            .chain(system)
            .filter_map(|dir| fs::read_dir(dir).ok())
            .flat_map(|dir| {
                let mut paths: Vec<_> = dir.flatten().map(|entry| entry.path()).collect();
                paths.sort_unstable();
                paths
            })
            .flat_map(|path| read_faces(&path))
            .collect()
    })
}

fn family_face(name: &str) -> Option<&'static Entry> {
    let name = name.to_lowercase();
    database()
        .iter()
        .filter(|entry| {
            entry.names.contains(&name)
                || entry
                    .families
                    .iter()
                    .any(|family| family.to_lowercase() == name)
        })
        .min_by_key(|entry| (!entry.names.contains(&name), entry.score(false, false)))
}

/// Discover an installed Windows face from its family, full name, or PostScript name.
pub fn font_named(name: &str) -> Option<FontDescriptor> {
    let alias = match name {
        "system" | "system-ui" | "sans-serif" | ".AppleSystemUIFont" => "Segoe UI",
        "monospace" | "Menlo" => "Consolas",
        "serif" => "Times New Roman",
        _ => name,
    };
    family_face(name)
        .or_else(|| family_face(alias))
        .map(|entry| entry.face.clone())
}

/// Family names for the font chooser, using the same catalog as rendering.
pub fn font_families() -> Vec<String> {
    database()
        .iter()
        .flat_map(|entry| &entry.families)
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Discover the closest available weight and slope within the selected face's family.
pub fn font_style(face: &FontDescriptor, bold: bool, italic: bool) -> Option<FontDescriptor> {
    let base = database().iter().find(|entry| {
        entry.face.path == face.path && entry.face.postscript_name == face.postscript_name
    })?;
    database()
        .iter()
        .filter(|entry| {
            entry
                .families
                .iter()
                .any(|name| base.families.contains(name))
        })
        .min_by_key(|entry| entry.score(bold, italic))
        .map(|entry| entry.face.clone())
}

pub(super) fn fallback(primary: &str, text: &str) -> Option<FontDescriptor> {
    let emoji = primary.eq_ignore_ascii_case("Segoe UI Emoji");
    let candidates = if emoji {
        [primary, "Segoe UI Symbol", "Microsoft YaHei", "Segoe UI"]
    } else {
        [
            primary,
            "Microsoft YaHei",
            "Segoe UI Symbol",
            "Segoe UI Emoji",
        ]
    };
    candidates
        .into_iter()
        .filter_map(family_face)
        .find(|entry| entry.covers(text))
        .or_else(|| {
            database()
                .iter()
                .find(|entry| !entry.italic && entry.covers(text))
        })
        .map(|entry| entry.face.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_server_has_a_monospace_and_system_cascade() {
        assert!(font_named("Consolas").unwrap().path.is_file());
        assert!(font_named("system-ui").unwrap().path.is_file());
        // The East Asian language packs are optional on Windows Server.
        if font_named("Microsoft YaHei").is_some() {
            assert!(fallback("Consolas", "\u{4f60}").is_some());
        }
    }
}
