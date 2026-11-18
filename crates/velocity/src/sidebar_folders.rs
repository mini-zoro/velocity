use super::*;
use std::io::{Read, Write};
use vel_config::json::Document;
use vel_session::project::{Folder as SavedFolder, Project};

pub(super) enum Edit {
    Add(PathBuf),
    Reveal(PathBuf),
    Remove(PathBuf),
    Clear,
}
pub(super) struct Applied {
    edit: Edit,
    model: Option<Project>,
}
pub(super) type Pending = (Option<PathBuf>, mpsc::Receiver<Result<Applied, String>>);

fn apply(project: Option<&Path>, mut edit: Edit) -> Result<Applied, String> {
    if let Edit::Add(path) | Edit::Reveal(path) = &mut edit {
        *path = crate::workspace::absolute_folder(path)?;
        std::fs::read_dir(&*path).map_err(|e| vel_fileio::regular::error(path, e))?;
    }
    let model = project.filter(|_| !matches!(edit, Edit::Reveal(_)))
        .map(|project| {
            // Project folder data is persisted when a project exists:
            // https://www.sublimetext.com/docs/api_reference.html#sublime.Window.set_project_data
            // https://www.sublimetext.com/docs/projects.html#folders-key
            let mut source = String::new();
            vel_fileio::regular::read(project)?
                .read_to_string(&mut source)
                .map_err(|e| vel_fileio::regular::error(project, e))?;
            let model = Project::parse(&source)?;
            let mut document = Document::parse(&source)?;
            let base = crate::workspace::absolute_folder(project.parent().unwrap_or(Path::new(".")))?;
            let same = |left: &Path, right: &Path| {
                left == right || crate::workspace::absolute_folder(left).is_ok_and(|left| left == right)
            };
            match &edit {
                Edit::Add(path) => {
                    if !model
                        .folders
                        .iter()
                        .any(|entry| same(&base.join(&entry.path), path))
                    {
                        let relative = path
                            .strip_prefix(&base)
                            .ok()
                            .filter(|p| !p.as_os_str().is_empty());
                        let entry = SavedFolder {
                            path: relative.unwrap_or(path).to_owned(),
                            ..Default::default()
                        };
                        let value = serde_json::to_value(entry).map_err(|e| e.to_string())?;
                        document = Document::parse(&document.append_array("folders", &value)?)?;
                    }
                }
                Edit::Remove(_) | Edit::Clear => {
                    for (i, entry) in model.folders.iter().enumerate().rev() {
                        if matches!(&edit, Edit::Clear) || matches!(&edit, Edit::Remove(path) if same(&base.join(&entry.path), path)) {
                            document = Document::parse(&document.remove_array_element("folders", i)?)?;
                        }
                    }
                }
                Edit::Reveal(_) => unreachable!(),
            }
            if document.source() != source {
                vel_session::atomic_write(project, |file| file.write_all(document.source().as_bytes()).map_err(|e| e.to_string()))?;
            }
            Project::parse(document.source())
        })
        .transpose()?;
    Ok(Applied { edit, model })
}

impl Panels {
    pub(super) fn edit_folders(
        &mut self,
        workspace: &mut Workspace,
        edit: Edit,
    ) -> Result<(), String> {
        let project = workspace.project_path().map(Path::to_owned);
        if !self.review.native_context {
            // Headless construction has no AppKit event loop and callers can observe
            // the completed action immediately.
            let applied = apply(project.as_deref(), edit)?;
            return self.publish_folder_edit(workspace, applied);
        }
        if self.folder_edits.len() >= 32 {
            return Err("Too many pending folder changes".into());
        }
        let path = project.clone();
        let receiver = crate::release::enqueue(move || apply(path.as_deref(), edit))?;
        self.folder_edits.push((project, receiver));
        Ok(())
    }

    pub(super) fn poll_folder_edits(&mut self, workspace: &mut Workspace) -> bool {
        if self.folder_edits.is_empty() {
            return false;
        }
        let mut changed = false;
        for (project, receiver) in std::mem::take(&mut self.folder_edits) {
            let result = match receiver.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => {
                    self.folder_edits.push((project, receiver));
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => Err("Folder edit did not finish".into()),
            };
            if project.as_deref() == workspace.project_path() {
                if let Err(error) =
                    result.and_then(|applied| self.publish_folder_edit(workspace, applied))
                {
                    self.set_message(error);
                }
                changed = true;
            }
        }
        changed
    }

    fn publish_folder_edit(
        &mut self,
        workspace: &mut Workspace,
        applied: Applied,
    ) -> Result<(), String> {
        if let Some(model) = applied.model {
            workspace.project_model = model;
            workspace.request_resource_reload();
        }
        match applied.edit {
            Edit::Add(path) => {
                workspace.transient_folders.remove(&path);
                crate::workbench::remember(&mut workspace.recent_folders, path.clone());
                self.install_folder(workspace, path)?;
            }
            Edit::Reveal(path) => {
                if !workspace.folders.iter().any(|f| f.path == path) {
                    workspace.transient_folders.insert(path.clone());
                }
                self.install_folder(workspace, path)?;
            }
            Edit::Remove(path) => {
                workspace.transient_folders.remove(&path);
                workspace.folders.retain(|folder| folder.path != path);
                self.restart_index(workspace);
            }
            Edit::Clear => {
                workspace.transient_folders.clear();
                workspace.folders.clear();
                self.restart_index(workspace);
            }
        }
        Ok(())
    }
}
