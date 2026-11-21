//! Application release actions. Disk, settings and HTTPS work share one serial queue.
use crate::{
    panels::Panels,
    ui_text::{Dialog, UiText},
    workspace::Workspace,
};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{OnceLock, mpsc},
};
use vel_config::{
    Value, json,
    keymap::Invocation,
    settings::Settings,
    ui::{MenuItem, PaletteItem},
};
use vel_packages::{
    PackageManager, ResourceKind, Support,
    cli::{CliInstallOutcome, CliInstallPlan, install_cli, install_directories},
    https::HttpsClient,
};
use vel_platform::WindowHandle;

type Job = Box<dyn FnOnce() + Send>;
/// Serializes every app preferences writer with package enable/disable, across windows.
pub(crate) fn enqueue<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<mpsc::Receiver<Result<T, String>>, String> {
    static QUEUE: OnceLock<Result<mpsc::Sender<Job>, String>> = OnceLock::new();
    let queue = QUEUE
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Job>();
            std::thread::Builder::new()
                .name("velocity-release-settings".into())
                .spawn(move || {
                    for job in rx {
                        job();
                    }
                })
                .map_err(|e| e.to_string())?;
            Ok(tx)
        })
        .as_ref()
        .map_err(Clone::clone)?;
    let (tx, rx) = mpsc::channel();
    queue
        .send(Box::new(move || {
            let _ = tx.send(job());
        }))
        .map_err(|e| e.to_string())?;
    Ok(rx)
}

pub fn bundle_resources() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let contents = exe.parent()?.parent()?;
    let resources = contents.join("Resources");
    (contents.file_name()? == "Contents" && resources.is_dir()).then_some(resources)
}
fn bundle_for_exe(exe: &Path) -> Result<PathBuf, String> {
    let contents = exe
        .parent()
        .and_then(Path::parent)
        .ok_or("executable is not in Velocity.app")?;
    let bundle = contents
        .parent()
        .ok_or("executable is not in Velocity.app")?;
    if contents.file_name() != Some(std::ffi::OsStr::new("Contents"))
        || bundle.extension().is_none_or(|s| s != "app")
    {
        return Err("Install Command Line Tool requires the packaged Velocity.app. Run scripts/package-macos.sh first.".into());
    }
    Ok(bundle.into())
}
fn choice(caption: String, command: &str, args: Value) -> PaletteItem {
    PaletteItem {
        caption,
        invocation: Invocation {
            command: command.into(),
            args,
        },
    }
}
pub fn extend_commands(
    palette: &mut Vec<PaletteItem>,
    menus: &mut [MenuItem],
    settings: &Settings,
) {
    for (caption, command) in [
        ("Install Command Line Tool…", "install_cli"),
        ("Open Crash Reports", "open_crash_reports"),
        ("Package: Install from File…", "package_install_file"),
        ("Package: Install from URL…", "package_install_url"),
        ("Package: Remove", "package_remove"),
        ("Package: List", "package_list"),
    ] {
        palette.push(choice(caption.into(), command, json!({})));
    }
    for (caption, enabled) in [("Package: Disable", false), ("Package: Enable", true)] {
        palette.push(choice(
            caption.into(),
            "package_toggle",
            json!({"enabled":enabled}),
        ));
    }
    if settings
        .get("package_index_url")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty())
    {
        palette.push(choice(
            "Package: Check for Upgrades".into(),
            "package_check_upgrades",
            json!({}),
        ));
    }
    if let Some(help) = menus.iter_mut().find(|m| m.id == "help") {
        help.children.push(
            serde_json::from_value(
                json!({"caption":"Open Crash Reports","command":"open_crash_reports"}),
            )
            .expect("menu"),
        );
    }
}

