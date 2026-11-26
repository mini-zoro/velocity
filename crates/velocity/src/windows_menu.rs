//! Windows menu roles applied after resource merging and localization.
use vel_config::keymap::Keymap;
use vel_i18n::Translator;
use vel_platform::MenuEntry;

const ORDER: &[(&str, char)] = &[
    ("file", 'F'),
    ("edit", 'E'),
    ("selection", 'S'),
    ("find", 'I'),
    ("view", 'V'),
    ("goto", 'G'),
    ("tools", 'T'),
    ("project", 'P'),
    ("preferences", 'N'),
    ("help", 'H'),
];

pub(crate) fn arrange(entries: &mut Vec<MenuEntry>, tr: &Translator, keys: &Keymap) {
    let row = |caption: std::borrow::Cow<'static, str>, command: &str, mnemonic: char| MenuEntry {
        caption: caption.into_owned(),
        command: Some(command.into()),
        args: "{}".into(),
        shortcut: keys.menu_shortcut(command, &serde_json::json!({})),
        mnemonic: Some(mnemonic),
        ..Default::default()
    };
    let separator = || MenuEntry {
        caption: "-".into(),
        ..Default::default()
    };
    if let Some(at) = entries.iter().position(|entry| entry.id == "window") {
        let mut window = entries.remove(at);
        for child in &mut window.children {
            match child.command.as_deref() {
                Some("native_zoom") => child.caption = tr.tr("Maximize or Restore").into_owned(),
                Some("native_minimize") => child.shortcut = None,
                _ => {}
            }
        }
        window.children.extend([
            separator(),
            row(tr.tr("Next Window"), "next_window", 'N'),
            row(tr.tr("Previous Window"), "prev_window", 'P'),
        ]);
        if let Some(view) = entries.iter_mut().find(|entry| entry.id == "view") {
            view.children.extend([separator(), window]);
        }
    }
    if let Some(file) = entries.iter_mut().find(|entry| entry.id == "file") {
        file.children
            .extend([separator(), row(tr.tr("Exit"), "exit", 'X')]);
    }
    if let Some(help) = entries.iter_mut().find(|entry| entry.id == "help") {
        help.children.extend([
            separator(),
            row(tr.tr("Check for Updates…"), crate::updates::CHECK, 'U'),
            MenuEntry {
                id: crate::updates::ACTIONS.into(),
                caption: tr.tr("Update Actions…").into_owned(),
                children: crate::updates::menu(tr, None),
                ..Default::default()
            },
            row(tr.tr("About Velocity"), "native_about", 'A'),
        ]);
    }
    entries.sort_by_key(|entry| {
        if entry.id == "help" {
            ORDER.len()
        } else {
            ORDER
                .iter()
                .position(|(id, _)| *id == entry.id)
                .unwrap_or(ORDER.len() - 1)
        }
    });
    for entry in entries {
        if let Some((_, key)) = ORDER.iter().find(|(id, _)| *id == entry.id) {
            entry.mnemonic = Some(*key);
        }
    }
}

#[cfg(test)]
#[test]
fn native_roles_and_mnemonics_are_stable_in_both_languages() {
    use vel_i18n::{Catalog, Catalogs, Language};
    let mut catalog = Catalogs::default();
    catalog.insert(
        Language::parse("zh-Hans").unwrap(),
        Catalog::load(include_str!(
            "../resources/Packages/Default/Localization/zh-Hans.sublime-localization"
        ))
        .unwrap(),
    );
    let mut tr = Translator::default();
    tr.replace(catalog);
    let mut keys = Keymap::windows();
    keys.append(r#"[{"keys":["ctrl+q"],"command":"exit"}]"#)
        .unwrap();
    let menu = vel_config::ui::parse::<vel_config::ui::MenuItem>(include_str!(
        "../resources/Packages/Default/Main.sublime-menu"
    ))
    .unwrap();
    for (language, exit, about, update) in [
        ("en", "Exit", "About Velocity", "Check for Updates…"),
        ("zh-Hans", "退出", "关于 Velocity", "检查更新…"),
    ] {
        tr.select(Language::parse(language).unwrap());
        let mut entries = menu
            .iter()
            .map(|item| MenuEntry {
                id: item.id.clone(),
                caption: tr.caption(&item.caption).into(),
                ..Default::default()
            })
            .collect();
        arrange(&mut entries, &tr, &keys);
        assert_eq!(
            entries
                .iter()
                .map(|e| (e.id.as_str(), e.mnemonic.unwrap()))
                .collect::<Vec<_>>(),
            ORDER
        );
        let file_exit = entries[0].children.last().unwrap();
        assert_eq!(file_exit.caption, exit);
        assert_eq!(file_exit.shortcut.as_deref(), Some("ctrl+q"));
        let help = entries.last().unwrap();
        assert!(
            help.children
                .iter()
                .any(|row| row.command.as_deref() == Some(crate::updates::CHECK)
                    && row.caption == update)
        );
        assert_eq!(
            entries.last().unwrap().children.last().unwrap().caption,
            about
        );
    }
}
