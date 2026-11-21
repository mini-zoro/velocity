//! The native external-terminal action. Paths are never evaluated as shell code.
use super::*;
use std::{ffi::OsString, process::Command};

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Launch {
    pub(super) directory: PathBuf,
    pub(super) program: OsString,
    pub(super) arguments: Vec<OsString>,
}

impl Launch {
    pub(super) fn open(self) -> Result<(), String> {
        let mut child = Command::new(&self.program)
            .args(self.arguments)
            .current_dir(&self.directory)
            .stdin(std::process::Stdio::null())
            .spawn()
            .map_err(|error| format!("Could not open a terminal: {error}"))?;
        // A custom terminal may run as long as its window is open.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

impl App {
    pub(super) fn terminal_launch(&self, args: &Value) -> Result<Launch, String> {
        use vel_config::settings::Layer;
        let file = args["_context_sheet"]
            .as_u64()
            .and_then(|id| self.workspace.sheet_index(id))
            .or_else(|| (!self.workspace.is_empty()).then_some(self.workspace.active))
            .and_then(|index| self.workspace.tabs[index].path.as_deref());
        let folder = self
            .workspace
            .folders
            .first()
            .map(|folder| folder.path.as_path());
        let clicked = args["paths"]
            .as_array()
            .and_then(|paths| paths.first())
            .and_then(Value::as_str);
        let directory = match args["dir"].as_str() {
            Some("$file_path" | "${file_path}") => file.and_then(Path::parent),
            Some("$folder" | "${folder}") => folder,
            Some(dir) => Some(Path::new(dir)),
            None => clicked
                .map(Path::new)
                .map(|path| {
                    if path.is_dir() {
                        path
                    } else {
                        path.parent().unwrap_or(path)
                    }
                })
                .or_else(|| file.and_then(Path::parent))
                .or(folder),
        }
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or("No file or folder to open in a terminal")?;
        let directory = directory
            .canonicalize()
            .map_err(|error| format!("{}: {error}", directory.display()))?;
        if !directory.is_dir() {
            return Err(format!("{} is not a directory", directory.display()));
        }

        // Do not run commands from .sublime-project or syntax settings.
        let setting = [Layer::User, Layer::Platform, Layer::Default]
            .into_iter()
            .find_map(|layer| self.workspace.settings.layer(layer).get("terminal_command"));
        let mut arguments = Vec::new();
        let program = match setting {
            Some(Value::Array(items)) if !items.is_empty() => {
                for arg in &items[1..] {
                    let arg = arg
                        .as_str()
                        .ok_or("terminal_command arguments must be strings")?;
                    arguments.push(match arg {
                        "$dir" | "${dir}" => directory.as_os_str().to_owned(),
                        _ => arg.into(),
                    });
                }
                items[0]
                    .as_str()
                    .filter(|program| !program.is_empty())
                    .ok_or("terminal_command needs an executable")?
                    .into()
            }
            Some(Value::String(custom)) if !custom.is_empty() => {
                if custom.ends_with(".app") {
                    arguments.extend([
                        "-a".into(),
                        custom.into(),
                        "--".into(),
                        directory.as_os_str().to_owned(),
                    ]);
                    "/usr/bin/open".into()
                } else {
                    custom.into()
                }
            }
            _ => {
                arguments.extend([
                    "-b".into(),
                    "com.apple.Terminal".into(),
                    "--".into(),
                    directory.as_os_str().to_owned(),
                ]);
                "/usr/bin/open".into()
            }
        };
        Ok(Launch {
            directory,
            program,
            arguments,
        })
    }
}
