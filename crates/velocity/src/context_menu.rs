//! Resource-driven context menus and file operations that need no native UI.
use std::{
    fs::{self, OpenOptions},
    path::{Component, Path, PathBuf},
};
use vel_buffer::Region;
pub use vel_config::ui::{CommandState, ResolvedMenuItem};
use vel_config::{
    Value, json,
    keymap::Invocation,
    ui::{self, MenuItem, PaletteItem},
};
use vel_platform::Point;

/// Window commands intercepted by the application before text-command dispatch.
pub const COMMANDS: &[&str] = &[
    "close_other_tabs",
    "close_tabs_to_right",
    "close_unmodified_tabs",
    "side_bar_open",
    "side_bar_new_file",
    "side_bar_new_folder",
    "side_bar_rename",
    "side_bar_delete",
    "copy_file_path",
    "copy_relative_path",
    "reveal_in_finder",
    "reveal_in_side_bar",
    "side_bar_find_in_folder",
    "side_bar_remove_folder",
];

/// The resource selected by the surface under the context click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextMenuKind {
    /// Text editor surface.
    Editor,
    /// One sheet in a tab group.
    Tab,
    /// A file or directory in the sidebar.
    SideBar,
}

impl ContextMenuKind {
    /// Complete resource filename used as the Workspace context-menu map key.
    pub const fn resource_name(self) -> &'static str {
        match self {
            Self::Editor => "Context.sublime-menu",
            Self::Tab => "Tab Context.sublime-menu",
            Self::SideBar => "Side Bar.sublime-menu",
        }
    }
}

/// Actual click context; it fills keys the resource leaves absent or as `-1` / `[]` / `null`.
#[derive(Clone, Debug, Default)]
pub struct MenuContext {
    /// Selected filesystem entries, in display order.
    pub paths: Vec<PathBuf>,
    /// Clicked group, rather than the previously focused group.
    pub group: usize,
    /// Clicked tab's position within that group.
    pub index: usize,
    /// Window-local click point.
    pub event: Point,
}
impl MenuContext {
    /// Keyword arguments shared by state evaluation and eventual dispatch.
    pub fn arguments(&self) -> Value {
        json!({
            "paths": self.paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
            "group": self.group,
            "index": self.index,
            "event": {"x": self.event.x, "y": self.event.y},
        })
    }
}

/// Arguments from a typed click context or App-enriched JSON context.
pub trait MenuArguments {
    /// Complete keywords supplied to both state evaluation and command dispatch.
    fn menu_arguments(&self) -> Value;
}
impl MenuArguments for MenuContext {
    fn menu_arguments(&self) -> Value {
        self.arguments()
    }
}
impl MenuArguments for Value {
    fn menu_arguments(&self) -> Value {
        self.clone()
    }
}

/// Evaluates visible/enabled/checked state with the same args dispatched on click.
pub fn resolve_menu(
    items: &[MenuItem],
    context: &(impl MenuArguments + ?Sized),
    command_state: impl FnMut(&Invocation) -> CommandState,
) -> Vec<ResolvedMenuItem> {
    let platform = if cfg!(target_os = "macos") {
        "OSX"
    } else if cfg!(target_os = "windows") {
        "Windows"
    } else {
        "Linux"
    };
    ui::resolve_menu(items, platform, &context.menu_arguments(), command_state)
}

/// Expands `velocity_dynamic` in place with spelling/diff/Git commands.
///
/// If a user replaces the resource and omits the marker, dynamic items append.
/// This leaves definitions untouched and lets normal resolution clean separators.
pub fn with_dynamic(items: &[MenuItem], dynamic: &[PaletteItem]) -> Vec<MenuItem> {
    fn expand(items: &[MenuItem], dynamic: &[MenuItem], found: &mut bool) -> Vec<MenuItem> {
        let mut out = Vec::new();
        for item in items {
            if item.id == "velocity_dynamic" {
                *found = true;
                out.extend_from_slice(dynamic);
            } else {
                let mut item = item.clone();
                item.children = expand(&item.children, dynamic, found);
                out.push(item);
            }
        }
        out
    }
    let dynamic: Vec<_> = dynamic
        .iter()
        .map(|item| MenuItem {
            caption: item.caption.clone(),
            id: String::new(),
            command: Some(item.invocation.command.clone()),
            args: item.invocation.args.clone(),
            children: Vec::new(),
            platform: None,
            mnemonic: None,
        })
        .collect();
    let mut found = false;
    let mut out = expand(items, &dynamic, &mut found);
    if !found && !dynamic.is_empty() {
        out.push(MenuItem {
            caption: "-".into(),
            id: String::new(),
            command: None,
            args: json!({}),
            children: Vec::new(),
            platform: None,
            mnemonic: None,
        });
        out.extend(dynamic);
    }
    out
}

