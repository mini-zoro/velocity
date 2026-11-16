//! Finder, Dock and file-drop requests share the existing text/huge/hex tab loaders.
//! Native delegates can queue literal filesystem paths before a window is available.
use crate::{
    cli::{FileLocation, Options},
    instance::{Instance, Request, Start},
    panels::Panels,
    workspace::Workspace,
};
use std::{
    collections::VecDeque,
    fs::{self, File},
    path::{Path, PathBuf},
};

/// Result of relaying a secondary LaunchServices process's actual open event.
#[derive(Debug)]
pub enum NativeForward {
    /// The existing process acknowledged the native request; the proxy can quit.
    Forwarded,
    /// The previous process exited; retain this listener and open paths locally.
    Primary(Instance),
}

/// Relay literal native paths to the existing key window, or claim the profile if
/// its previous primary has exited. The returned listener must be attached to App.
pub fn forward_native_paths(
    data_dir: Option<&Path>,
    paths: &[PathBuf],
) -> Result<NativeForward, String> {
    match Instance::start(data_dir, &native_request(paths)?)? {
        Start::Forwarded => Ok(NativeForward::Forwarded),
        Start::Primary(instance) => Ok(NativeForward::Primary(instance)),
    }
}
fn native_request(paths: &[PathBuf]) -> Result<Request, String> {
    let mut request = Request::new(
        &Options {
            files: paths
                .iter()
                .map(|path| FileLocation {
                    path: path.clone(),
                    line: 1,
                    column: 1,
                })
                .collect(),
            add_to_window: true,
            ..Options::default()
        },
        None,
    )?;
    request.native_open = true;
    Ok(request)
}

