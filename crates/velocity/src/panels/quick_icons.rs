//! Quick-panel identity and painting, separate from caption matching and layout.
use super::{Path, Rect, Workspace};
use vel_config::ui::PaletteItem;
use vel_render::scene::{Color, Layer};
use vel_ui::{ItemIcon, SymbolBadge, theme::ThemeUi};

pub(super) fn command(item: &PaletteItem) -> ItemIcon {
    use ItemIcon::*;
    let name = item.invocation.command.as_str();
    // Use the original caption, never a translation or the winning match.
    // Some Edit actions use the snippet dispatcher but are not snippet picks.
    let group = item.caption.split(": ").next().unwrap_or_default();
    match group {
        "File" => return FileCommand,
        "Edit" => return Edit,
        "Developer" => return Build,
        _ => {}
    }
    match name {
        "save" | "save_all" | "revert" | "new_file" => return FileCommand,
        "undo"
        | "redo"
        | "cut"
        | "copy"
        | "toggle_comment"
        | "indent"
        | "unindent"
        | "duplicate_line"
        | "join_lines"
        | "wrap_lines"
        | "close_tag"
        | "upper_case"
        | "lower_case"
        | "title_case"
        | "swap_case"
        | "sort_lines"
        | "reindent"
        | "auto_complete"
        | "revert_modification" => return Edit,
        _ if name.starts_with("paste") || name.starts_with("swap_line") => return Edit,
        "set_file_type" | "show_syntax" => return Syntax,
        "insert_snippet"
        | "show_snippets"
        | "clear_snippet_fields"
        | "clear_fields"
        | "new_snippet" => {
            return Snippet;
        }
        "toggle_bookmark"
        | "next_bookmark"
        | "prev_bookmark"
        | "clear_bookmarks"
        | "select_all_bookmarks" => return Bookmarks,
        "toggle_minimap"
        | "toggle_word_wrap"
        | "fold"
        | "fold_by_level"
        | "fold_tag_attributes"
        | "unfold"
        | "unfold_all"
        | "toggle_inline_diff" => return View,
        "toggle_setting"
            if matches!(
                item.invocation.args["setting"].as_str(),
                Some("word_wrap" | "spell_check")
            ) =>
        {
            return View;
        }
        "build"
        | "set_build_system"
        | "show_build_systems"
        | "cancel_build"
        | "new_build_system"
        | "run_macro_file"
        | "show_scope_name"
        | "show_indexing_status" => {
            return Build;
        }
        "set_ui_language" | "set_indentation" | "set_encoding" | "show_dictionary"
        | "set_dictionary" => return Settings,
        "next_modification" | "prev_modification" | "next_misspelling" | "prev_misspelling" => {
            return Goto;
        }
        "open_project" => return Project,
        "show_overlay" if item.invocation.args["overlay"] == "syntax" => return Syntax,
        "show_overlay" if item.invocation.args["overlay"] == "goto" => return Goto,
        _ if name.starts_with("package_") => return Package,
        "show_settings" | "edit_settings" | "edit_syntax_settings" => return Settings,
        _ => {}
    }
    match group {
        "View" | "Window" | "Large File" | "Spell Check" => View,
        "Selection" => Selection,
        "Goto" | "Jump Back" | "Jump Forward" | "Accessibility" => Goto,
        "Find" => Find,
        "Project" | "Git" => Project,
        "Tools" | "Build" | "Theme & minihtml" => Build,
        "Package" | "Packages" | "Package Control" => Package,
        "Preferences" | "Settings" | "Theme" | "UI" => Settings,
        "Syntax" | "Set Syntax" => Syntax,
        "Snippet" => Snippet,
        "Bookmarks" => Bookmarks,
        "Indentation" | "Hex" => Edit,
        _ if group.starts_with("Goto ") => Goto,
        _ if name.starts_with("prompt_open") || name.starts_with("close") => FileCommand,
        _ => Neutral,
    }
}

pub(super) fn symbol(kind: vel_index::SymbolKind) -> ItemIcon {
    use vel_index::SymbolKind::*;
    ItemIcon::Symbol(match kind {
        Function => SymbolBadge::Function,
        Type => SymbolBadge::Type,
        Namespace => SymbolBadge::Namespace,
        Variable => SymbolBadge::Variable,
        Markup => SymbolBadge::Markup,
        Other => SymbolBadge::Other,
    })
}

