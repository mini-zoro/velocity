//! The app, not the sandboxed Finder extension, creates and opens the document.
use super::*;
use std::{fs, io};

const EXTENSION_ID: &str = "dev.velocity.Velocity.FinderNewFile";

impl App {
    pub(crate) fn handle_finder_new_text_file(
        &mut self,
        window: Option<&WindowHandle>,
        directory: &Path,
    ) {
        self.ensure_startup_services();
        let stem = self.workspace.translations.tr("New Text Document");
        let path = match create_text_file(directory, &stem) {
            Ok(path) => path,
            Err(error) => {
                eprintln!("velocity: could not create a Finder text file: {error}");
                self.status_error(UiText::tr_fmt(
                    "Could not create a text file in {path}: {detail}",
                    [
                        ("path", directory.display().to_string()),
                        ("detail", error.to_string()),
                    ],
                ));
                if let Some(window) = window {
                    window.request_frame();
                }
                return;
            }
        };
        // The native bridge already chose the current window. Use the ordinary
        // Finder tab loader without rerouting a freshly created file to another window.
        let result = crate::open_requests::native_request(&[path]).and_then(|request| {
            let incoming = crate::instance::Incoming::local(request);
            if let Some(window) = window {
                self.apply_open_request(window, incoming)
            } else {
                self.accept_request(incoming)?;
                self.named_changed(None, false);
                Ok(())
            }
        });
        if let Err(error) = result {
            self.named_error(window, &error);
        }
    }

    pub(super) fn finder_new_text_file(&mut self, window: &WindowHandle, directory: &Path) {
        if self.options.mode != Mode::Interactive {
            return;
        }
        if !window.is_closed() {
            self.handle_finder_new_text_file(Some(window), directory);
            return;
        }
        let result = self.window_app(None, None, false).and_then(|mut app| {
            app.handle_finder_new_text_file(None, directory);
            window.new_window(
                WindowOptions {
                    title: "Velocity".into(),
                    size: Size {
                        width: app.options.width as f64,
                        height: app.options.height as f64,
                    },
                },
                app,
            )
        });
        if let Err(error) = result {
            eprintln!("velocity: could not open a window for Finder New Text File: {error}");
        }
    }

    pub(super) fn set_finder_new_text_file_enabled(&mut self, enabled: bool) {
        let Some(extension) = Platform::finder_new_text_file_extension() else {
            self.status_error(UiText::tr("This copy of Velocity has no Finder extension"));
            return;
        };
        let result = (|| {
            if enabled {
                // The app may have been moved since packaging. Register this path
                // before changing the PluginKit preference for the extension ID.
                run_pluginkit(&["-a".as_ref(), extension.as_os_str()])?;
            }
            let election = if enabled { "use" } else { "ignore" };
            run_pluginkit(&["-e", election, "-i", EXTENSION_ID].map(std::ffi::OsStr::new))
        })();
        match result {
            Ok(()) => self.panels.set_message(if enabled {
                UiText::tr("Finder: New Text File enabled")
            } else {
                UiText::tr("Finder: New Text File disabled")
            }),
            Err(detail) => self.status_error(UiText::tr_fmt(
                "Could not change the Finder extension: {detail}",
                [("detail", detail)],
            )),
        }
    }
}

fn run_pluginkit(args: &[&std::ffi::OsStr]) -> Result<(), String> {
    let output = std::process::Command::new("/usr/bin/pluginkit")
        .args(args)
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    Err(if detail.is_empty() {
        output.status.to_string()
    } else {
        detail.to_owned()
    })
}