/// Keeps every selection when the click lies in any region, including endpoints.
pub fn right_click_preserves_selection(regions: &[Region], byte: usize) -> bool {
    regions.iter().any(|region| region.contains(byte))
}

/// Native filesystem/UI services injected by the application or a headless test.
pub trait SidebarPlatform {
    /// Moves an item to the system Trash. Implementations must never unlink it.
    fn trash(&mut self, path: &Path) -> Result<(), String>;
    /// Replaces the system clipboard's text.
    fn copy_text(&mut self, text: &str) -> Result<(), String>;
    /// Shows an item in the system file browser.
    fn reveal(&mut self, path: &Path) -> Result<(), String>;
}

/// UI/workspace work to apply after an operation. Buffers are never touched here.
#[derive(Clone, Debug, PartialEq)]
pub enum SidebarEffect {
    /// Run this `show_input_panel` invocation; its callback carries the context.
    Input(Invocation),
    /// Ask for confirmation, then dispatch `on_confirm` only on acceptance.
    ConfirmDelete {
        /// Entries to move, with duplicates and covered descendants removed.
        paths: Vec<PathBuf>,
        /// Original context, plus `confirmed: true`.
        on_confirm: Invocation,
    },
    /// Refresh the tree and optionally open a newly created file.
    Created {
        /// Newly created entry.
        path: PathBuf,
        /// Whether the entry is a directory.
        directory: bool,
    },
    /// Retarget matching open tabs and descendant paths without reloading buffers.
    Renamed {
        /// Previous path prefix.
        from: PathBuf,
        /// New path prefix.
        to: PathBuf,
    },
    /// Retain buffers at successful paths and mark the corresponding tabs deleted.
    Deleted {
        /// Successfully trashed entries, including directory prefixes.
        paths: Vec<PathBuf>,
        /// Per-entry errors to show even when other entries succeeded.
        failures: Vec<String>,
    },
    /// Open files or expand selected directories.
    Open(Vec<PathBuf>),
    /// Open Find in Files with this directory in the Where field.
    FindInFolder(PathBuf),
    /// Remove this exact project root from the sidebar, retaining files and tabs.
    RemoveFolder(PathBuf),
    /// Select/reveal this entry in the sidebar.
    RevealInSideBar(PathBuf),
    /// A clipboard or system reveal action completed.
    Done,
}

