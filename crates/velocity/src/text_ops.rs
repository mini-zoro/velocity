//! Window bridge for recorded text commands, synchronous find playback and package macro files.
use super::*;
use vel_config::resources::Resources;

type MacroCommands = Arc<[(String, Value)]>;

/// Only parsed macros that have actually been invoked, invalidated by the resource revision.
#[derive(Debug, Default)]
pub(super) struct MacroFiles {
    root: Option<PathBuf>,
    revision: u64,
    parsed: BTreeMap<String, MacroCommands>,
}

fn load_macro(resources: &Resources, path: &str) -> Result<Vec<(String, Value)>, String> {
    if let Some(resource) = resources.get(path) {
        return vel_commands::parse_macro(resource.text()?);
    }
    if resources.package_enabled("Default")
        && let Some(commands) = vel_commands::builtin_macro_file(path)
    {
        return Ok(commands);
    }
    Err(format!("macro file {path} is not available"))
}

pub(crate) fn usable_package_binding(
    resources: &Resources,
    known: &impl Fn(&str) -> bool,
    command: &str,
    args: &Value,
    depth: usize,
) -> bool {
    if depth >= 32 || !known(command) {
        return false;
    }
    let step =
        |name: &str, args: &Value| usable_package_binding(resources, known, name, args, depth + 1);
    match command {
        "run_macro_file" if args.get("file").is_some() => {
            return args["file"]
                .as_str()
                .and_then(|p| load_macro(resources, p).ok())
                .is_some_and(|commands| commands.iter().all(|(name, args)| step(name, args)));
        }
        "chain" => {
            return args["commands"].as_array().is_some_and(|commands| {
                commands.iter().all(|c| {
                    vel_commands::chain_entry(c).is_ok_and(|(name, args)| step(name, args))
                })
            });
        }
        "run_macro" if args["commands"].is_array() => {
            return vel_commands::parse_macro(&args["commands"].to_string())
                .is_ok_and(|commands| commands.iter().all(|(name, args)| step(name, args)));
        }
        "insert_snippet" if !args["contents"].is_string() => {
            return args["name"]
                .as_str()
                .or_else(|| args["file"].as_str())
                .and_then(|path| resources.get(path))
                .is_some_and(|resource| {
                    resource
                        .text()
                        .is_ok_and(|s| vel_snippet::SnippetFile::parse(s).is_ok())
                });
        }
        "edit_settings" if !args["base_file"].is_string() => return false,
        _ => (),
    }
    ["file", "base_file"]
        .iter()
        .filter_map(|key| args[key].as_str())
        .all(|path| {
            if let Some(relative) = path
                .strip_prefix("${packages}/")
                .or_else(|| path.strip_prefix("$packages/"))
            {
                return resources
                    .get(&format!("Packages/{relative}"))
                    .is_some_and(|r| r.text().is_ok());
            }
            let path = path.strip_prefix("res://").unwrap_or(path);
            !path.starts_with("Packages/") || resources.get(path).is_some_and(|r| r.text().is_ok())
        })
}