fn create_text_file(directory: &Path, stem: &str) -> io::Result<PathBuf> {
    if !directory.is_absolute() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let directory = directory.canonicalize()?;
    if !directory.metadata()?.is_dir() {
        return Err(io::ErrorKind::NotADirectory.into());
    }
    // Localization overrides are data, not paths.
    let stem = if stem.is_empty() || stem.contains(['/', '\0']) {
        "New Text Document"
    } else {
        stem
    };
    for number in 1..=u32::MAX {
        let name = if number == 1 {
            format!("{stem}.txt")
        } else {
            format!("{stem} ({number}).txt")
        };
        let path = directory.join(name);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::ErrorKind::AlreadyExists.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(language: &str) -> App {
        let options = Options::default();
        let now = Instant::now();
        let mut workspace = Workspace::new_empty(None, None, false).unwrap();
        workspace
            .settings
            .set(Layer::User, "ui_language", json!(language));
        workspace.sync_ui_language();
        App::new(
            options.clone(),
            Clock::new(now, std::time::SystemTime::now()),
            Reporter::for_options(&options, now).unwrap(),
            workspace,
            Arc::new(AtomicBool::new(false)),
            std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(None))),
        )
    }

    #[test]
    fn finder_new_file_fills_numbering_gaps_without_overwriting() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().canonicalize().unwrap().join("有空格 & café");
        fs::create_dir(&directory).unwrap();
        for stem in ["New Text Document", "新建文本文档"] {
            let original = create_text_file(&directory, stem).unwrap();
            assert_eq!(original, directory.join(format!("{stem}.txt")));
            fs::write(&original, b"keep").unwrap();
            let third = directory.join(format!("{stem} (3).txt"));
            fs::write(&third, b"keep this too").unwrap();
            let second = create_text_file(&directory, stem).unwrap();
            assert_eq!(second, directory.join(format!("{stem} (2).txt")));
            assert_eq!(fs::read(second).unwrap(), b"");
            let fourth = create_text_file(&directory, stem).unwrap();
            assert_eq!(fourth, directory.join(format!("{stem} (4).txt")));
            assert_eq!(fs::read(fourth).unwrap(), b"");
            assert_eq!(fs::read(original).unwrap(), b"keep");
            assert_eq!(fs::read(third).unwrap(), b"keep this too");
        }
    }

    #[test]
    fn finder_new_file_concurrent_requests_and_dangling_symlinks_are_exclusive() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().canonicalize().unwrap();
        let original = directory.join("New Text Document.txt");
        let missing = directory.join("must-not-exist");
        std::os::unix::fs::symlink(&missing, &original).unwrap();
        let paths = std::thread::scope(|scope| {
            (0..8)
                .map(|_| scope.spawn(|| create_text_file(&directory, "New Text Document").unwrap()))
                .collect::<Vec<_>>()
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .collect::<std::collections::BTreeSet<_>>()
        });
        assert_eq!(paths.len(), 8);
        assert!(
            paths
                .iter()
                .all(|path| fs::metadata(path).unwrap().len() == 0)
        );
        assert!(!missing.exists());
        assert_eq!(fs::read_link(original).unwrap(), missing);
    }

    #[test]
    fn finder_new_file_opens_a_tab_in_the_receiving_window_in_its_ui_language() {
        for (language, name) in [
            ("en", "New Text Document.txt"),
            ("zh-Hans", "新建文本文档.txt"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let directory = root.path().canonicalize().unwrap();
            let existing = directory.join("existing.txt");
            fs::write(&existing, "existing").unwrap();
            let mut app = app(language);
            app.open_received_paths(std::slice::from_ref(&existing))
                .unwrap();
            app.workspace
                .settings
                .set(Layer::User, "open_files_in_new_window", json!("always"));
            let window = app.session_id;
            app.handle_finder_new_text_file(None, &directory);
            assert_eq!(app.session_id, window);
            assert_eq!(app.workspace.tabs.len(), 2);
            assert_eq!(app.workspace.tabs[0].path.as_ref(), Some(&existing));
            let path = directory.join(name);
            assert_eq!(
                app.workspace.tabs[app.workspace.active].path.as_ref(),
                Some(&path)
            );
            assert_eq!(fs::read(&path).unwrap(), b"");
            assert!(app.workspace.recent_files.contains(&path));
        }
    }

    #[test]
    fn finder_new_file_errors_go_to_the_status_bar_without_opening_a_tab() {
        let root = tempfile::tempdir().unwrap();
        let mut app = app("zh-Hans");
        let file = root.path().join("not-a-directory");
        fs::write(&file, "keep").unwrap();
        for invalid in [file.clone(), root.path().join("missing")] {
            app.handle_finder_new_text_file(None, &invalid);
            assert!(app.workspace.is_empty());
            assert!(app.panels.message.starts_with("无法在 "));
            assert!(app.panels.message.contains(invalid.to_str().unwrap()));
        }
        assert_eq!(fs::read(file).unwrap(), b"keep");
    }
}
