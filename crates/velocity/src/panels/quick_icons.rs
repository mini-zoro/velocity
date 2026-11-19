//! Quick-panel identity and painting, separate from caption matching and layout.
use super::{Path, Rect, Workspace};
use vel_config::ui::PaletteItem;
use vel_render::scene::{Color, Layer};
use vel_ui::{ItemIcon, SymbolBadge, theme::ThemeUi};

pub(super) fn command(item: &PaletteItem) -> ItemIcon {
    use ItemIcon::*;
    let name = item.invocation.command.as_str();
    match name {
        "set_file_type" | "show_syntax" => return Syntax,
        "insert_snippet" | "show_snippets" | "clear_snippet_fields" | "new_snippet" => {
            return Snippet;
        }
        "toggle_bookmark"
        | "next_bookmark"
        | "prev_bookmark"
        | "clear_bookmarks"
        | "select_all_bookmarks" => return Bookmarks,
        "toggle_minimap" | "toggle_word_wrap" => return View,
        "build" | "set_build_system" | "show_build_systems" | "cancel_build"
        | "new_build_system" | "run_macro_file" => return Build,
        "set_ui_language" | "set_indentation" | "set_encoding" | "show_dictionary"
        | "set_dictionary" => return Settings,
        "open_project" => return Project,
        "show_overlay" if item.invocation.args["overlay"] == "syntax" => return Syntax,
        "show_overlay" if item.invocation.args["overlay"] == "goto" => return Goto,
        _ if name.starts_with("package_") => return Package,
        "edit_settings" | "edit_syntax_settings" => return Settings,
        _ => {}
    }
    // Only the resource caption is considered here, never a translation or the
    // label that won matching. The prefix is also how packages group commands.
    let group = item.caption.split(": ").next().unwrap_or_default();
    match group {
        "View" | "Window" => View,
        "Selection" => Selection,
        "Goto" | "Jump Back" | "Jump Forward" => Goto,
        "Find" => Find,
        "Project" => Project,
        "Tools" | "Build" => Build,
        "Package" | "Packages" | "Package Control" => Package,
        "Preferences" | "Settings" | "Theme" => Settings,
        "Syntax" | "Set Syntax" => Syntax,
        "Snippet" => Snippet,
        "Bookmarks" => Bookmarks,
        _ if group.starts_with("Goto ") => Goto,
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
    fn assembled_palette_uses_twelve_registered_families() {
        use ItemIcon::*;
        let workspace = workspace();
        let expected: BTreeSet<_> = [
            Neutral, View, Selection, Goto, Find, Project, Build, Package, Settings, Syntax,
            Snippet, Bookmarks,
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