#[derive(Clone, Debug)]
pub struct CrashStartup {
    pub reporter: vel_crash::CrashReporter,
    pub pending: Vec<vel_crash::Report>,
}
impl CrashStartup {
    pub fn install(home: &Path) -> Result<Self, String> {
        let reporter = vel_crash::CrashReporter::new(vel_crash::Config::for_home(
            home,
            env!("CARGO_PKG_VERSION"),
        ))
        .map_err(|e| e.to_string())?;
        let pending = reporter.pending_reports().map_err(|e| e.to_string())?;
        reporter.install_panic_hook();
        #[cfg(unix)]
        reporter
            .install_signal_handlers()
            .map_err(|e| e.to_string())?;
        reporter.record(vel_crash::Level::Info, "application startup");
        Ok(Self { reporter, pending })
    }
}
#[derive(Debug)]
enum Reply {
    Choices(Vec<PaletteItem>),
    Changed(UiText),
    Message(UiText),
    CliPlans(Vec<CliInstallPlan>),
}
#[derive(Debug, Default)]
pub struct Actions {
    rx: Option<mpsc::Receiver<Result<Reply, String>>>,
    pub crash: Option<CrashStartup>,
}
impl Actions {
    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }
    fn start(
        &mut self,
        job: impl FnOnce() -> Result<Reply, String> + Send + 'static,
    ) -> Result<(), String> {
        if self.busy() {
            return Err("A package or CLI operation is still running".into());
        }
        self.rx = Some(enqueue(job)?);
        Ok(())
    }
    pub fn action(
        &mut self,
        workspace: &mut Workspace,
        panels: &mut Panels,
        w: &WindowHandle,
        name: &str,
        args: &Value,
        viewport: std::ops::Range<usize>,
    ) -> Result<bool, String> {
        match name {
            "open_crash_reports" => {
                let folder = self
                    .crash
                    .as_ref()
                    .map(|c| c.reporter.directory().to_owned())
                    .or_else(|| {
                        std::env::var_os("HOME")
                            .map(|h| PathBuf::from(h).join("Library/Logs/Velocity"))
                    })
                    .ok_or("home directory unavailable")?;
                self.start(move || {
                    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
                    open_folder(&folder)?;
                    Ok(Reply::Message(UiText::tr(
                        "Opened local crash reports folder",
                    )))
                })?;
            }
            "install_cli" => {
                let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                let home = std::env::var_os("HOME").ok_or("home directory unavailable")?;
                self.start(move || {
                    let bundle = bundle_for_exe(&exe)?;
                    let [system, local] = install_directories(Path::new(&home));
                    let mut dirs = vec![local];
                    if directory_writable(&system) {
                        dirs.insert(0, system);
                    } else {
                        dirs.push(system);
                    }
                    let plans = dirs
                        .iter()
                        .map(|dir| {
                            CliInstallPlan::for_bundle(&bundle, dir).map_err(|e| e.to_string())
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(Reply::CliPlans(plans))
                })?;
            }
            "package_install_url" if args["url"].as_str().or(args["text"].as_str()).is_none() => {
                panels.show_input_caption(
                    workspace,
                    "show_input_panel",
                    &json!({"on_done":{"command":"package_install_url","args":{}}}),
                    UiText::tr("HTTPS URL of a .sublime-package archive"),
                    viewport,
                )?;
            }
            "package_install_url"
                if args["name"].as_str().is_none()
                    && (args["url"].as_str().is_none() || args["text"].as_str().is_none()) =>
            {
                let url = args["url"]
                    .as_str()
                    .or(args["text"].as_str())
                    .ok_or("missing HTTPS URL")?;
                let guess = url
                    .split('?')
                    .next()
                    .unwrap_or(url)
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .trim_end_matches(".sublime-package");
                panels.show_input_caption(workspace, "show_input_panel",
                    &json!({"initial_text":guess,"on_done":{"command":"package_install_url","args":{"url":url}}}),
                    UiText::tr("Package name"), viewport)?;
            }
            "package_list" if args["detail"].as_str().is_some() => {
                Dialog::new(
                    UiText::tr("Package contributions"),
                    args["detail"].as_str().unwrap_or(""),
                    [UiText::tr("OK")],
                )
                .show(&workspace.translations, w);
            }
            "package_install_file"
            | "package_install_url"
            | "package_list"
            | "package_remove"
            | "package_toggle"
            | "package_check_upgrades"
            | "package_upgrade" => {
                let data = workspace
                    .data_path()
                    .ok_or("package actions require a data directory")?
                    .to_owned();
                let mut args = args.clone();
                if name == "package_install_url"
                    && args["name"].as_str().is_none()
                    && args["url"].as_str().is_some()
                {
                    args["name"] = args["text"].clone();
                }
                if name == "package_install_file" && args["file"].as_str().is_none() {
                    let Some(file) = w.open_files().into_iter().next() else {
                        return Ok(true);
                    };
                    args["file"] = json!(file);
                }
                if name == "package_install_file"
                    && args["name"].as_str().is_none()
                    && let Some(source) = args["file"].as_str()
                {
                    args["name"] = json!(
                        Path::new(source)
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("Package")
                    );
                }
                if name == "package_remove"
                    && args["name"].as_str().is_some()
                    && Dialog::new(
                        UiText::tr("Remove package?"),
                        UiText::tr_fmt(
                            "Move {name} and its loose overrides to recoverable backups?",
                            [("name", args["name"].as_str().unwrap_or("").into())],
                        ),
                        [UiText::tr("Cancel"), UiText::tr("Remove")],
                    )
                    .show(&workspace.translations, w)
                        != 1
                {
                    return Ok(true);
                }
                if name == "package_upgrade" && Dialog::new(
                    UiText::tr("Upgrade package?"),
                    UiText::tr_fmt("Install {name} {version} from {url}? The previous archive will be backed up; disabled packages stay disabled.",
                        ["name", "version", "url"].map(|key| (key, args["release"][key].as_str().unwrap_or("").into()))),
                    [UiText::tr("Cancel"), UiText::tr("Install")],
                ).show(&workspace.translations, w) != 1 {
                    return Ok(true);
                }
                let name = name.to_owned();
                let ignored = workspace.ignored_packages();
                let index = workspace
                    .settings
                    .get("package_index_url")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let tr = workspace.translations.clone();
                self.start(move || {
                    localized_package_job(&data, &name, &args, ignored, index, &tr)
                })?;
                panels.set_message(UiText::tr("Working on packages…"));
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    pub fn poll(
        &mut self,
        workspace: &mut Workspace,
        panels: &mut Panels,
        w: &WindowHandle,
    ) -> bool {
        if !self.busy()
            && let Some(crash) = &mut self.crash
            && !crash.pending.is_empty()
        {
            let reports = std::mem::take(&mut crash.pending);
            let reporter = crash.reporter.clone();
            let open = Dialog::new(
                UiText::tr("Velocity found a crash report"),
                UiText::tr_fmt("{count} local report(s) from a previous launch. No report is uploaded.\n{path}",
                    [("count", reports.len().to_string()), ("path", reporter.directory().display().to_string())]),
                [UiText::tr("Open Report Folder"), UiText::tr("Dismiss")],
            ).show(&workspace.translations, w) == 0;
            if let Err(error) = self.start(move || {
                if open {
                    open_folder(reporter.directory())?;
                }
                for report in reports {
                    reporter.acknowledge(&report).map_err(|e| e.to_string())?;
                }
                Ok(Reply::Message(UiText::tr(
                    "Crash report prompt dismissed; reports retained locally",
                )))
            }) {
                panels.message = error.into();
            }
        }
        let Some(result) = self.rx.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(r) => Some(r),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(e) => Some(Err(e.to_string())),
        }) else {
            return false;
        };
        self.rx = None;
        match result {
            Ok(Reply::Choices(items)) => {
                if items.is_empty() {
                    panels.set_message(UiText::tr("No packages or upgrades found"));
                } else {
                    panels.choices(workspace, items);
                }
            }
            Ok(Reply::Changed(message)) => {
                workspace.request_resource_reload();
                panels.set_message(message);
            }
            Ok(Reply::Message(message)) => panels.set_message(message),
            Ok(Reply::CliPlans(plans)) => {
                let Some(first) = plans.first() else {
                    return true;
                };
                let detail = UiText::tr_fmt(
                    "Source: {source}\nDestination: {destination}\n\nAlternative: {alternative}\n\nThe user-local directory may need to be added to PATH. Existing commands are never overwritten. No sudo is used.",
                    [
                        ("source", first.launcher().display().to_string()),
                        ("destination", first.link().display().to_string()),
                        (
                            "alternative",
                            plans
                                .get(1)
                                .map(|p| p.link().display().to_string())
                                .unwrap_or_default(),
                        ),
                    ],
                );
                let selected = Dialog::new(
                    UiText::tr("Install Command Line Tool…"),
                    detail,
                    [
                        UiText::tr("Cancel"),
                        UiText::tr("Install at Destination"),
                        UiText::tr("Use Alternative…"),
                    ],
                )
                .show(&workspace.translations, w);
                let plan = match selected {
                    1 => plans.first(),
                    2 => plans.get(1),
                    _ => None,
                }
                .cloned();
                if let Some(plan) = plan {
                    let confirmed = selected == 1
                        || Dialog::new(
                            UiText::tr("Confirm command line tool destination"),
                            UiText::tr_fmt(
                                "{source} → {destination}\nAdd {directory} to PATH if needed.",
                                [
                                    ("source", plan.launcher().display().to_string()),
                                    ("destination", plan.link().display().to_string()),
                                    (
                                        "directory",
                                        plan.link()
                                            .parent()
                                            .unwrap_or(Path::new(""))
                                            .display()
                                            .to_string(),
                                    ),
                                ],
                            ),
                            [UiText::tr("Cancel"), UiText::tr("Install")],
                        )
                        .show(&workspace.translations, w)
                            == 1;
                    if confirmed && let Err(error) = self.start(move || {
                        install_cli(&plan, |_| true).map(|outcome| Reply::Message(match outcome {
                            CliInstallOutcome::Declined => UiText::tr("Command line tool installation cancelled"),
                            CliInstallOutcome::AlreadyInstalled => UiText::tr_fmt("Command line tool is already installed at {path}", [("path", plan.link().display().to_string())]),
                            CliInstallOutcome::Installed(path) => UiText::tr_fmt("Installed command line tool at {path}. Add {directory} to PATH if needed.",
                                [("path", path.display().to_string()), ("directory", path.parent().unwrap_or(Path::new("")).display().to_string())]),
                        })).map_err(|e| e.to_string())
                    }) {
                        panels.set_message(UiText::tr_fmt("Could not install the command line tool: {detail}", [("detail", error)]));
                    }
                }
            }
            Err(error) => {
                panels.note_console(workspace, &error);
                panels.set_message(UiText::tr_fmt(
                    "Package or release action failed: {detail}",
                    [("detail", error)],
                ));
            }
        }
        true
    }
}
fn directory_writable(path: &Path) -> bool {
    let mut candidate = path;
    while !candidate.exists() {
        let Some(parent) = candidate.parent() else {
            return false;
        };
        candidate = parent;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(path) = std::ffi::CString::new(candidate.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: path is a live NUL-terminated pathname; access only queries permissions.
        unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}
fn open_folder(path: &Path) -> Result<(), String> {
    std::process::Command::new("/usr/bin/open")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}
#[cfg(test)]
fn package_job(
    data: &Path,
    command: &str,
    args: &Value,
    ignored: BTreeSet<String>,
    index: Option<String>,
) -> Result<Reply, String> {
    localized_package_job(data, command, args, ignored, index, &Default::default())
}

fn localized_package_job(
    data: &Path,
    command: &str,
    args: &Value,
    ignored: BTreeSet<String>,
    index: Option<String>,
    tr: &vel_i18n::Translator,
) -> Result<Reply, String> {
    let disabled_suffix = tr.tr(" (disabled)");
    let unknown_version = tr.tr("unknown version");
    let mut manager = PackageManager::new(data);
    let client = HttpsClient::default();
    let result = match command {
        "package_install_file" | "package_install_url" => {
            let source = args["file"]
                .as_str()
                .or(args["url"].as_str())
                .or(args["text"].as_str())
                .ok_or("missing archive path or URL")?;
            let name = args["name"]
                .as_str()
                .or_else(|| suggested_package_name(source))
                .ok_or("archive must end in .sublime-package or .zip, or supply a name")?;
            let installed = if command == "package_install_file" {
                manager.install_local(name, Path::new(source))
            } else {
                manager.install_https(name, source, &client)
            }
            .map_err(|e| e.to_string())?;
            Reply::Changed(
                UiText::tr_fmt(
                    "Installed {path}. Backup: {backup}",
                    [
                        ("path", installed.path.display().to_string()),
                        ("backup", backup_text(installed.backup.as_deref())),
                    ],
                )
                .with_raw_suffix(&warnings_text(&installed.warnings)),
            )
        }
        "package_remove" if args["name"].as_str().is_some() => {
            let backups = manager
                .remove(args["name"].as_str().unwrap_or(""))
                .map_err(|e| e.to_string())?;
            Reply::Changed(UiText::tr_fmt(
                "Removed package. Backups: {paths}",
                [(
                    "paths",
                    backups
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                )],
            ))
        }
        "package_toggle" if args["name"].as_str().is_some() => {
            let name = args["name"].as_str().unwrap_or("");
            let enabled = args["enabled"].as_bool().unwrap_or(ignored.contains(name));
            let backup = manager
                .set_enabled_inheriting(name, enabled, &ignored)
                .map_err(|e| e.to_string())?;
            let arguments = [
                ("name", name.to_owned()),
                ("backup", backup_text(backup.as_deref())),
            ];
            Reply::Changed(if enabled {
                UiText::tr_fmt("Enabled {name}. Preferences backup: {backup}", arguments)
            } else {
                UiText::tr_fmt("Disabled {name}. Preferences backup: {backup}", arguments)
            })
        }
        "package_check_upgrades" => {
            let url = index
                .filter(|s| !s.is_empty())
                .ok_or("Configure package_index_url before checking for upgrades")?;
            manager
                .set_index_url(Some(url))
                .map_err(|e| e.to_string())?;
            let upgrades = manager.check_upgrades(&client).map_err(|e| e.to_string())?;
            Reply::Choices(
                upgrades
                    .into_iter()
                    .map(|u| {
                        choice(
                            format!(
                                "{}: {} → {}{}",
                                u.available.name,
                                u.installed_version
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| unknown_version.to_string()),
                                u.available.version,
                                if ignored.contains(&u.available.name) {
                                    &disabled_suffix
                                } else {
                                    ""
                                }
                            ),
                            "package_upgrade",
                            json!({"release":u.available}),
                        )
                    })
                    .collect(),
            )
        }
        "package_upgrade" => {
            let release =
                serde_json::from_value(args["release"].clone()).map_err(|e| e.to_string())?;
            let installed = manager
                .install_release(&release, &client)
                .map_err(|e| e.to_string())?;
            Reply::Changed(
                UiText::tr_fmt(
                    "Upgraded {path}. Backup: {backup}",
                    [
                        ("path", installed.path.display().to_string()),
                        ("backup", backup_text(installed.backup.as_deref())),
                    ],
                )
                .with_raw_suffix(&warnings_text(&installed.warnings)),
            )
        }
        _ => {
            let packages = manager.list().map_err(|e| e.to_string())?;
            // The engine inventory covers seven resource families. Supplement
            // app-owned menu/palette/mouse/metadata resources without changing it.
            let mut resources = vel_config::resources::Resources::default();
            let _warnings = resources.load_with_warnings(data);
            let mut known: BTreeSet<_> = packages.iter().map(|p| p.name.clone()).collect();

            let mut choices = packages
                .into_iter()
                .filter(|p| p.name != "User" || command == "package_list")
                .filter(|p| {
                    command != "package_toggle"
                        || args["enabled"]
                            .as_bool()
                            .is_none_or(|enable| enable == ignored.contains(&p.name))
                })
                .map(|p| {
                    let disabled = ignored.contains(&p.name);
                    let prefix = format!("Packages/{}/", p.name);
                    let extra = [
                        (".sublime-menu", tr.tr("Menu")),
                        (".sublime-commands", tr.tr("Command palette")),
                        (".sublime-mousemap", tr.tr("Mouse bindings")),
                        (".tmPreferences", tr.tr("Metadata")),
                        (".hidden-color-scheme", tr.tr("Color scheme")),
                    ]
                    .into_iter()
                    .flat_map(|(suffix, kind)| {
                        resources
                            .matching(suffix)
                            .into_iter()
                            .filter_map(|r| {
                                r.path
                                    .strip_prefix(&prefix)
                                    .map(|path| inventory_resource(tr, &kind, path))
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>();

                    let support = match p.support() {
                        Support::Declarative => tr.tr("declarative resources supported"),
                        Support::PartialNoPluginApi => tr.tr("partially supported: no plugin API"),
                    };
                    let detail = format!(
                        "{} {} — {}\n{}\n{}",
                        p.name,
                        p.version
                            .as_ref()
                            .map(ToString::to_string)
                            .unwrap_or_else(|| unknown_version.to_string()),
                        if disabled {
                            tr.tr("disabled")
                        } else {
                            tr.tr("enabled")
                        },
                        support,
                        p.contributions
                            .iter()
                            .map(|c| inventory_resource(tr, &resource_kind(tr, c.kind), &c.path))
                            .chain(extra.iter().cloned())
                            .chain(p.python_plugins.iter().map(|p| inventory_resource(
                                tr,
                                &tr.tr("Python (not executed)"),
                                p
                            )))
                            .chain(p.error.iter().cloned())
                            .collect::<Vec<_>>()
                            .join("\n")
                    );
                    let caption = tr.tr_fmt(
                        "{name}{state} — {count} resource(s); {support}",
                        &[
                            ("name", p.name.clone()),
                            (
                                "state",
                                if disabled {
                                    disabled_suffix.to_string()
                                } else {
                                    String::new()
                                },
                            ),
                            ("count", (p.contributions.len() + extra.len()).to_string()),
                            ("support", p.error.as_deref().unwrap_or(&support).to_owned()),
                        ],
                    );
                    choice(
                        caption,
                        command,
                        if command == "package_list" {
                            json!({"detail":detail})
                        } else {
                            json!({"name":p.name,"enabled":disabled})
                        },
                    )
                })
                .collect::<Vec<_>>();
            if command == "package_toggle" {
                let mut bundled = vel_config::resources::Resources::default();
                crate::shipped::register(&crate::language::packages_root(), &mut bundled);
                let plugins = bundled.plugins();
                let shipped = std::fs::read_dir(crate::language::packages_root())
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
                    .filter_map(|entry| entry.file_name().into_string().ok());
                let extra: BTreeSet<_> = shipped
                    .chain(std::iter::once("Vintage".into()))
                    .chain(ignored.iter().cloned())
                    .collect();
                for name in extra
                    .into_iter()
                    .filter(|p| p != "User" && known.insert(p.clone()))
                    // Shipped and ignored packages follow the same rule as
                    // installed ones: a toggle picker only offers what the
                    // requested direction would actually change.
                    .filter(|name| {
                        args["enabled"]
                            .as_bool()
                            .is_none_or(|enable| enable == ignored.contains(name))
                    })
                {
                    let disabled = ignored.contains(&name);
                    let support =
                        plugins
                            .get(name.as_str())
                            .map_or_else(String::new, |(count, _)| {
                                tr.tr_fmt(
                                    "; {count} Python files not run",
                                    &[("count", count.to_string())],
                                )
                            });
                    choices.push(choice(
                        format!(
                            "{name}{}{support}",
                            if disabled { &disabled_suffix } else { "" }
                        ),
                        command,
                        json!({"name":name,"enabled":disabled}),
                    ));
                }
            }
            Reply::Choices(choices)
        }
    };
    Ok(result)
}
fn resource_kind(tr: &vel_i18n::Translator, kind: ResourceKind) -> std::borrow::Cow<'static, str> {
    match kind {
        ResourceKind::Syntax => tr.tr("Syntax"),
        ResourceKind::ColorScheme => tr.tr("Color scheme"),
        ResourceKind::Theme => tr.tr("Theme"),
        ResourceKind::Keymap => tr.tr("Key Bindings"),
        ResourceKind::Snippet => tr.tr("Snippet"),
        ResourceKind::Completions => tr.tr("Completions"),
        ResourceKind::BuildSystem => tr.tr("Build System"),
    }
}

fn inventory_resource(tr: &vel_i18n::Translator, kind: &str, path: &str) -> String {
    tr.tr_fmt(
        "{kind}: {path}",
        &[("kind", kind.to_owned()), ("path", path.to_owned())],
    )
}

/// Offline inventory used by the same scene fixtures as other quick panels.
#[cfg(target_os = "macos")]
pub(crate) fn inventory_scene(workspace: &Workspace) -> Result<Vec<PaletteItem>, String> {
    let Reply::Choices(rows) = localized_package_job(
        workspace.data_path().ok_or("missing fixture data")?,
        "package_list",
        &json!({}),
        workspace.ignored_packages(),
        None,
        &workspace.translations,
    )?
    else {
        unreachable!("inventory always produces choices")
    };
    Ok(rows)
}
fn suggested_package_name(source: &str) -> Option<&str> {
    let filename = source.split(['?', '#']).next()?.rsplit('/').next()?;
    let name = filename
        .strip_suffix(".sublime-package")
        .or_else(|| filename.strip_suffix(".zip"))?;
    Some(
        name.strip_suffix("-main")
            .or_else(|| name.strip_suffix("-master"))
            .unwrap_or(name),
    )
}

fn warnings_text(warnings: &[String]) -> String {
    if warnings.is_empty() {
        String::new()
    } else {
        format!(". {}", warnings.join("; "))
    }
}
fn backup_text(path: Option<&Path>) -> String {
    path.map(|p| p.display().to_string())
        .unwrap_or_else(|| "none (first installation)".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fresh_profile_can_enable_shipped_vintage_and_ignored_packages() {
        let temp = tempfile::tempdir().unwrap();
        let ignored = BTreeSet::from(["Vintage".to_owned(), "Not installed".to_owned()]);
        let Reply::Choices(rows) = package_job(
            temp.path(),
            "package_toggle",
            &json!({}),
            ignored.clone(),
            None,
        )
        .unwrap() else {
            panic!("expected package choices");
        };
        assert!(
            rows.iter()
                .any(|row| row.invocation.args["name"] == "Vintage"
                    && row.invocation.args["enabled"] == true)
        );
        assert!(
            rows.iter()
                .any(|row| row.invocation.args["name"] == "Not installed")
        );
        assert!(
            rows.iter()
                .any(|row| row.invocation.args["name"] == "Python")
        );
        package_job(
            temp.path(),
            "package_toggle",
            &json!({"name":"Vintage","enabled":true}),
            ignored,
            None,
        )
        .unwrap();
        assert!(
            !PackageManager::new(temp.path())
                .ignored_packages()
                .unwrap()
                .contains("Vintage")
        );
    }
    #[test]
    fn package_enable_and_disable_offer_only_applicable_packages() {
        let data = tempfile::tempdir().unwrap();
        for name in ["Active", "Inactive"] {
            let dir = data.path().join("Packages").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("fixture.sublime-build"), "{}").unwrap();
        }
        for (enabled, expected) in [(true, "Inactive"), (false, "Active")] {
            let reply = package_job(
                data.path(),
                "package_toggle",
                &json!({"enabled": enabled}),
                ["Inactive".into()].into(),
                None,
            )
            .unwrap();
            let Reply::Choices(items) = reply else {
                panic!("expected a package picker");
            };
            // Shipped packages are real, disable-able packages, so the picker
            // may also offer them; what matters is that every row would
            // actually change state in the requested direction.
            assert!(
                items
                    .iter()
                    .all(|item| item.invocation.args["enabled"] == json!(enabled))
            );
            let names: Vec<_> = items
                .iter()
                .map(|item| item.invocation.args["name"].as_str().unwrap_or(""))
                .collect();
            assert!(names.contains(&expected), "{enabled}: {names:?}");
            assert!(!names.contains(&if enabled { "Active" } else { "Inactive" }));
        }
    }
    #[test]
    fn list_reports_plugins_and_effective_disabled_state_without_network_or_execution() {
        let temp = tempfile::tempdir().unwrap();
        let loose = temp.path().join("Packages/Fixture");
        std::fs::create_dir_all(&loose).unwrap();
        std::fs::write(loose.join("plugin.py"), "raise Exception('not executed')").unwrap();
        std::fs::write(loose.join("thing.sublime-build"), "{\"cmd\":[\"true\"]}").unwrap();
        let reply = package_job(
            temp.path(),
            "package_list",
            &json!({}),
            ["Fixture".into()].into(),
            None,
        )
        .unwrap();
        let Reply::Choices(items) = reply else {
            panic!("expected choices");
        };
        assert_eq!(items.len(), 1);
        assert!(items[0].caption.contains("no plugin API"));
        assert!(items[0].caption.contains("disabled"));
        assert!(
            items[0].invocation.args["detail"]
                .as_str()
                .unwrap()
                .contains("Build System: thing.sublime-build")
        );
        assert!(
            package_job(
                temp.path(),
                "package_check_upgrades",
                &json!({}),
                BTreeSet::new(),
                None
            )
            .unwrap_err()
            .contains("Configure package_index_url")
        );
    }
    #[test]
    fn menu_only_packages_report_their_app_contributions() {
        let temp = tempfile::tempdir().unwrap();
        let loose = temp.path().join("Packages/Menus");
        std::fs::create_dir_all(&loose).unwrap();
        std::fs::write(loose.join("Main.sublime-menu"), "[]").unwrap();
        std::fs::write(loose.join("Custom.sublime-commands"), "[]").unwrap();
        let Reply::Choices(items) = package_job(
            temp.path(),
            "package_list",
            &json!({}),
            BTreeSet::new(),
            None,
        )
        .unwrap() else {
            panic!("expected inventory");
        };
        assert!(items[0].caption.contains("2 resource(s)"));
        let detail = items[0].invocation.args["detail"].as_str().unwrap();
        assert!(detail.contains("Menu: Main.sublime-menu"));
        assert!(detail.contains("Command palette: Custom.sublime-commands"));
    }

    #[test]
    fn package_inventory_localizes_app_copy_but_not_resource_names() {
        let temp = tempfile::tempdir().unwrap();
        let package = temp.path().join("Packages/Example");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("example.py"), "# not run").unwrap();
        std::fs::write(package.join("Example.sublime-commands"), "[]").unwrap();
        let mut tr = vel_i18n::Translator::default();
        let mut catalogs = vel_i18n::Catalogs::default();
        for (tag, catalog) in crate::ui_language::shipped_catalogs() {
            catalogs.insert(tag, catalog);
        }
        tr.replace(catalogs);
        tr.select(tr.resolve("zh-Hans").unwrap());
        let Reply::Choices(rows) = localized_package_job(
            temp.path(),
            "package_list",
            &json!({}),
            ["Example".into()].into(),
            None,
            &tr,
        )
        .unwrap() else {
            panic!("expected package inventory")
        };
        assert_eq!(
            rows[0].caption,
            "Example（已停用）：1 个资源；部分支持：不支持插件 API"
        );
        let detail = rows[0].invocation.args["detail"].as_str().unwrap();
        assert!(detail.contains("Example 版本未知 — 已停用"));
        assert!(detail.contains("命令面板：Example.sublime-commands"));
        assert!(detail.contains("Python（不执行）：example.py"));
    }
    #[test]
    fn global_worker_orders_preferences_writes_before_package_edits() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().to_owned();
        let first = data.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let writer = enqueue(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            let user = first.join("Packages/User");
            std::fs::create_dir_all(&user).unwrap();
            std::fs::write(
                user.join("Preferences.sublime-settings"),
                "{ // retained comment\n\"added_words\":[\"alienword\"],\"unrelated\":42}",
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
        started_rx
            .recv_timeout(std::time::Duration::from_secs(120))
            .unwrap();
        let second = enqueue(move || {
            PackageManager::new(data)
                .set_enabled_inheriting("Fixture", false, &["Vintage".into()].into())
                .map_err(|e| e.to_string())
        })
        .unwrap();
        assert!(matches!(second.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release_tx.send(()).unwrap();
        writer
            .recv_timeout(std::time::Duration::from_secs(120))
            .unwrap()
            .unwrap();
        second
            .recv_timeout(std::time::Duration::from_secs(120))
            .unwrap()
            .unwrap();
        let source = std::fs::read_to_string(
            temp.path()
                .join("Packages/User/Preferences.sublime-settings"),
        )
        .unwrap();
        assert!(source.contains("retained comment") && source.contains("alienword"));
        let doc = vel_config::json::Document::parse(&source).unwrap();
        assert_eq!(doc.value["unrelated"], 42);
        let ignored = PackageManager::new(temp.path()).ignored_packages().unwrap();
        assert!(ignored.contains("Vintage") && ignored.contains("Fixture"));
    }
    #[test]
    fn bundle_plan_never_guesses_a_launcher_for_a_cargo_binary() {
        assert!(bundle_for_exe(Path::new("/tmp/target/release/velocity")).is_err());
        assert_eq!(
            bundle_for_exe(Path::new(
                "/Applications/Velocity.app/Contents/MacOS/velocity"
            ))
            .unwrap(),
            Path::new("/Applications/Velocity.app")
        );
    }
}
#[cfg(test)]
mod dbg_package {
    use super::*;
    #[test]
    fn dbg_toggle_names() {
        let data = tempfile::tempdir().unwrap();
        for name in ["Active", "Inactive"] {
            let dir = data.path().join("Packages").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("fixture.sublime-build"), "{}").unwrap();
        }
        let reply = package_job(
            data.path(),
            "package_toggle",
            &json!({"enabled": true}),
            ["Inactive".into()].into(),
            None,
        )
        .unwrap();
        let Reply::Choices(items) = reply else {
            panic!()
        };
        eprintln!("items={}", items.len());
        for i in items.iter().take(8) {
            eprintln!("{:?} | {:?}", i.caption, i.invocation.args);
        }
    }
}
