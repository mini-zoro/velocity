//! Settings windows built from our resources and User files, through the normal editor/loaders.
use crate::{app::App, cli::FileLocation, workbench::restore_settings_tab, workspace::Workspace};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};
use vel_config::{Value, json, resources::Resources};
use vel_platform::{Size, WindowHandle, WindowOptions};
use vel_text::Text;

fn expand(path: &str, data: &Path) -> String {
    // Public platform names: https://docs.sublimetext.io/guide/customization/settings.html
    path.replace("${platform}", "OSX")
        .replace("$platform", "OSX")
        .replace("${packages}", &data.join("Packages").to_string_lossy())
        .replace("$packages", &data.join("Packages").to_string_lossy())
}
fn source(base: &str, data: &Path, resources: &Resources) -> Result<(String, String), String> {
    let expanded = expand(base, data);
    let resource = Path::new(&expanded).strip_prefix(data).ok().map_or_else(
        || expanded.clone(),
        |p| p.to_string_lossy().replace('\\', "/"),
    );
    if let Some(resource) = resources.get(&resource) {
        return Ok((resource.path.clone(), resource.text()?.to_owned()));
    }
    if resource.starts_with("res://") || resource.starts_with("Packages/") {
        return Err(format!("settings resource {resource} is not available"));
    }
    let mut source = String::new();
    vel_fileio::regular::read(Path::new(&expanded))?
        .read_to_string(&mut source)
        .map_err(|e| format!("{expanded}: {e}"))?;
    Ok((expanded, source))
}
fn create_user(path: &Path, template: &str) -> Result<(), String> {
    if path
        .try_exists()
        .map_err(|e| format!("{}: {e}", path.display()))?
    {
        return Ok(());
    }
    let parent = path.parent().ok_or("User file needs a parent directory")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    // A complete template is published atomically and only if absent. Opening
    // settings twice (including two windows) can never overwrite an existing file.
    use vel_fileio::atomic::{Durability, Stage};
    let mut temporary =
        Stage::new(parent, ".velocity-settings-", None).map_err(|e| e.to_string())?;
    let expansion = vel_snippet::Snippet::parse(template)
        .map_err(|e| e.to_string())?
        .expand(
            &Text::new(),
            &std::iter::once(0..0).collect::<Vec<_>>(),
            &vel_snippet::Environment::default(),
            vel_snippet::ExpansionOptions {
                translate_tabs_to_spaces: false,
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
    temporary
        .file()
        .write_all(expansion.edits[0].text.as_bytes())
        .map_err(|e| e.to_string())?;
    temporary
        .sync(Durability::Full)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    match temporary.publish(path, true) {
        Ok(p) => {
            if let Some(warning) = p.warning {
                return Err(warning);
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}
/// Preferences left/default, right/User, as documented at settings.html. The
/// command's paths/variables follow reference/commands.html and the new-window
/// edit_settings example in the public community customization guide.
fn settings_workspace(
    data: &Path,
    args: &Value,
    resources: &Resources,
) -> Result<Workspace, String> {
    // https://www.sublimetext.com/docs/settings.html
    // https://docs.sublimetext.io/reference/commands.html
    let base = args["base_file"]
        .as_str()
        .ok_or("edit_settings needs base_file")?;
    let (resource, contents) = source(base, data, resources)?;
    let name = Path::new(&resource)
        .file_name()
        .ok_or("base_file needs a filename")?
        .to_string_lossy()
        .into_owned();
    let user = args["user_file"]
        .as_str()
        .map(|s| {
            let path = PathBuf::from(expand(s, data));
            if path.is_absolute() {
                path
            } else {
                data.join(path)
            }
        })
        .unwrap_or_else(|| data.join("Packages/User").join(&name));
    create_user(&user, args["default"].as_str().unwrap_or("{\n\t$0\n}\n"))?;
    let mut workspace = Workspace::new(
        Text::from(contents.as_str()),
        Some(data.to_owned()),
        None,
        false,
    )?;
    #[cfg(test)]
    workspace.wait_resources();
    // Resource documents are scratch views: retain their text in sessions, never
    // pretend a packed resource is a writable filesystem path.
    let tab = &mut workspace.tabs[workspace.active];
    restore_settings_tab(
        tab,
        &vel_session::SettingsSheet {
            resource: Some(resource),
            label: format!("{} (Default)", name),
        },
    );
    workspace.configure_language(workspace.active, None, false)?;
    workspace.workbench_action("set_layout", &json!({"preset":"columns2"}))?;
    workspace.workbench.active = 1;
    workspace.open(FileLocation {
        path: user,
        line: 1,
        column: 1,
    })?;
    restore_settings_tab(
        &mut workspace.tabs[workspace.active],
        &vel_session::SettingsSheet {
            resource: None,
            label: format!("{} (User)", name),
        },
    );
    workspace.configure_language(workspace.active, None, false)?;
    Ok(workspace)
}
impl App {
    pub(super) fn apply_settings_action(
        &mut self,
        window: Option<&WindowHandle>,
        name: &str,
        args: &Value,
    ) -> Result<bool, String> {
        if !matches!(
            name,
            "edit_settings"
                | "edit_syntax_settings"
                | "open_file_settings"
                | "customize_color_scheme"
                | "customize_theme"
                | "open_dir"
        ) {
            return Ok(false);
        }
        let data = self
            .workspace
            .data_path()
            .map(Path::to_owned)
            .ok_or("Settings require a data directory")?;
        if name == "open_dir" {
            let directory = args["dir"].as_str().unwrap_or("$packages");
            // Window/build variables share the documented expansion semantics.
            // https://www.sublimetext.com/docs/build_systems.html#variables
            // open_dir opens a directory; it never creates one:
            // https://docs.sublimetext.io/reference/commands.html#open-dir
            let packages = data.join("Packages");
            let variables = vel_build::Variables::new(
                self.workspace.tabs[self.workspace.active].path.as_deref(),
                self.workspace.folders.first().map(|f| f.path.as_path()),
                self.workspace.project_path(),
                Some(&packages),
            );
            let directory = PathBuf::from(variables.expand(directory)?);
            if !directory.is_absolute() || !directory.is_dir() {
                return Err(format!("{}: directory does not exist", directory.display()));
            }
            if let Some(window) = window {
                window.show_directory(&directory)?;
            } else {
                self.revealed_directory = Some(directory);
            }
            return Ok(true);
        }
        let mut args = args.clone();
        if matches!(name, "edit_syntax_settings" | "open_file_settings") {
            let language = self.workspace.editor().language.borrow();
            let (syntax, display) = language.as_ref().map_or(("Plain Text", "Plain Text"), |l| {
                (
                    crate::workspace::syntax_setting_key(&l.syntax),
                    l.name.as_str(),
                )
            });
            // Syntax-specific settings layer: https://www.sublimetext.com/docs/settings.html#syntax-specific-settings
            let user = format!("Packages/User/{syntax}.sublime-settings");
            let alias = format!("Packages/User/{display}.sublime-settings");
            let user = data.join(
                if self.workspace.resources.get(&user).is_none()
                    && self.workspace.resources.get(&alias).is_some()
                {
                    alias
                } else {
                    user
                },
            );
            args = json!({"base_file":"Packages/Default/Preferences.sublime-settings","user_file":user,"default":"{\n\t$0\n}\n"});
        }
        if matches!(name, "customize_color_scheme" | "customize_theme") {
            let scheme = name == "customize_color_scheme";
            let key = if scheme { "color_scheme" } else { "theme" };
            let settings = &self.workspace.editor().settings;
            let auto = settings.string(key, "auto") == "auto";
            let active = if auto {
                match (scheme, self.workspace.dark) {
                    (true, true) => {
                        settings.string("dark_color_scheme", "Velocity Dark.sublime-color-scheme")
                    }
                    (true, false) => {
                        settings.string("light_color_scheme", "Velocity Light.sublime-color-scheme")
                    }
                    (false, true) => settings.string("dark_theme", "Velocity Dark.sublime-theme"),
                    (false, false) => {
                        settings.string("light_theme", "Velocity Light.sublime-theme")
                    }
                }
            } else {
                settings.string(key, "")
            }
            .to_owned();
            let catalog = if scheme {
                &self.workspace.appearance.schemes
            } else {
                &self.workspace.appearance.themes
            };
            let base = catalog
                .iter()
                .find(|r| r.path == active || r.value == active)
                .map(|r| r.path.clone())
                .ok_or_else(|| format!("Unknown {key}: {active}"))?;
            // The documented matching-filename merge is specifically for JSON
            // schemes; XML customization/conversion behavior is unconfirmed.
            // https://www.sublimetext.com/docs/color_schemes.html#customization
            if scheme && !base.ends_with(".sublime-color-scheme") {
                self.panels.message = "Customization requires a .sublime-color-scheme file".into();
                return Ok(true);
            }
            // Customization merges matching filenames; templates are authored here.
            // https://www.sublimetext.com/docs/color_schemes.html#customization
            // https://www.sublimetext.com/docs/themes.html#customization
            // Build 4107 adds the active-scheme/theme customization commands:
            // https://www.sublimetext.com/download
            let template = if scheme {
                "{\n    \"variables\": {},\n    \"globals\": {},\n    \"rules\": []\n}\n"
            } else {
                "{\n    \"variables\": {},\n    \"rules\": []\n}\n"
            };
            args = json!({"base_file":base,"default":template});
            self.panels.message = format!(
                "Customizing {active}{}",
                if auto {
                    if self.workspace.dark {
                        " (dark variant)"
                    } else {
                        " (light variant)"
                    }
                } else {
                    ""
                }
            );
        }
        let workspace = settings_workspace(&data, &args, &self.workspace.resources)?;
        let mut options = self.options.clone();
        options.files.clear();
        options.file = None;
        options.project = None;
        let mut app = App::new(
            options.clone(),
            crate::report::Clock::new(std::time::Instant::now(), std::time::SystemTime::now()),
            self.reporter.clone(),
            workspace,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            self.sessions.clone(),
        );
        if matches!(name, "customize_color_scheme" | "customize_theme") {
            app.panels.message = self.panels.message.clone();
        }
        if let Some(window) = window {
            window.new_window(
                WindowOptions {
                    title: "Settings — Velocity".into(),
                    size: Size {
                        width: options.width as f64,
                        height: options.height as f64,
                    },
                },
                app,
            )?;
        } else {
            self.opened_settings.push(app);
        }
        Ok(true)
    }
}