impl Panels {
    fn macro_file(
        &mut self,
        workspace: &mut Workspace,
        file: &str,
    ) -> Result<MacroCommands, String> {
        let path = file.strip_prefix("res://").unwrap_or(file);
        let package = path
            .strip_prefix("Packages/")
            .and_then(|p| p.split('/').next());
        if package.is_some_and(|p| p != "User" && workspace.ignored_packages().contains(p)) {
            return Err(format!("macro file {file} is not available"));
        }
        let root = workspace.data_path().map(Path::to_path_buf);
        let cache = &mut self.macro_files;
        if cache.root != root || cache.revision != workspace.resource_revision {
            cache.parsed.clear();
            cache.root = root.clone();
            cache.revision = workspace.resource_revision;
        }
        if let Some(commands) = cache.parsed.get(path) {
            return Ok(Arc::clone(commands));
        }
        if let Some(warning) =
            package.and_then(|package| workspace.resources.archive_warning(package))
        {
            workspace
                .messages
                .push(format!("Macro resource warning: {warning}"));
        }
        // Shipped and third-party bindings use the same immutable overlay
        // validated at load time. Keep the existing first-use behavior for
        // User-recorded macros that can be created or edited between reloads.
        let live = if package == Some("User")
            && let Some(root) = root
        {
            match vel_config::resources::read_one(&root, path) {
                Ok(Some(resource)) => Some(resource.text().and_then(vel_commands::parse_macro)),
                Ok(None) => None,
                Err(error) => {
                    workspace
                        .messages
                        .push(format!("Macro resource warning: {error}"));
                    None
                }
            }
        } else {
            None
        };
        let commands: MacroCommands = match live {
            Some(Ok(commands)) => commands,
            Some(Err(error)) if workspace.resources.get(path).is_none() => return Err(error),
            _ => load_macro(&workspace.resources, path)?,
        }
        .into();
        cache.parsed.insert(path.into(), Arc::clone(&commands));
        Ok(commands)
    }
    /// Effective package macro commands, with our own captions, for the macro chooser/menu.
    pub fn macro_file_items(
        &mut self,
        workspace: &Workspace,
    ) -> Result<Vec<vel_config::ui::PaletteItem>, String> {
        // Configuration already builds the effective resource index off the command path.
        // Retain filenames here, never a second copy of every package's resource contents.
        let ignored = workspace.ignored_packages();
        let mut paths = workspace.macro_paths.clone();
        paths.retain(|path| {
            path.split('/')
                .nth(1)
                .is_some_and(|p| p == "User" || !ignored.contains(p))
        });
        for name in [
            "Add Line",
            "Add Line Before",
            "Add Line in Braces",
            "Delete Line",
            "Delete to Hard BOL",
            "Delete to Hard EOL",
            "Delete Left Right",
        ] {
            if !ignored.contains("Default") {
                paths.insert(format!("Packages/Default/{name}.sublime-macro"));
            }
        }
        Ok(paths
            .into_iter()
            .map(|path| vel_config::ui::PaletteItem {
                caption: format!(
                    "Macro: {}",
                    path.trim_start_matches("Packages/")
                        .trim_end_matches(".sublime-macro")
                ),
                invocation: Invocation {
                    command: "run_macro_file".into(),
                    args: json!({"file":format!("res://{path}")}),
                },
            })
            .collect())
    }
    /// Macro naming and file chooser UI. Playback itself belongs to App's named dispatcher.
    pub(super) fn macro_action(
        &mut self,
        workspace: &mut Workspace,
        name: &str,
        args: &Value,
        viewport: Range<usize>,
    ) -> Result<bool, String> {
        if name == "save_macro" {
            let Some(filename) = args["text"].as_str().or_else(|| args["name"].as_str()) else {
                return self.action(
                    workspace,
                    "show_input_panel",
                    &json!({
                    "caption":"Save recorded macro as", "initial_text":"Recorded.sublime-macro",
                    "on_done":{"command":"save_macro","args":{}}}),
                    viewport,
                );
            };
            if filename.is_empty()
                || filename.contains(['/', '\\'])
                || matches!(filename, "." | "..")
            {
                return Err("Choose a macro file name inside Packages/User".into());
            }
            let root = workspace
                .data_path()
                .ok_or("No profile directory for saving macros")?
                .join("Packages/User");
            std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
            let filename = if filename.ends_with(".sublime-macro") {
                filename.to_owned()
            } else {
                format!("{filename}.sublime-macro")
            };
            let source = workspace.editor().registry.macro_source()? + "\n";
            let path = save_new_macro(&root, &filename, &source)?;
            let resource = format!(
                "Packages/User/{}",
                path.file_name()
                    .expect("saved macro filename")
                    .to_string_lossy()
            );
            workspace
                .resources
                .insert(&resource, source.into_bytes(), path.clone());
            workspace.macro_paths.insert(resource);
            workspace.request_resource_reload();
            self.set_message(format!("Saved macro {}", path.display()));
            return Ok(true);
        }
        if name == "run_macro_file" && args.get("file").is_none() {
            let items = self.macro_file_items(workspace)?;
            self.choices(workspace, items);
            return Ok(true);
        }
        Ok(false)
    }
    /// Resolves a complete playback before execution. The application dispatches every step
    /// through the same named-command path as a key/menu and keeps this guard until completion.
    pub(crate) fn macro_playback(
        &mut self,
        workspace: &mut Workspace,
        name: &str,
        args: &Value,
        viewport: Range<usize>,
    ) -> Result<Option<MacroPlayback>, String> {
        if name == "run_macro_file" && args.get("file").is_none() {
            self.macro_action(workspace, name, args, viewport)?;
            return Ok(None);
        }
        let registry = &workspace.editor().registry;
        if registry.playback_depth() >= 32 {
            return Err("macro recursion exceeds 32".into());
        }
        if name == "run_macro" && registry.is_recording() {
            return Err("stop recording before replay".into());
        }
        let commands = if name == "run_macro" {
            if let Some(commands) = args.get("commands") {
                vel_commands::parse_macro(&commands.to_string())?.into()
            } else {
                registry.macro_commands().into()
            }
        } else {
            let file = args["file"].as_str().ok_or("run_macro_file needs a file")?;
            self.macro_file(workspace, file)?
        };
        if name == "run_macro_file" {
            workspace.editor_mut().registry.record_command(name, args);
        }
        Ok(Some(MacroPlayback {
            commands,
            recording: workspace.editor().registry.suspend_recording(),
        }))
    }
}

/// A resolved macro and its recording suppression lifetime, owned by the window dispatcher.
pub(crate) struct MacroPlayback {
    pub commands: MacroCommands,
    pub recording: vel_commands::RecordingGuard,
}

// Publish complete new contents without ever replacing an existing path, including a symlink.
// The hard-link operation is atomic and fails on an occupied name; retrying picks a free suffix.
fn save_new_macro(root: &Path, filename: &str, source: &str) -> Result<PathBuf, String> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let (temporary, mut file) = loop {
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let temporary = root.join(format!(
            ".velocity-macro-{}-{serial}.tmp",
            std::process::id()
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    };
    let result = (|| {
        file.write_all(source.as_bytes())
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        let stem = filename
            .strip_suffix(".sublime-macro")
            .expect("normalized macro extension");
        for suffix in 1usize.. {
            let name = if suffix == 1 {
                filename.to_owned()
            } else {
                format!("{stem} {suffix}.sublime-macro")
            };
            let path = root.join(name);
            match std::fs::hard_link(&temporary, &path) {
                Ok(()) => return Ok(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.to_string()),
            }
        }
        unreachable!("finite directory exhausted all macro file names")
    })();
    let _ = std::fs::remove_file(temporary);
    result
}