fn paths(args: &Value) -> Result<Vec<PathBuf>, String> {
    let raw = if let Some(value) = args.get("paths") {
        value
            .as_array()
            .ok_or("paths must be an array")?
            .iter()
            .map(|p| {
                p.as_str()
                    .filter(|p| !p.is_empty())
                    .map(PathBuf::from)
                    .ok_or_else(|| "paths must contain nonempty strings".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?
    } else if let Some(value) = args.get("path") {
        vec![PathBuf::from(
            value
                .as_str()
                .filter(|p| !p.is_empty())
                .ok_or("path must be a nonempty string")?,
        )]
    } else {
        Vec::new()
    };
    let mut unique = Vec::new();
    for path in raw {
        if !unique.contains(&path) {
            unique.push(path);
        }
    }
    if unique.is_empty() {
        return Err("Choose a file or folder first".into());
    }
    Ok(unique)
}
fn one_path(args: &Value) -> Result<PathBuf, String> {
    let paths = paths(args)?;
    if paths.len() != 1 {
        return Err("Choose exactly one file or folder".into());
    }
    Ok(paths[0].clone())
}
fn entry(path: &Path) -> Result<fs::Metadata, String> {
    fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))
}
fn directory_for(path: &Path) -> Result<PathBuf, String> {
    entry(path)?;
    if path.is_dir() {
        return Ok(path.to_owned());
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !parent.is_dir() {
        return Err(format!("{} is not a folder", parent.display()));
    }
    Ok(parent.to_owned())
}
fn validate_name(name: &str) -> Result<(), String> {
    let mut parts = Path::new(name).components();
    if name.is_empty()
        || name.contains(['/', '\\', '\0'])
        || !matches!(parts.next(), Some(Component::Normal(_)))
        || parts.next().is_some()
    {
        return Err("Enter one file or folder name, without a path".into());
    }
    Ok(())
}
fn relative_path(path: &Path, roots: &[PathBuf]) -> Result<String, String> {
    let relative = roots
        .iter()
        .filter_map(|root| {
            path.strip_prefix(root)
                .ok()
                .map(|relative| (root.components().count(), relative))
        })
        .max_by_key(|(depth, _)| *depth)
        .map(|(_, relative)| relative)
        .ok_or_else(|| format!("{} is outside the project folders", path.display()))?;
    Ok(if relative.as_os_str().is_empty() {
        ".".into()
    } else {
        relative.to_string_lossy().into_owned()
    })
}
fn input(name: &str, args: &Value, caption: &str, initial: &str) -> SidebarEffect {
    SidebarEffect::Input(Invocation {
        command: "show_input_panel".into(),
        args: json!({"caption": caption, "initial_text": initial,
            "on_done": {"command": name, "args": args}}),
    })
}
fn deletion_paths(args: &Value) -> Result<Vec<PathBuf>, String> {
    let entries = paths(args)?
        .into_iter()
        .map(|path| {
            if path.file_name().is_none() {
                return Err("Cannot move a filesystem root to the Trash".into());
            }
            let directory = entry(&path)?.is_dir();
            Ok((path, directory))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(entries
        .iter()
        .filter(|(path, _)| {
            !entries.iter().any(|(ancestor, directory)| {
                *directory && ancestor != path && path.starts_with(ancestor)
            })
        })
        .map(|(path, _)| path.clone())
        .collect())
}

/// Routes sidebar/path commands, returning `None` for commands owned elsewhere.
///
/// Name input uses `text`; no mutation occurs before input is submitted. Delete
/// always requires the exact boolean `confirmed: true` and calls only `trash`.
/// A partial multi-delete returns successes and failures together for tab updates.
pub fn sidebar_command(
    name: &str,
    args: &Value,
    roots: &[PathBuf],
    platform: &mut dyn SidebarPlatform,
) -> Result<Option<SidebarEffect>, String> {
    let effect = match name {
        "side_bar_new_file" | "side_bar_new_folder" => {
            let path = one_path(args)?;
            let parent = directory_for(&path)?;
            let directory = name == "side_bar_new_folder";
            let Some(text) = args.get("text") else {
                return Ok(Some(input(
                    name,
                    args,
                    if directory {
                        "Name for the new folder"
                    } else {
                        "Name for the new file"
                    },
                    "",
                )));
            };
            let text = text.as_str().ok_or("text must be a name")?;
            validate_name(text)?;
            let path = parent.join(text);
            if directory {
                fs::create_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            } else {
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
            }
            SidebarEffect::Created { path, directory }
        }
        "side_bar_rename" => {
            let from = one_path(args)?;
            entry(&from)?;
            let old_name = from.file_name().ok_or("Cannot rename a filesystem root")?;
            let Some(text) = args.get("text") else {
                return Ok(Some(input(
                    name,
                    args,
                    "New name for this entry",
                    &old_name.to_string_lossy(),
                )));
            };
            let text = text.as_str().ok_or("text must be a name")?;
            validate_name(text)?;
            let to = from.with_file_name(text);
            if from != to {
                // One exclusive rename: an existing destination is never replaced, and a
                // case-only change on a case-insensitive volume is allowed.
                vel_fileio::rename_no_replace(&from, &to).map_err(|e| {
                    if e.kind() == std::io::ErrorKind::AlreadyExists {
                        format!("{} already exists", to.display())
                    } else {
                        format!("{}: {e}", from.display())
                    }
                })?;
            }
            SidebarEffect::Renamed { from, to }
        }
        "side_bar_delete" => {
            let paths = deletion_paths(args)?;
            if args["confirmed"].as_bool() != Some(true) {
                let mut confirmed = args.clone();
                confirmed["paths"] = json!(paths);
                confirmed["confirmed"] = json!(true);
                return Ok(Some(SidebarEffect::ConfirmDelete {
                    paths,
                    on_confirm: Invocation {
                        command: name.into(),
                        args: confirmed,
                    },
                }));
            }
            let mut trashed = Vec::new();
            let mut failures = Vec::new();
            for path in paths {
                match platform.trash(&path) {
                    Ok(()) => trashed.push(path),
                    Err(error) => failures.push(format!("{}: {error}", path.display())),
                }
            }
            SidebarEffect::Deleted {
                paths: trashed,
                failures,
            }
        }
        "copy_file_path" | "copy_relative_path" => {
            let text = paths(args)?
                .iter()
                .map(|path| {
                    if name == "copy_relative_path" {
                        relative_path(path, roots)
                    } else {
                        Ok(path.to_string_lossy().into_owned())
                    }
                })
                .collect::<Result<Vec<_>, String>>()?
                .join("\n");
            platform.copy_text(&text)?;
            SidebarEffect::Done
        }
        "reveal_in_finder" => {
            for path in paths(args)? {
                platform
                    .reveal(&path)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
            }
            SidebarEffect::Done
        }
        "reveal_in_side_bar" => SidebarEffect::RevealInSideBar(one_path(args)?),
        "side_bar_open" => {
            let paths = paths(args)?;
            for path in &paths {
                entry(path)?;
            }
            SidebarEffect::Open(paths)
        }
        "side_bar_find_in_folder" => SidebarEffect::FindInFolder(directory_for(&one_path(args)?)?),
        "side_bar_remove_folder" => {
            let path = one_path(args)?;
            if !roots.contains(&path) {
                return Err("Choose a project root folder to remove".into());
            }
            SidebarEffect::RemoveFolder(path)
        }
        _ => return Ok(None),
    };
    Ok(Some(effect))
}

/// State for the commands above; unknown commands defer to the editor/App state.
///
/// Only the root-removal row is hidden outside project roots. Filesystem mutation
/// errors are still checked when dispatch occurs, after input/confirmation.
pub fn sidebar_command_state(name: &str, args: &Value, roots: &[PathBuf]) -> Option<CommandState> {
    if !COMMANDS.contains(&name) || name.starts_with("close_") {
        return None;
    }
    let selected = paths(args).unwrap_or_default();
    let single = selected.first().filter(|_| selected.len() == 1);
    let root = single.is_some_and(|path| roots.contains(path));
    let enabled = match name {
        "side_bar_new_file" | "side_bar_new_folder" | "side_bar_find_in_folder" => {
            single.is_some_and(|p| directory_for(p).is_ok())
        }
        "side_bar_rename" => single.is_some_and(|p| p.file_name().is_some() && entry(p).is_ok()),
        "side_bar_delete" => deletion_paths(args).is_ok(),
        "copy_relative_path" => {
            !selected.is_empty() && selected.iter().all(|p| relative_path(p, roots).is_ok())
        }
        "side_bar_remove_folder" => root,
        "side_bar_open" => !selected.is_empty() && selected.iter().all(|p| entry(p).is_ok()),
        "reveal_in_side_bar" => single.is_some(),
        _ => !selected.is_empty(),
    };
    Some(CommandState {
        enabled,
        visible: name != "side_bar_remove_folder" || root,
        checked: false,
    })
}

/// Context tab-close operation within the clicked group only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabCloseKind {
    /// Close the clicked sheet, including a modified sheet after normal prompting.
    Current,
    /// Keep the clicked sheet; close the remaining sheets after normal prompting.
    Others,
    /// Close sheets after the clicked sheet in visual group order.
    ToRight,
    /// Close every unmodified sheet in the group, including the clicked one.
    Unmodified,
}
impl TabCloseKind {
    /// Maps both existing single-close and custom context resource commands.
    pub fn from_command(name: &str) -> Option<Self> {
        Some(match name {
            "close" | "close_file" | "close_by_index" => Self::Current,
            "close_other_tabs" | "close_others_by_index" => Self::Others,
            "close_tabs_to_right" | "close_to_right_by_index" => Self::ToRight,
            "close_unmodified_tabs" | "close_unmodified" => Self::Unmodified,
            _ => return None,
        })
    }
}

/// Snapshot of one group's visual tab order, using stable sheet identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabSnapshot {
    /// Stable sheet ID; use it to look up current vector indices during closing.
    pub sheet: u64,
    /// Current document/format dirty state.
    pub dirty: bool,
}

/// Computes close targets in reverse visual order, independent of active focus.
///
/// Invalid/stale click indices close nothing. The caller must use its usual dirty
/// prompt for Current/Others/ToRight, and stop closing when the user cancels.
pub fn tab_close_targets(tabs: &[TabSnapshot], index: usize, kind: TabCloseKind) -> Vec<u64> {
    if index >= tabs.len() {
        return Vec::new();
    }
    tabs.iter()
        .enumerate()
        .rev()
        .filter(|(i, tab)| match kind {
            TabCloseKind::Current => *i == index,
            TabCloseKind::Others => *i != index,
            TabCloseKind::ToRight => *i > index,
            TabCloseKind::Unmodified => !tab.dirty,
        })
        .map(|(_, tab)| tab.sheet)
        .collect()
}

/// Maps a renamed file/directory prefix onto an open tab's path.
///
/// Component-aware matching leaves siblings such as `src-old` untouched.
pub fn retarget_path(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    path.strip_prefix(from).ok().map(|relative| {
        if relative.as_os_str().is_empty() {
            to.to_owned()
        } else {
            to.join(relative)
        }
    })
}

/// Resolves symlinks in the parent directories of a side bar entry, keeping its own name.
///
/// Tabs store canonical paths while side bar rows are built from the folder as written,
/// so an entry below a symlinked directory has two spellings. The final component is not
/// resolved: a symlink entry itself is renamed or trashed, never its target.
pub fn canonical_entry(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => fs::canonicalize(parent)
            .map(|parent| parent.join(name))
            .unwrap_or_else(|_| path.to_owned()),
        _ => path.to_owned(),
    }
}

/// Whether a successful trash operation removed an open tab's backing path.
pub fn path_was_deleted(path: &Path, deleted: &[PathBuf]) -> bool {
    deleted
        .iter()
        .any(|root| path.starts_with(root) || path.starts_with(canonical_entry(root)))
}

/// Materializes affected lazy tabs before a submitted rename or confirmed trash.
///
/// Call before filesystem mutation. A background load must finish before retrying;
/// this prevents a rename/delete from racing a loader's first open of the old path.
pub fn prepare_file_operation(
    workspace: &mut crate::workspace::Workspace,
    paths: &[PathBuf],
) -> Result<(), String> {
    let affected: Vec<_> = workspace
        .tabs
        .iter()
        .enumerate()
        .filter_map(|(i, tab)| {
            tab.path
                .as_deref()
                .filter(|path| path_was_deleted(path, paths))
                .map(|_| i)
        })
        .collect();
    let mut failures = Vec::new();
    for index in affected {
        if workspace.tabs[index].lazy.is_some()
            && let Err(error) = workspace.prepare_sheet(index)
        {
            failures.push(error);
            continue;
        }
        let tab = &workspace.tabs[index];
        if tab.loading.is_some() || tab.binary_loading.is_some() {
            failures.push(format!(
                "Wait for {} to finish loading before changing its file",
                tab.label()
            ));
        }
    }
    workspace.prepare_sheet(workspace.active)?;
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

/// Retargets all matching sheets after a successful filesystem rename.
///
/// Text, undo history, view state and shared document identities survive. Every
/// path is updated before watcher/language errors are returned, since the rename
/// has already happened on disk. Call `prepare_file_operation` before renaming.
pub fn apply_rename(
    workspace: &mut crate::workspace::Workspace,
    from: &Path,
    to: &Path,
) -> Result<(), String> {
    if from == to {
        return Ok(());
    }
    let (canonical_from, canonical_to) = (canonical_entry(from), canonical_entry(to));
    let mut affected = Vec::new();
    for (index, tab) in workspace.tabs.iter_mut().enumerate() {
        let Some(path) = tab.path.as_deref().and_then(|p| {
            retarget_path(p, from, to).or_else(|| retarget_path(p, &canonical_from, &canonical_to))
        }) else {
            continue;
        };
        // A rename keeps the file's length and modification time, so the disk stamps and
        // any pending external change stay valid. A buffer whose file was already deleted
        // (for example inside a renamed folder) stays deleted and dirty.
        tab.deleted = tab.deleted && fs::symlink_metadata(&path).is_err();
        tab.path = Some(path.clone());
        tab.editor.snippet_path = Some(path.clone());
        tab.transient = false;
        if let Some((document, _)) = &mut tab.lazy {
            document.path = Some(path);
        }
        affected.push(index);
    }
    for folder in &mut workspace.folders {
        if let Some(path) = retarget_path(&folder.path, from, to) {
            if folder
                .path
                .file_name()
                .is_some_and(|old| old.to_string_lossy() == folder.name)
            {
                folder.name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
            }
            folder.path = path;
        }
    }
    // Project entries may be relative to the project file's directory (which may itself
    // move). Resolve them against the old location and keep them relative when possible.
    let old_base = workspace
        .project
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_owned);
    let new_base = old_base
        .as_deref()
        .map(|base| retarget_path(base, from, to).unwrap_or_else(|| base.to_owned()));
    let mut project_folders_changed = false;
    for folder in &mut workspace.project_model.folders {
        let resolved = match &old_base {
            Some(base) if folder.path.is_relative() => base.join(&folder.path),
            _ => folder.path.clone(),
        };
        let Some(path) = retarget_path(&resolved, from, to) else {
            continue;
        };
        let relative = folder.path.is_relative().then(|| {
            new_base
                .as_deref()
                .and_then(|base| path.strip_prefix(base).ok())
                .map(|p| {
                    if p.as_os_str().is_empty() {
                        PathBuf::from(".")
                    } else {
                        p.to_owned()
                    }
                })
        });
        let path = relative.flatten().unwrap_or(path);
        if folder.path != path {
            folder.path = path;
            project_folders_changed = true;
        }
    }
    for list in [
        &mut workspace.recent_files,
        &mut workspace.recent_folders,
        &mut workspace.recent_projects,
    ] {
        for path in list {
            if let Some(renamed) = retarget_path(path, from, to) {
                *path = renamed;
            }
        }
    }
    if let Some(path) = workspace
        .project
        .as_deref()
        .and_then(|p| retarget_path(p, from, to))
    {
        workspace.project = Some(path);
        workspace.request_resource_reload();
    }
    let mut failures = Vec::new();
    if project_folders_changed && let Some(project) = workspace.project.clone() {
        // Without saving, the next resource reload would bring the old root back.
        if let Err(error) = workspace.project_model.save(&project) {
            failures.push(error);
        }
        workspace.request_resource_reload();
    }
    for index in affected {
        let path = workspace.tabs[index]
            .path
            .as_deref()
            .expect("retargeted path");
        if let Some(watcher) = workspace.watcher.as_mut()
            && let Err(error) = watcher.watch(path)
        {
            failures.push(format!("{}: {error}", path.display()));
        }
        if let Err(error) = workspace.configure_language(index, None, true) {
            failures.push(error);
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

/// Detaches a root from both the live sidebar and the in-memory project model.
///
/// Project model paths may be relative to the project file, while sidebar paths
/// are canonical. Retained model entries keep their original path spelling and
/// settings. The caller refreshes Panels and persists the updated project model;
/// this function never writes the project file or removes files/open tabs.
/// Returns whether either folder list changed.
pub fn apply_remove_folder(workspace: &mut crate::workspace::Workspace, path: &Path) -> bool {
    let root = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let matches = |candidate: &Path| {
        candidate == root || fs::canonicalize(candidate).is_ok_and(|resolved| resolved == root)
    };
    let previous_live = workspace.folders.len();
    workspace.folders.retain(|folder| !matches(&folder.path));
    let project_base = workspace
        .project_path()
        .and_then(Path::parent)
        .unwrap_or(Path::new("."))
        .to_owned();
    // The root can already be in Trash. Resolve its still-existing project directory
    // (e.g. /var -> /private/var) before matching entries whose last component is gone.
    let project_base = fs::canonicalize(&project_base).unwrap_or(project_base);
    let previous_model = workspace.project_model.folders.len();
    workspace.project_model.folders.retain(|folder| {
        let resolved = if folder.path.is_absolute() {
            folder.path.clone()
        } else {
            project_base.join(&folder.path)
        };
        !matches(&resolved)
    });
    previous_live != workspace.folders.len()
        || previous_model != workspace.project_model.folders.len()
}

/// Marks successfully trashed open files deleted while retaining all buffer state.
///
/// Include only `SidebarEffect::Deleted::paths`, never its failed entries. Parent
/// paths cover descendants and every shared sheet. Call `prepare_file_operation`
/// before trashing so hidden lazy documents have their contents in memory too.
pub fn apply_delete(workspace: &mut crate::workspace::Workspace, paths: &[PathBuf]) {
    for tab in &mut workspace.tabs {
        if tab
            .path
            .as_deref()
            .is_some_and(|path| path_was_deleted(path, paths))
        {
            tab.deleted = true;
            tab.transient = false;
            tab.disk_stamp = None;
            tab.disk_saved = None;
            tab.external = None;
            if let Some((document, _)) = &mut tab.lazy {
                document.disk = None;
            }
        }
    }
}

#[cfg(test)]
#[path = "context_tests.rs"]
mod context_tests;