/// Ordered native open requests retained until the target window is ready.
#[derive(Debug, Default)]
pub struct Router {
    pending: VecDeque<PathBuf>,
}
impl Router {
    /// Queue paths without opening or inspecting them before a window exists.
    pub fn enqueue(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.pending.extend(paths);
    }
    /// Apply all queued requests to the chosen window, collecting failures without
    /// discarding later valid requests. Repeated paths focus the existing tab/root.
    pub fn apply(&mut self, workspace: &mut Workspace, panels: &mut Panels) -> Result<(), String> {
        let mut errors = Vec::new();
        for path in self.pending.drain(..) {
            if let Err(error) = open_path(workspace, panels, &path) {
                errors.push(format!("{}: {error}", path.display()));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }
}

/// Open literal native paths in an existing window; the caller presents any errors.
/// Numeric colon suffixes are part of the filename here, never CLI line/column syntax.
pub fn open_paths(
    workspace: &mut Workspace,
    panels: &mut Panels,
    paths: &[PathBuf],
) -> Result<(), String> {
    let mut router = Router::default();
    router.enqueue(paths.iter().cloned());
    router.apply(workspace, panels)
}

fn open_path(workspace: &mut Workspace, panels: &mut Panels, path: &Path) -> Result<(), String> {
    // Validate before selecting existing tabs too: missing or unreadable native requests
    // must report their path rather than silently succeeding against an old buffer.
    let path = fs::canonicalize(path).map_err(|e| e.to_string())?;
    let metadata = fs::metadata(&path).map_err(|e| e.to_string())?;
    if metadata.is_dir() {
        // Test actual access rather than Unix mode bits (ACLs and user identity matter).
        fs::read_dir(&path)
            .map_err(|e| e.to_string())?
            .next()
            .transpose()
            .map_err(|e| e.to_string())?;
        panels.add_folder(workspace, path)
    } else if metadata.is_file() {
        File::open(&path).map_err(|e| e.to_string())?;
        if let Some(index) = workspace
            .tabs
            .iter()
            .position(|tab| tab.path.as_ref() == Some(&path))
        {
            workspace.select(index);
            workspace.tabs[index].transient = false;
            return Ok(());
        }
        workspace.open(FileLocation {
            path,
            line: 1,
            column: 1,
        })
    } else {
        Err("only regular files and folders can be opened".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        time::{Duration, Instant},
    };
    use vel_text::Text;

    fn window() -> (Workspace, Panels) {
        let workspace = Workspace::new(Text::new(), None, None, false).unwrap();
        let panels = Panels::new(&workspace, false);
        (workspace, panels)
    }
    #[test]
    fn forwarded_native_requests_keep_literal_paths_and_native_duplicate_semantics() {
        let request =
            native_request(&[PathBuf::from("literal:2:3"), PathBuf::from("folder")]).unwrap();
        assert!(request.native_open);
        assert!(request.add);
        assert!(!request.new_window);
        assert!(!request.wait);
        assert!(request.stdin.is_none());
        assert!(request.files[0].path.is_absolute());
        assert!(request.files[0].path.ends_with("literal:2:3"));
        assert_eq!((request.files[0].line, request.files[0].column), (1, 1));
    }
    #[test]
    fn before_window_queue_opens_files_folders_and_literal_colon_names_once_ready() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("你好:2:3");
        let folder = directory.path().join("folder");
        fs::write(&file, "first\nsecond\n").unwrap();
        fs::create_dir(&folder).unwrap();
        let mut router = Router::default();
        router.enqueue([file.clone(), folder.clone()]);
        assert_eq!(router.pending.len(), 2);
        let (mut workspace, mut panels) = window();
        assert!(workspace.tabs[0].path.is_none());
        router.apply(&mut workspace, &mut panels).unwrap();
        assert!(router.pending.is_empty());
        assert_eq!(workspace.tabs.len(), 1);
        assert_eq!(
            workspace.tabs[0].path.as_ref(),
            Some(&file.canonicalize().unwrap())
        );
        assert_eq!(workspace.editor().caret, 0);
        assert_eq!(workspace.editor().text.to_string(), "first\nsecond\n");
        assert_eq!(workspace.folders.len(), 1);
        assert_eq!(workspace.folders[0].path, folder.canonicalize().unwrap());
        assert!(panels.side_visible);
        router.apply(&mut workspace, &mut panels).unwrap();
        assert_eq!(workspace.tabs.len(), 1);
        assert_eq!(workspace.folders.len(), 1);
    }
    #[test]
    fn duplicates_focus_existing_tab_and_preserve_unsaved_edits() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("a.txt");
        let second = directory.path().join("b.txt");
        fs::write(&first, "saved").unwrap();
        fs::write(&second, "second").unwrap();
        let (mut workspace, mut panels) = window();
        open_paths(&mut workspace, &mut panels, std::slice::from_ref(&first)).unwrap();
        workspace
            .editor_mut()
            .run("insert", &serde_json::json!({"characters":"unsaved "}))
            .unwrap();
        workspace
            .editor_mut()
            .set_selection(vel_buffer::Selection::from_regions([
                vel_buffer::Region::new(2, 5),
            ]));
        workspace.tabs[0].scroll = (24., 456.);
        workspace.tabs[0].transient = true;
        workspace.take_pending_center();
        let selection = workspace.editor().regions();
        open_paths(
            &mut workspace,
            &mut panels,
            &[second, directory.path().join("./a.txt")],
        )
        .unwrap();
        assert_eq!(workspace.tabs.len(), 2);
        assert_eq!(workspace.active, 0);
        assert!(workspace.tabs[0].dirty());
        assert!(!workspace.tabs[0].transient);
        assert_eq!(workspace.editor().regions(), selection);
        assert_eq!(workspace.tabs[0].scroll, (24., 456.));
        assert!(!workspace.take_pending_center());
        assert_eq!(workspace.editor().text.to_string(), "unsaved saved");
        #[cfg(unix)]
        {
            let alias = directory.path().join("alias.txt");
            std::os::unix::fs::symlink(&first, &alias).unwrap();
            let folder_alias = directory.path().join("folder-alias");
            let folder = directory.path().join("folder");
            fs::create_dir(&folder).unwrap();
            std::os::unix::fs::symlink(&folder, &folder_alias).unwrap();
            open_paths(&mut workspace, &mut panels, &[alias, folder, folder_alias]).unwrap();
            assert_eq!(workspace.tabs.len(), 2);
            assert_eq!(workspace.active, 0);
            assert_eq!(workspace.editor().regions(), selection);
            assert_eq!(workspace.tabs[0].scroll, (24., 456.));
            assert!(!workspace.take_pending_center());
            assert_eq!(workspace.folders.len(), 1);
        }
    }
    #[test]
    fn missing_paths_report_errors_and_later_valid_paths_still_open() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing");
        let valid = directory.path().join("valid.txt");
        let later_missing = directory.path().join("later missing");
        fs::write(&valid, "valid").unwrap();
        let (mut workspace, mut panels) = window();
        let error = open_paths(
            &mut workspace,
            &mut panels,
            &[missing.clone(), valid, later_missing.clone()],
        )
        .unwrap_err();
        assert!(error.contains(&missing.display().to_string()));
        assert!(error.contains(&later_missing.display().to_string()));
        assert_eq!(workspace.tabs.len(), 1);
        assert_eq!(workspace.editor().text.to_string(), "valid");
    }
    #[cfg(unix)]
    #[test]
    fn unreadable_files_and_folders_report_their_paths_without_opening_tabs() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("unreadable.txt");
        let folder = directory.path().join("unreadable-folder");
        fs::write(&file, "private").unwrap();
        fs::create_dir(&folder).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o0)).unwrap();
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o0)).unwrap();
        // Privileged test runners can legitimately read mode-000 paths; assert that
        // routing follows actual OS access rather than manufacturing a permission error.
        let inaccessible = [file.clone(), folder.clone()]
            .into_iter()
            .filter(|path| {
                if path == &file {
                    File::open(path).is_err()
                } else {
                    fs::read_dir(path).is_err()
                }
            })
            .collect::<Vec<_>>();
        let (mut workspace, mut panels) = window();
        let result = open_paths(&mut workspace, &mut panels, &inaccessible);
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o700)).unwrap();
        if !inaccessible.is_empty() {
            let error = result.unwrap_err();
            for path in inaccessible {
                assert!(error.contains(&path.display().to_string()));
            }
        }
        assert!(workspace.tabs[0].path.is_none());
        assert!(workspace.folders.is_empty());
    }
    #[cfg(unix)]
    #[test]
    fn special_files_fail_without_attempting_a_stream_read() {
        use std::os::unix::net::UnixListener;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("socket");
        let _listener = UnixListener::bind(&path).unwrap();
        let (mut workspace, mut panels) = window();
        let error = open_paths(&mut workspace, &mut panels, &[path]).unwrap_err();
        assert!(error.contains("only regular files and folders"));
        assert!(workspace.tabs[0].path.is_none());
    }
    #[test]
    fn binaries_and_huge_text_delegate_to_existing_snapshot_and_background_loaders() {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("binary.bin");
        let huge = directory.path().join("huge.log");
        fs::write(&binary, b"\0binary\xff").unwrap();
        let mut file = File::create(&huge).unwrap();
        let block = b"a log line\n".repeat(8 * 1024);
        for _ in 0..800 {
            file.write_all(&block).unwrap();
        }
        drop(file);
        let (mut workspace, mut panels) = window();
        open_paths(&mut workspace, &mut panels, std::slice::from_ref(&huge)).unwrap();
        assert!(workspace.tabs[workspace.active].loading.is_some());
        assert!(workspace.editor().read_only);
        assert!(workspace.editor().source_bytes >= 64 * 1024 * 1024);
        let cancel = workspace.tabs[workspace.active]
            .loading
            .as_ref()
            .unwrap()
            .cancellation();
        workspace.close(workspace.active).unwrap();
        assert!(cancel.is_cancelled());
        open_paths(&mut workspace, &mut panels, &[binary]).unwrap();
        assert!(workspace.tabs[workspace.active].binary_loading.is_some());
        assert!(workspace.editor().read_only);
        let deadline = Instant::now() + Duration::from_secs(30);
        while workspace.tabs[workspace.active].binary_loading.is_some() {
            workspace.poll();
            assert!(Instant::now() < deadline, "{:?}", workspace.messages);
            std::thread::park_timeout(Duration::from_millis(1));
        }
        assert_eq!(
            workspace.editor().binary.as_ref().unwrap().view.len_bytes(),
            8
        );
        assert!(workspace.editor().read_only);
    }
}