pub(super) fn symbol_style(badge: SymbolBadge) -> (&'static str, &'static str) {
    match badge {
        SymbolBadge::Function => ("kind_container kind_function", "f"),
        SymbolBadge::Type => ("kind_container kind_type", "t"),
        SymbolBadge::Namespace => ("kind_container kind_namespace", "n"),
        SymbolBadge::Variable => ("kind_container kind_variable", "v"),
        SymbolBadge::Markup => ("kind_container kind_markup", "m"),
        SymbolBadge::Other => ("kind_container kind_navigation", "s"),
    }
}

fn texture(icon: ItemIcon) -> &'static str {
    use ItemIcon::*;
    macro_rules! asset {
        ($id:literal) => {
            concat!("Theme - Velocity/palette-", $id, ".png")
        };
    }
    match icon {
        FileCommand => asset!("file"),
        Edit => asset!("edit"),
        View => asset!("view"),
        Selection => asset!("selection"),
        Goto => asset!("goto"),
        Find => asset!("find"),
        Project => asset!("project"),
        Build => asset!("build"),
        Package => asset!("package"),
        Settings => asset!("settings"),
        Syntax => asset!("syntax"),
        Snippet => asset!("snippet"),
        Bookmarks => asset!("bookmarks"),
        Neutral | File | Symbol(_) => asset!("neutral"),
    }
}

pub(super) fn paint(
    workspace: &Workspace,
    theme: &ThemeUi,
    layer: &mut Layer,
    item: &vel_ui::Item,
    badge: Rect,
    clip: Rect,
    ink: Color,
) {
    let inset = badge.width / 18.;
    let rect = Rect {
        x: badge.x + inset,
        y: badge.y + inset,
        width: badge.width - 2. * inset,
        height: badge.height - 2. * inset,
    };
    let resource = if item.icon == ItemIcon::File {
        workspace.themes.file_icon(Path::new(&item.details))
    } else {
        Some(texture(item.icon))
    };
    paint_glyph(theme, layer, resource, rect, clip, ink);
}

