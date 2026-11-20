//! Appearance quick panels share the ordinary input, keyboard and mouse routes.
use super::*;

#[derive(Debug)]
pub(super) struct Row {
    caption: UiText,
    key: &'static str,
    value: String,
    path: String,
}
#[derive(Debug)]
pub(super) struct SelectionPanel {
    rows: Vec<Row>,
    candidates: Vec<fuzzy::Candidate>,
    revision: u64,
    sheet: u64,
    root: &'static str,
}
impl Panels {
    pub(super) fn open_appearance(&mut self, workspace: &mut Workspace, theme: bool) {
        self.cancel_overlay(workspace);
        let root = if theme { "theme" } else { "color_scheme" };
        let current = workspace.editor().settings.string(root, "auto").to_owned();
        let resources = if theme {
            &workspace.appearance.themes
        } else {
            &workspace.appearance.schemes
        };
        let mut rows = vec![Row {
            caption: UiText::tr("Automatic (follow system appearance)"),
            key: root,
            value: "auto".into(),
            path: String::new(),
        }];
        for resource in resources {
            let name = resource
                .value
                .rsplit('/')
                .next()
                .unwrap_or(&resource.value)
                .trim_end_matches(if theme {
                    ".sublime-theme"
                } else if resource.value.ends_with(".tmTheme") {
                    ".tmTheme"
                } else {
                    ".sublime-color-scheme"
                });
            rows.push(Row {
                caption: name.into(),
                key: root,
                value: resource.value.clone(),
                path: resource.path.clone(),
            });
            if current == "auto" {
                for (prefix, key) in if theme {
                    [("Dark", "dark_theme"), ("Light", "light_theme")]
                } else {
                    [
                        ("Dark", "dark_color_scheme"),
                        ("Light", "light_color_scheme"),
                    ]
                } {
                    rows.push(Row {
                        caption: UiText::tr_fmt(
                            if prefix == "Dark" {
                                "Dark: {name} (applies in Dark mode)"
                            } else {
                                "Light: {name} (applies in Light mode)"
                            },
                            [("name", name.into())],
                        ),
                        key,
                        value: resource.value.clone(),
                        path: resource.path.clone(),
                    });
                }
            }
        }
        let selected = rows
            .iter()
            .position(|row| {
                row.key == root
                    && (row.value == current
                        || (current != "auto"
                            && row.value.rsplit('/').next() == current.rsplit('/').next()))
            })
            .unwrap_or(0);
        for row in &mut rows {
            row.caption.refresh(&workspace.translations);
        }
        self.appearance_selection = Some(SelectionPanel {
            candidates: rows
                .iter()
                .map(|r| fuzzy::Candidate::new(r.caption.as_str()))
                .collect(),
            rows,
            revision: workspace.resource_revision,
            sheet: workspace.tabs[workspace.active].sheet,
            root,
        });
        self.overlay = Some(Overlay::Appearance);
        self.fuzzy_worker.cancel();
        self.quick_pending = false;
        self.quick_input.set("");
        self.side_focused = false;
        self.focus.show(Surface::Overlay, true);
        self.focus.set(Surface::Overlay, 1);
        workspace.begin_appearance_preview();
        self.query_appearance(workspace, false);
        self.quick.selected = self
            .quick
            .items
            .iter()
            .position(|r| r.value == selected)
            .unwrap_or(0);
        self.quick.scroll.reveal(selected);
        // Opening the panel leaves the existing value exactly intact; preview starts
        // only when the user changes the selection.
    }
    pub(super) fn query_appearance(&mut self, workspace: &mut Workspace, preview: bool) {
        let Some(panel) = &self.appearance_selection else {
            return;
        };
        let query = self.quick_input.value();
        let hits = if query.is_empty() {
            (0..panel.rows.len())
                .map(|index| fuzzy::Hit {
                    index,
                    score: 0,
                    positions: Vec::new(),
                })
                .collect()
        } else {
            fuzzy::Matcher::default().rank(&panel.candidates, &query, panel.rows.len())
        };
        self.quick.set_items(
            hits.into_iter()
                .map(|hit| {
                    let row = &panel.rows[hit.index];
                    Item {
                        caption: row.caption.to_string(),
                        value: hit.index,
                        icon: vel_ui::ItemIcon::Settings,
                        annotation: row.key.into(),
                        details: row.path.clone(),
                        highlights: hit.positions,
                    }
                })
                .collect(),
        );
        if preview {
            self.preview_appearance(workspace);
        }
    }
    pub(super) fn refresh_appearance_locale(&mut self, workspace: &mut Workspace) {
        let Some(panel) = self.appearance_selection.as_mut() else {
            return;
        };
        let mut changed = false;
        for row in &mut panel.rows {
            changed |= row.caption.refresh(&workspace.translations);
        }
        if changed {
            let selected = self.quick.current();
            panel.candidates = panel
                .rows
                .iter()
                .map(|row| fuzzy::Candidate::new(row.caption.as_str()))
                .collect();
            self.query_appearance(workspace, false);
            if let Some(index) = self
                .quick
                .items
                .iter()
                .position(|item| Some(item.value) == selected)
            {
                self.quick.selected = index;
                self.quick.scroll.reveal(index);
            }
        }
    }
    pub(super) fn preview_appearance(&mut self, workspace: &mut Workspace) {
        let Some(panel) = &self.appearance_selection else {
            return;
        };
        let Some(row) = self.quick.current().and_then(|i| panel.rows.get(i)) else {
            return;
        };
        if let Err(error) = workspace.preview_appearance(row.key, &row.value) {
            self.set_message(error);
        }
    }
    pub(super) fn accept_appearance(&mut self, workspace: &mut Workspace) -> Result<(), String> {
        let Some(panel) = &self.appearance_selection else {
            return Ok(());
        };
        let Some(row) = self.quick.current().and_then(|i| panel.rows.get(i)) else {
            return Ok(());
        };
        // Validate the selected row again: Enter on a broken row neither writes nor closes.
        if let Err(error) = workspace.preview_appearance(row.key, &row.value) {
            self.set_message(error);
            return Ok(());
        }
        if let Err(error) = workspace.commit_appearance(row.key, &row.value) {
            workspace.cancel_appearance_preview();
            self.set_message(format!("could not save Preferences: {error}"));
        }
        self.appearance_selection = None;
        self.overlay = None;
        self.focus.show(Surface::Overlay, false);
        Ok(())
    }
    pub(super) fn appearance_stale(&mut self, workspace: &Workspace) -> bool {
        let Some(panel) = &mut self.appearance_selection else {
            return false;
        };
        if panel.sheet != workspace.tabs[workspace.active].sheet {
            return true;
        }
        if panel.revision == workspace.resource_revision {
            return false;
        }
        let resources = if panel.root == "theme" {
            &workspace.appearance.themes
        } else {
            &workspace.appearance.schemes
        };
        // A preference reload (including a language change) need not close a
        // picker whose underlying resources still have the same identities.
        let same_resources = panel
            .rows
            .iter()
            .filter(|row| row.key == panel.root && !row.path.is_empty())
            .map(|row| (&row.path, &row.value))
            .eq(resources
                .iter()
                .map(|resource| (&resource.path, &resource.value)));
        panel.revision = workspace.resource_revision;
        !same_resources
    }
}
