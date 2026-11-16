//! Window bridge for recorded text commands, synchronous find playback and package macro files.
use super::*;

type MacroCommands = Arc<[(String, Value)]>;

/// Only parsed macros that have actually been invoked, invalidated by the resource revision.
#[derive(Debug, Default)]
pub(super) struct MacroFiles {
    root: Option<PathBuf>,
    revision: u64,
    parsed: BTreeMap<String, MacroCommands>,
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
        // Profile loose override/archive, then our bundled package, then clean-room built-ins.
        // read_one opens this path only; never rebuild Resources on the UI command path.
        let mut commands = None;
        for root in [root, crate::release::bundle_resources()]
            .into_iter()
            .flatten()
        {
            match vel_config::resources::read_one(&root, path) {
                Ok(Some(resource)) => {
                    commands = Some(vel_commands::parse_macro(resource.text()?)?);
                    break;
                }
                Ok(None) => {}
                Err(error) => workspace
                    .messages
                    .push(format!("Macro resource warning: {error}")),
            }
        }
        let commands: MacroCommands = commands
            .or_else(|| vel_commands::builtin_macro_file(file))
            .ok_or_else(|| format!("macro file {file} is not available"))?
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
            workspace.macro_paths.insert(format!(
                "Packages/User/{}",
                path.file_name()
                    .expect("saved macro filename")
                    .to_string_lossy()
            ));
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
