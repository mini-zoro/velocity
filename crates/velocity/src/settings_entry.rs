//! Settings windows built from our resources and User files, through the normal editor/loaders.
use crate::{
    app::App, cli::FileLocation, ui_text::UiText, workbench::restore_settings_tab,
    workspace::Workspace,
};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};
use vel_config::{Value, json, resources::Resources};
use vel_platform::{Size, WindowHandle, WindowOptions};
use vel_text::Text;

pub(super) fn preferences_json_args() -> Value {
    json!({"base_file": "Packages/Default/Preferences.sublime-settings", "default": "{\n\t$0\n}\n"})
}

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
fn syntax_settings_args(
    data: &Path,
    name: &str,
    syntax_path: &str,
    stem: &str,
    resources: &Resources,
) -> Result<Value, String> {
    // https://www.sublimetext.com/docs/settings.html#syntax-specific-settings
    // The User file uses the grammar's file stem, as Sublime does; the package default
    // is looked up by display name first, then by the same stem.
    let file = format!("{name}.sublime-settings");
    let stem_file = format!("{stem}.sublime-settings");
    let user = data.join("Packages/User").join(&stem_file);
    let package = syntax_path.split('/').nth(1).unwrap_or("Default");
    let preferred = format!("Packages/{package}/{file}");
    let source = resources
        .get(&preferred)
        .or_else(|| resources.get(&format!("Packages/{package}/{stem_file}")))
        .or_else(|| {
            resources.matching(&file).into_iter().find(|r| {
                !r.path.starts_with("Packages/User/")
                    && r.path.rsplit('/').next() == Some(file.as_str())
            })
        })
        .or_else(|| {
            resources.matching(&stem_file).into_iter().find(|r| {
                !r.path.starts_with("Packages/User/")
                    && r.path.rsplit('/').next() == Some(stem_file.as_str())
            })
        });
    Ok(json!({
        "base_file": source.map_or_else(|| format!("Generated/{file}"), |p| p.path.clone()),
        "_generated_syntax_defaults": source.is_none(), "user_file":user,
        "default":"{\n\t$0\n}\n"
    }))
}
/// Preferences left/default, right/User, as documented at settings.html. The
/// command's paths/variables follow reference/commands.html and the new-window
/// edit_settings example in the public community customization guide.
/// What an edit_settings invocation opens: the resolved base resource, its text, and the
/// User file it pairs with.
struct SettingsTarget {
    resource: String,
    contents: String,
    name: String,
    user: PathBuf,
}
fn settings_target(
    data: &Path,
    args: &Value,
    resources: &Resources,
) -> Result<SettingsTarget, String> {
    // https://www.sublimetext.com/docs/settings.html
    // https://docs.sublimetext.io/reference/commands.html
    let base = args["base_file"]
        .as_str()
        .ok_or("edit_settings needs base_file")?;
    let (resource, contents) = if args["_generated_syntax_defaults"] == true {
        (
            base.to_owned(),
            "// No package defaults for this syntax.\n{}\n".into(),
        )
    } else {
        source(base, data, resources)?
    };
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
    Ok(SettingsTarget {
        resource,
        contents,
        name,
        user,
    })
}
fn settings_workspace(
    data: &Path,
    args: &Value,
    target: SettingsTarget,
) -> Result<Workspace, String> {
    let SettingsTarget {
        resource,
        contents,
        name,
        user,
    } = target;
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
    /// Brings the window that already shows `user` to the front and focuses that file.
    fn focus_open_settings(
        &mut self,
        window: Option<&WindowHandle>,
        user: &Path,
    ) -> Result<bool, String> {
        self.publish_open_route();
        let target = {
            let sessions = self.sessions.borrow();
            sessions
                .open_windows
                .iter()
                .find(|(_, route)| {
                    route
                        .files
                        .iter()
                        .any(|file| crate::open_requests::same_path(file, user))
                })
                .map(|(id, _)| *id)
        };
        let Some(id) = target else {
            return Ok(false);
        };
        let mut request = crate::instance::Request::new(&crate::cli::Options::default(), None)?;
        request.native_open = true;
        request.files = vec![FileLocation::path_only(user.to_owned())];
        let incoming = crate::instance::Incoming::local(request);
        if id == self.session_id {
            match window {
                Some(window) => self.apply_open_request(window, incoming)?,
                None => self.accept_request(incoming)?,
            }
            return Ok(true);
        }
        let handle = {
            let mut sessions = self.sessions.borrow_mut();
            let handle = sessions
                .handles
                .get(&id)
                .filter(|h| !h.is_closed())
                .cloned();
            if handle.is_none() {
                return Ok(false);
            }
            sessions.open_inbox.entry(id).or_default().push(incoming);
            handle
        };
        if let Some(handle) = handle {
            handle.focus();
            handle.request_frame();
        }
        Ok(true)
    }
    pub(super) fn apply_settings_action(
        &mut self,
        window: Option<&WindowHandle>,
        name: &str,
        args: &Value,
    ) -> Result<bool, String> {
        if name == "show_settings" {
            self.show_settings_page(window);
            return Ok(true);
        }
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
            let path = language.as_ref().map_or("", |l| l.syntax.as_str());
            let mut args_value =
                syntax_settings_args(&data, display, path, syntax, &self.workspace.resources)?;
            // Prefer an existing display-name User file over an empty stem file.
            let user = format!("Packages/User/{syntax}.sublime-settings");
            let alias = format!("Packages/User/{display}.sublime-settings");
            if self.workspace.resources.get(&user).is_none()
                && self.workspace.resources.get(&alias).is_some()
            {
                args_value["user_file"] = json!(data.join(alias).to_string_lossy());
            }
            args = args_value;
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
                self.panels.set_message(UiText::tr(
                    "Customization requires a .sublime-color-scheme file",
                ));
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
            self.panels.set_message(if auto {
                if self.workspace.dark {
                    UiText::tr_fmt(
                        "Customizing {name} (dark variant)",
                        [("name", active.to_owned())],
                    )
                } else {
                    UiText::tr_fmt(
                        "Customizing {name} (light variant)",
                        [("name", active.to_owned())],
                    )
                }
            } else {
                UiText::tr_fmt("Customizing {name}", [("name", active.to_owned())])
            });
        }
        let target = settings_target(&data, &args, &self.workspace.resources)?;
        // Sublime Text 4215 (black box): invoking Settings again brings the open settings
        // window to the front instead of opening another one.
        if self.focus_open_settings(window, &target.user)? {
            return Ok(true);
        }
        let workspace = settings_workspace(&data, &args, target)?;
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
                    title: app.workspace.window_title(),
                    size: Size {
                        width: options.width as f64,
                        height: options.height as f64,
                    },
                    ..Default::default()
                },
                app,
            )?;
        } else {
            self.opened_settings.push(app);
        }
        Ok(true)
    }
}
