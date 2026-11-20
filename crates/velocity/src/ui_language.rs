//! Resource-backed language picker rows, shared by menus and the palette.
use vel_config::{
    keymap::Invocation,
    ui::{MenuItem, PaletteItem},
};
use vel_i18n::{Catalogs, Translator, format_named};

pub(crate) const SHIPPED: &[(&str, &[u8])] =
    include!(concat!(env!("OUT_DIR"), "/localizations.rs"));

#[cfg(test)]
pub(crate) fn shipped_catalogs() -> impl Iterator<Item = (vel_i18n::Language, vel_i18n::Catalog)> {
    SHIPPED.iter().map(|&(path, bytes)| {
        let file = path.rsplit('/').next().unwrap();
        let name = file.strip_suffix(".sublime-localization").unwrap();
        let tag = vel_i18n::Language::parse(name).unwrap_or_else(|| panic!("{path}: bad tag"));
        assert!(
            !tag.is_english(),
            "{path}: English is the source, not a catalog"
        );
        let catalog = vel_i18n::Catalog::load(std::str::from_utf8(bytes).unwrap())
            .unwrap_or_else(|error| panic!("{path}: {error}"));
        (tag, catalog)
    })
}
const PALETTE_ROW: &str = "Preferences: Language: {language}";
const COMMAND: &str = "set_ui_language";

pub(crate) fn menu_rows(menus: &mut [MenuItem], catalogs: &Catalogs) {
    let Some(menu) = menus
        .iter_mut()
        .find(|item| item.id == "preferences")
        .and_then(|item| item.children.iter_mut().find(|item| item.id == "language"))
    else {
        return;
    };
    menu.children.splice(
        0..0,
        choices(catalogs).map(|(tag, native, _)| MenuItem {
            caption: native.to_owned(),
            id: String::new(),
            command: Some(COMMAND.to_owned()),
            shortcut: None,
            args: serde_json::json!({"language": tag}),
            children: Vec::new(),
            platform: None,
            mnemonic: None,
        }),
    );
}

pub(crate) fn palette_rows(rows: &mut Vec<PaletteItem>, catalogs: &Catalogs) {
    if let Some(at) = rows.iter().position(|item| {
        item.invocation.command == COMMAND
            && item.caption == PALETTE_ROW
            && item.invocation.args["language"].is_null()
    }) {
        rows.splice(
            at..=at,
            choices(catalogs).map(|(tag, _, english)| PaletteItem {
                caption: format_named(PALETTE_ROW, &[("language", english)]),
                invocation: Invocation {
                    command: COMMAND.to_owned(),
                    args: serde_json::json!({"language": tag}),
                },
            }),
        );
    }
}

fn choices(catalogs: &Catalogs) -> impl Iterator<Item = (&str, &str, &str)> {
    std::iter::once(("en", "English", "English"))
        .chain(catalogs.iter().map(|(tag, catalog)| {
            (
                tag.as_str(),
                catalog.language_name.as_str(),
                catalog
                    .english_name
                    .as_deref()
                    .unwrap_or(&catalog.language_name),
            )
        }))
        .chain(std::iter::once(("auto", "Follow System", "Follow System")))
}

pub(crate) fn palette_caption(tr: &Translator, item: &PaletteItem) -> Option<String> {
    let tag = item.invocation.args["language"].as_str()?;
    let native = match tag {
        "en" => "English",
        "auto" => tr.caption("Follow System"),
        _ => &tr.catalog(tag)?.language_name,
    };
    let template = tr.caption(PALETTE_ROW);
    (template != PALETTE_ROW).then(|| format_named(template, &[("language", native)]))
}