fn paint_glyph(
    theme: &ThemeUi,
    layer: &mut Layer,
    resource: Option<&str>,
    rect: Rect,
    clip: Rect,
    ink: Color,
) {
    let neutral = texture(ItemIcon::Neutral);
    if let Some(resource) = resource
        && resource != neutral
        && theme.icon(layer, resource, rect, clip, ink)
    {
        return;
    }
    if !theme.icon(layer, neutral, rect, clip, ink) {
        // A partial theme load can lack even the neutral mask. Draw the same
        // three dots with primitives; don't use a font glyph or a row caption.
        let unit = rect.width / 16.;
        for x in [3.5, 7.5, 11.5] {
            vel_ui::fill(
                layer,
                Rect {
                    x: rect.x + x * unit,
                    y: rect.y + 7.5 * unit,
                    width: unit,
                    height: unit,
                },
                clip,
                ink,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::{Overlay, Panels};
    use std::collections::{BTreeSet, HashMap};
    use vel_platform::Size;
    use vel_render::scene::{ImageQuad, Scene, Surface};
    use vel_ui::Item;

    fn workspace() -> Workspace {
        Workspace::new(vel_text::Text::from("plain text"), None, None, false).unwrap()
    }

    fn image(layer: &Layer) -> Option<&ImageQuad> {
        layer.surfaces.iter().find_map(|surface| match surface {
            Surface::Image(glyph) => Some(glyph),
            _ => None,
        })
    }

    #[test]
    fn assembled_palette_uses_fourteen_registered_families() {
        use ItemIcon::*;
        let workspace = workspace();
        let expected: BTreeSet<_> = [
            Neutral,
            FileCommand,
            Edit,
            View,
            Selection,
            Goto,
            Find,
            Project,
            Build,
            Package,
            Settings,
            Syntax,
            Snippet,
            Bookmarks,
        ]
        .into_iter()
        .map(texture)
        .collect();
        let mut seen = BTreeSet::new();
        let mut captions = HashMap::new();
        for item in &workspace.palette {
            let icon = command(item);
            assert!(!matches!(icon, File | Symbol(_)));
            let id = texture(icon);
            assert!(expected.contains(id) && !id.is_empty(), "{item:?}");
            seen.insert(id);
            if let Some(previous) = captions.insert(&item.caption, icon) {
                assert_eq!(icon, previous, "{}", item.caption);
            }
        }
        // Neutral is a supported fallback even if no built-in row needs it.
        seen.insert(texture(Neutral));
        assert_eq!(seen, expected);
        let rect = Rect {
            width: 16.,
            height: 16.,
            ..Default::default()
        };
        let theme = workspace.themes.ui.borrow();
        let mut pixels = BTreeSet::new();
        for id in expected {
            let mut layer = Layer::default();
            assert!(theme.icon(&mut layer, id, rect, rect, [1.; 4]), "{id}");
            let glyph = image(&layer).expect("registered mask");
            assert!(pixels.insert(glyph.image.id), "duplicate mask {id}");
        }
    }

    #[test]
    fn file_edit_and_developer_icons_follow_caption_or_command() {
        use ItemIcon::{Build, Edit, FileCommand};
        let workspace = workspace();
        for (caption, expected) in [
            ("File: Revert File…", FileCommand),
            ("File: Save", FileCommand),
            ("File: Close All Files", FileCommand),
            ("Edit: Close Tag", Edit),
            ("Edit: Paste and Indent", Edit),
            ("Edit: Wrap Selection With Tag", Edit),
            ("Developer: Show Scope Name", Build),
        ] {
            let row = workspace
                .palette
                .iter()
                .find(|row| row.caption == caption)
                .expect(caption);
            assert_eq!(command(row), expected, "{caption}");
        }
        for (expected, names) in [
            (
                FileCommand,
                &[
                    "save",
                    "save_all",
                    "revert",
                    "close_all",
                    "new_file",
                    "prompt_open",
                    "prompt_open_file",
                    "close",
                    "close_others",
                ][..],
            ),
            (
                Edit,
                &[
                    "paste",
                    "paste_and_indent",
                    "undo",
                    "redo",
                    "cut",
                    "copy",
                    "toggle_comment",
                    "indent",
                    "unindent",
                    "swap_line_up",
                    "swap_line_down",
                    "duplicate_line",
                    "join_lines",
                    "wrap_lines",
                    "close_tag",
                    "upper_case",
                    "lower_case",
                    "title_case",
                    "swap_case",
                    "sort_lines",
                    "reindent",
                ],
            ),
            (Build, &["show_scope_name"][..]),
        ] {
            for &name in names {
                let row = PaletteItem {
                    caption: "An ungrouped caption".into(),
                    invocation: vel_config::keymap::Invocation {
                        command: name.into(),
                        args: Default::default(),
                    },
                };
                assert_eq!(command(&row), expected, "{name}");
            }
        }
    }

    #[test]
    fn assembled_english_palette_neutral_rows_are_intentional_and_under_budget() {
        let workspace = workspace();
        let mut neutral: Vec<_> = workspace
            .palette
            .iter()
            .filter(|row| command(row) == ItemIcon::Neutral)
            .map(|row| row.caption.as_str())
            .collect();
        neutral.sort_unstable();
        eprintln!(
            "Neutral English palette rows ({}/{}): {neutral:#?}",
            neutral.len(),
            workspace.palette.len()
        );
        assert!(neutral.len() * 10 <= workspace.palette.len(), "{neutral:?}");
        assert_eq!(
            neutral,
            [
                "Help: Release Notes",          // Opens a help page.
                "Help: Velocity Documentation", // Opens the external manual.
                "Install Command Line Tool…",   // App integration, not a package install.
                "Open Crash Reports",           // Opens the diagnostic log folder.
            ]
        );
    }

    #[test]
    fn missing_icon_falls_back_even_without_neutral_texture() {
        let workspace = workspace();
        let rect = Rect {
            width: 16.,
            height: 16.,
            ..Default::default()
        };
        let mut missing = Layer::default();
        let mut neutral = Layer::default();
        let theme = workspace.themes.ui.borrow();
        paint_glyph(
            &theme,
            &mut missing,
            Some("missing.png"),
            rect,
            rect,
            [1.; 4],
        );
        paint_glyph(&theme, &mut neutral, None, rect, rect, [1.; 4]);
        assert_eq!(
            image(&missing).unwrap().image.id,
            image(&neutral).unwrap().image.id
        );
        let mut dots = Layer::default();
        paint_glyph(&ThemeUi::default(), &mut dots, None, rect, rect, [1.; 4]);
        assert_eq!(dots.rects.len(), 3);
        assert!(dots.text.is_empty());
    }

    #[test]
    fn goto_file_uses_its_type_texture_and_symbols_keep_fixed_badges() {
        let workspace = workspace();
        let badge = Rect {
            width: 18.,
            height: 18.,
            ..Default::default()
        };
        let theme = workspace.themes.ui.borrow();
        let mut file = Layer::default();
        paint(
            &workspace,
            &theme,
            &mut file,
            &Item {
                details: "src/main.rs".into(),
                icon: ItemIcon::File,
                ..Default::default()
            },
            badge,
            badge,
            [1.; 4],
        );
        let mut code = Layer::default();
        assert!(theme.icon(
            &mut code,
            "Theme - Velocity/file-code.png",
            badge,
            badge,
            [1.; 4]
        ));
        assert_eq!(
            image(&file).unwrap().image.id,
            image(&code).unwrap().image.id
        );
        for (kind, expected) in [
            (
                vel_index::SymbolKind::Function,
                ("kind_container kind_function", "f"),
            ),
            (
                vel_index::SymbolKind::Type,
                ("kind_container kind_type", "t"),
            ),
            (
                vel_index::SymbolKind::Namespace,
                ("kind_container kind_namespace", "n"),
            ),
            (
                vel_index::SymbolKind::Variable,
                ("kind_container kind_variable", "v"),
            ),
            (
                vel_index::SymbolKind::Markup,
                ("kind_container kind_markup", "m"),
            ),
            (
                vel_index::SymbolKind::Other,
                ("kind_container kind_navigation", "s"),
            ),
        ] {
            let ItemIcon::Symbol(badge) = symbol(kind) else {
                panic!("expected a symbol badge")
            };
            assert_eq!(symbol_style(badge), expected);
        }
    }

    #[test]
    fn palette_texture_stays_inside_badge_and_cjk_caption_is_fitted() {
        let workspace = workspace();
        let caption = "设置语法并更新正在编辑文件的显示选项".repeat(12);
        let id = (1 << 40) + 5;
        for scale in [1., 2.] {
            let mut panels = Panels::new(&workspace, false);
            panels.ui_scale = scale;
            panels.overlay = Some(Overlay::Palette);
            panels.quick.set_items(vec![Item {
                caption: caption.clone(),
                icon: ItemIcon::Settings,
                value: 1,
                ..Default::default()
            }]);
            panels.layout(Size {
                width: 440. * scale,
                height: 435. * scale,
            });
            let mut scene = Scene::default();
            panels.paint(&workspace, &mut scene);
            let row = panels.tree.nodes[&id].rect;
            assert!(panels.tree.nodes[&id].children.is_empty());
            assert_eq!(
                panels.tree.hit_test(vel_platform::Point {
                    x: row.x + row.width / 2.,
                    y: row.y + row.height / 2.,
                }),
                Some(id)
            );
            let ink = scene
                .layers
                .iter()
                .flat_map(|layer| &layer.text)
                .find(|run| run.text.starts_with("设置语法"))
                .expect("caption");
            let glyph = scene
                .layers
                .iter()
                .flat_map(|layer| &layer.surfaces)
                .filter_map(|surface| match surface {
                    Surface::Image(glyph) => Some(glyph),
                    _ => None,
                })
                .find(|glyph| glyph.rect.x == row.x + 11. * scale && glyph.rect.y >= row.y)
                .expect("palette glyph");
            assert_eq!(glyph.rect.width, 16. * scale);
            assert!(glyph.rect.y + glyph.rect.height <= row.y + row.height);
            assert!(f64::from(ink.x) >= glyph.rect.x + glyph.rect.width);
            assert!(ink.text.ends_with('…'), "{}", ink.text);
            assert!(ink.clip.x + ink.clip.width <= row.x + row.width);
        }
    }
}
