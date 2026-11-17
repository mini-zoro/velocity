//! Compiled appearance choices and reversible, window-local previews.
use super::Workspace;
use std::{collections::BTreeMap, sync::Arc};
use vel_color_scheme::ColorScheme;
use vel_config::{Value, resources::Resources, settings::Layer};

pub(crate) const KEYS: [&str; 6] = [
    "color_scheme",
    "dark_color_scheme",
    "light_color_scheme",
    "theme",
    "dark_theme",
    "light_theme",
];
#[derive(Clone, Debug)]
pub(crate) struct Resource {
    pub path: String,
    pub value: String,
}
#[derive(Debug, Default)]
pub(crate) struct Catalog {
    pub schemes: Vec<Resource>,
    pub themes: Vec<Resource>,
    // Keys are resource paths. Values share the loader's already compiled schemes;
    // moving through the panel never parses JSON, scans packages or changes syntaxes.
    compiled: BTreeMap<String, Result<Arc<ColorScheme>, String>>,
}
impl Catalog {
    pub fn load(
        resources: &Resources,
        schemes: &BTreeMap<String, Arc<ColorScheme>>,
        errors: &BTreeMap<String, String>,
    ) -> Self {
        let mut catalog = Self::default();
        // Public formats: https://www.sublimetext.com/docs/color_schemes.html
        // and https://www.sublimetext.com/docs/themes.html. Hidden formats remain loadable
        // by settings, but are not offered as standalone choices.
        for suffix in [".sublime-color-scheme", ".tmTheme"] {
            for resource in resources.matching(suffix) {
                let name = resource.path.rsplit('/').next().unwrap_or(&resource.path);
                let compiled = errors
                    .get(name)
                    .map(|error| format!("{}: {error}", resource.path))
                    .map_or_else(
                        || {
                            schemes
                                .get(name)
                                .cloned()
                                .ok_or_else(|| format!("Unable to load {}", resource.path))
                        },
                        Err,
                    );
                if compiled.as_ref().is_ok_and(|scheme| scheme.hidden) {
                    continue;
                }
                catalog.compiled.insert(resource.path.clone(), compiled);
                // JSON schemes merge by filename, unlike legacy tmTheme paths:
                // https://www.sublimetext.com/docs/color_schemes.html#customization
                let value = if suffix == ".tmTheme" {
                    resource.path.clone()
                } else {
                    name.into()
                };
                if catalog
                    .schemes
                    .iter()
                    .any(|r| r.path.rsplit('/').next() == Some(name))
                {
                    continue;
                }
                catalog.schemes.push(Resource {
                    path: resource.path.clone(),
                    value,
                });
            }
        }
        for resource in resources.matching(".sublime-theme") {
            let name = resource.path.rsplit('/').next().unwrap_or(&resource.path);
            if !catalog.themes.iter().any(|r| r.value == name) {
                catalog.themes.push(Resource {
                    path: resource.path.clone(),
                    value: name.into(),
                });
            }
        }
        catalog
    }
    fn scheme(&self, value: &str) -> Option<Result<Arc<ColorScheme>, String>> {
        self.compiled.get(value).cloned().or_else(|| {
            self.compiled
                .iter()
                .find(|(path, _)| path.rsplit('/').next() == Some(value))
                .map(|(_, scheme)| scheme.clone())
        })
    }
}
#[derive(Debug)]
struct Original {
    sheet: u64,
    // Preview the User layer, so Syntax and real View overrides have exactly the
    // same precedence as the eventual saved setting. No real View value is touched.
    values: Vec<(&'static str, Option<Value>)>,
}
#[derive(Debug)]
pub(crate) struct Preview {
    originals: Vec<Original>,
    applied: Option<(&'static str, Value)>,
    revision: u64,
}
#[derive(Debug)]
pub(crate) struct PendingCommit {
    result: std::sync::mpsc::Receiver<Result<(), String>>,
    changes: Vec<(&'static str, Value)>,
    previous: Vec<(&'static str, Option<Value>)>,
    originals: Vec<Original>,
}
impl Workspace {
    fn visible_appearance_indices(&self) -> Vec<usize> {
        self.workbench
            .groups
            .iter()
            .flat_map(|group| &group.selected)
            .filter_map(|sheet| self.sheet_index(*sheet))
            .collect()
    }
    fn appearance_original(tab: &super::Tab) -> Original {
        Original {
            sheet: tab.sheet,
            values: KEYS
                .iter()
                .map(|&key| {
                    (
                        key,
                        tab.editor.settings.layer(Layer::User).get(key).cloned(),
                    )
                })
                .collect(),
        }
    }
    pub(crate) fn begin_appearance_preview(&mut self) {
        self.cancel_appearance_preview();
        self.appearance_preview = Some(Preview {
            applied: None,
            revision: self.resource_revision,
            originals: self
                .visible_appearance_indices()
                .into_iter()
                .map(|i| Self::appearance_original(&self.tabs[i]))
                .collect(),
        });
    }
    fn restore_appearance_values(&mut self, preview: &Preview) {
        // A resource publication already replaced the temporary User layers. It
        // owns the newly loaded values, so dismissal must not overwrite them.
        if preview.revision != self.resource_revision {
            return;
        }
        let Some((key, applied)) = &preview.applied else {
            return;
        };
        for original in &preview.originals {
            let Some(index) = self.sheet_index(original.sheet) else {
                continue;
            };
            let tab = &mut self.tabs[index];
            if tab.editor.settings.layer(Layer::User).get(*key) != Some(applied) {
                continue;
            }
            let mut values = tab.editor.settings.layer(Layer::User).clone();
            let original = original
                .values
                .iter()
                .find(|(k, _)| k == key)
                .and_then(|(_, value)| value.clone());
            if let Some(value) = original {
                values.insert((*key).into(), value);
            } else {
                values.remove(*key);
            }
            let ready = tab.language_settings_revision == tab.editor.settings.revision();
            tab.editor
                .settings
                .replace(Layer::User, Value::Object(values))
                .expect("object");
            if ready {
                tab.language_settings_revision = tab.editor.settings.revision();
            }
            // A tab may have become hidden before the picker dismissal arrives.
            // Restore its settings now and refresh its cached colors on activation.
            tab.appearance_revision = self.appearance_revision.wrapping_sub(1);
        }
    }
    fn set_appearance_value(tab: &mut super::Tab, key: &str, value: Value) {
        let ready = tab.language_settings_revision == tab.editor.settings.revision();
        tab.editor.settings.set(Layer::User, key, value);
        if ready {
            tab.language_settings_revision = tab.editor.settings.revision();
        }
    }
    fn recolor_appearance_at(&mut self, index: usize) {
        let tab = &mut self.tabs[index];
        let values = Value::Object(tab.editor.settings.effective());
        let selected = vel_color_scheme::selected_scheme(&values, self.dark);
        let name = selected.rsplit('/').next().unwrap_or(selected);
        let scheme = self
            .appearance
            .scheme(selected)
            .and_then(Result::ok)
            .or_else(|| self.schemes.get(name).cloned());
        if let Some(scheme) = scheme
            && let Some(language) = tab.editor.language.borrow_mut().as_mut()
            && !Arc::ptr_eq(&scheme, &language.scheme)
        {
            language.recolor(
                scheme,
                tab.editor.text.clone(),
                tab.editor.preferences.tab_size,
            );
        }
        tab.appearance_revision = self.appearance_revision;
        // Appearance-only changes do not reconfigure parsing, indentation or snippets.
        // set_appearance_value preserves any unrelated pending settings reconfiguration.
    }
    pub(super) fn recolor_appearance(&mut self) -> Result<(), String> {
        for index in self.visible_appearance_indices() {
            self.recolor_appearance_at(index);
        }
        let settings = self.editor().settings.clone();
        self.themes.select(&settings, self.dark)
    }
    pub(crate) fn refresh_appearance(&mut self, index: usize) {
        if self.tabs[index].appearance_revision != self.appearance_revision {
            let values = Value::Object(self.settings.layer(Layer::User).clone());
            let ready = self.tabs[index].language_settings_revision
                == self.tabs[index].editor.settings.revision();
            self.tabs[index]
                .editor
                .settings
                .replace(Layer::User, values)
                .expect("object");
            if ready {
                self.tabs[index].language_settings_revision =
                    self.tabs[index].editor.settings.revision();
            }
            self.tabs[index].appearance_revision = self.appearance_revision;
            self.recolor_appearance_at(index);
        }
    }
    pub(crate) fn preview_appearance(
        &mut self,
        key: &'static str,
        value: &str,
    ) -> Result<(), String> {
        if key.ends_with("color_scheme") && value != "auto" {
            self.appearance
                .scheme(value)
                .ok_or_else(|| format!("Unknown color scheme {value}"))??;
        } else if key.ends_with("theme") {
            let settings = self.editor().settings.clone();
            self.themes
                .validate_choice(
                    value,
                    &settings,
                    if key.starts_with("dark_") {
                        true
                    } else if key.starts_with("light_") {
                        false
                    } else {
                        self.dark
                    },
                )
                .map_err(|error| format!("{value}: {error}"))?;
        }
        let Some(mut preview) = self.appearance_preview.take() else {
            return Ok(());
        };
        self.restore_appearance_values(&preview);
        for original in &preview.originals {
            if let Some(index) = self.sheet_index(original.sheet) {
                Self::set_appearance_value(&mut self.tabs[index], key, Value::String(value.into()));
            }
        }
        preview.applied = Some((key, Value::String(value.into())));
        self.appearance_preview = Some(preview);
        self.recolor_appearance()
    }
    pub(crate) fn cancel_appearance_preview(&mut self) {
        let Some(preview) = self.appearance_preview.take() else {
            return;
        };
        self.restore_appearance_values(&preview);
        // Exact original values, including auto and all dark/light variants. Auto
        // resolves against the current system appearance when the panel is dismissed.
        let _ = self.recolor_appearance();
    }
    pub(crate) fn commit_appearance(
        &mut self,
        key: &'static str,
        value: &str,
    ) -> Result<(), String> {
        // https://www.sublimetext.com/docs/settings.html#settings-files documents
        // User/Syntax/View precedence. ST4 4107 preserves comments when editing preferences:
        // https://www.sublimetext.com/download. One serial atomic write per Enter.
        let root = self
            .data_dir
            .clone()
            .ok_or("User preferences directory unavailable")?;
        let mut changes = Vec::new();
        if key.starts_with("dark_") || key.starts_with("light_") {
            changes.push((
                if key.ends_with("theme") {
                    "theme"
                } else {
                    "color_scheme"
                },
                Value::String("auto".into()),
            ));
        }
        changes.push((key, Value::String(value.into())));
        let write_changes = changes.clone();
        #[cfg(test)]
        let writes = self.appearance_writes.clone();
        // The workspace's settings-write tracker keeps these pairs over reloads that
        // race the queue. It only needs completion: failures are reported (non-modally)
        // and rolled back by `poll_appearance_commits`, so the tracker always sees Ok.
        let (tracked_tx, tracked_rx) = std::sync::mpsc::channel();
        let result = crate::release::enqueue(move || {
            let outcome = (|| {
                let path = root.join("Packages/User/Preferences.sublime-settings");
                let mut source = match std::fs::read_to_string(&path) {
                    Ok(source) => source,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => "{\n}\n".into(),
                    Err(error) => return Err(format!("{}: {error}", path.display())),
                };
                for (key, value) in write_changes {
                    source = vel_config::json::Document::parse(&source)
                        .and_then(|document| document.set(key, &value))
                        .map_err(|error| format!("{}: {error}", path.display()))?;
                }
                vel_session::atomic_write(&path, |file| {
                    use std::io::Write;
                    file.write_all(source.as_bytes()).map_err(|e| e.to_string())
                })?;
                #[cfg(test)]
                writes.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(())
            })();
            let _ = tracked_tx.send(Ok(()));
            outcome
        })?;
        let tracked: Vec<(String, Option<Value>)> = changes
            .iter()
            .map(|(key, value)| ((*key).into(), Some(value.clone())))
            .collect();
        self.track_settings_write(tracked_rx, &tracked);
        // Promote the selected preview directly to committed memory. Do not
        // restore/recolor the original between Enter and the queued atomic write.
        let previous = changes
            .iter()
            .map(|(key, _)| (*key, self.settings.layer(Layer::User).get(*key).cloned()))
            .collect();
        let originals = self
            .appearance_preview
            .take()
            .map_or_else(Vec::new, |preview| preview.originals);
        for (key, value) in &changes {
            self.settings.set(Layer::User, key, value.clone());
        }
        self.apply_appearance_changes(&changes);
        self.appearance_commits.push(PendingCommit {
            result,
            changes,
            previous,
            originals,
        });
        Ok(())
    }
    fn apply_appearance_changes(&mut self, changes: &[(&str, Value)]) {
        for index in self.visible_appearance_indices() {
            for (key, value) in changes {
                Self::set_appearance_value(&mut self.tabs[index], key, value.clone());
            }
        }
        self.appearance_revision = self.appearance_revision.wrapping_add(1);
        for index in self.visible_appearance_indices() {
            self.tabs[index].appearance_revision = self.appearance_revision;
        }
        let _ = self.recolor_appearance();
    }
    fn rollback_appearance(
        &mut self,
        commit: &PendingCommit,
        newer: &std::collections::VecDeque<PendingCommit>,
    ) {
        for (key, value) in &commit.changes {
            if newer
                .iter()
                .any(|pending| pending.changes.iter().any(|(k, _)| k == key))
            {
                continue;
            }
            let previous = commit
                .previous
                .iter()
                .find(|(k, _)| k == key)
                .and_then(|(_, v)| v.clone());
            if self.settings.layer(Layer::User).get(*key) == Some(value) {
                let mut values = self.settings.layer(Layer::User).clone();
                if let Some(value) = &previous {
                    values.insert((*key).into(), value.clone());
                } else {
                    values.remove(*key);
                }
                self.settings
                    .replace(Layer::User, Value::Object(values))
                    .expect("object");
            }
            for tab in &mut self.tabs {
                if tab.editor.settings.layer(Layer::User).get(*key) != Some(value) {
                    continue;
                }
                let original = commit
                    .originals
                    .iter()
                    .find(|original| original.sheet == tab.sheet)
                    .and_then(|original| original.values.iter().find(|(k, _)| k == key))
                    .map(|(_, value)| value.clone())
                    .unwrap_or_else(|| previous.clone());
                let ready = tab.language_settings_revision == tab.editor.settings.revision();
                let mut values = tab.editor.settings.layer(Layer::User).clone();
                if let Some(value) = original {
                    values.insert((*key).into(), value);
                } else {
                    values.remove(*key);
                }
                tab.editor
                    .settings
                    .replace(Layer::User, Value::Object(values))
                    .expect("object");
                if ready {
                    tab.language_settings_revision = tab.editor.settings.revision();
                }
            }
        }
        self.appearance_revision = self.appearance_revision.wrapping_add(1);
        for index in self.visible_appearance_indices() {
            self.tabs[index].appearance_revision = self.appearance_revision;
        }
        let _ = self.recolor_appearance();
    }
    pub(crate) fn poll_appearance_commits(&mut self) -> bool {
        let mut changed = false;
        let mut pending =
            std::collections::VecDeque::from(std::mem::take(&mut self.appearance_commits));
        while let Some(commit) = pending.pop_front() {
            match commit.result.try_recv() {
                Err(std::sync::mpsc::TryRecvError::Empty) => self.appearance_commits.push(commit),
                result => {
                    changed = true;
                    let result =
                        result.unwrap_or_else(|_| Err("Preferences writer disconnected".into()));
                    if let Err(error) = result {
                        // Revoke failed overrides before rollback: the same poll may
                        // publish a resource load that raced the queued write.
                        let failed: Vec<_> = commit
                            .changes
                            .iter()
                            .filter(|(key, _)| {
                                !pending
                                    .iter()
                                    .any(|newer| newer.changes.iter().any(|(k, _)| k == key))
                            })
                            .cloned()
                            .collect();
                        self.discard_settings_write(&failed);
                        self.rollback_appearance(&commit, &pending);
                        self.appearance_notice =
                            Some(format!("could not save Preferences: {error}"));
                        continue;
                    }
                    // The queued write was registered with the settings-write tracker
                    // at commit time, which also requests the settled reload.
                    for (key, value) in &commit.changes {
                        if self.settings.layer(Layer::User).get(*key) == Some(value)
                            && self.editor().settings.get(key) != Some(value)
                        {
                            self.appearance_notice = Some(format!(
                                "Saved {key}; a syntax or view setting overrides it in this view"
                            ));
                        }
                    }
                }
            }
        }
        changed
    }
}
