use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};
use vel_search::{fuzzy, project::Folder};

type Files = (Arc<Vec<PathBuf>>, Arc<Vec<fuzzy::Candidate>>);

/// Only used by Goto Anything when the user requests Git ignore filtering.
#[derive(Debug)]
pub(super) struct Filter {
    pub files: Option<Files>,
    result: mpsc::Receiver<Files>,
    stop: Arc<AtomicBool>,
}

impl Filter {
    pub(super) fn start(folders: Vec<Folder>) -> Self {
        let (send, result) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        std::thread::spawn(move || {
            let paths = Mutex::new(BTreeSet::new());
            for folder in &folders {
                folder
                    .walker_with_gitignore(&folder.path, None, true, true)
                    .build_parallel()
                    .run(|| {
                        Box::new(|entry| {
                            use vel_search::project::WalkFlow;
                            if cancelled.load(Ordering::Relaxed) {
                                return WalkFlow::Quit;
                            }
                            if let Ok(entry) = entry
                                && entry.file_type().is_some_and(|kind| kind.is_file())
                            {
                                paths
                                    .lock()
                                    .expect("Goto file list")
                                    .insert(entry.into_path());
                            }
                            WalkFlow::Continue
                        })
                    });
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
            }
            let paths: Vec<_> = paths
                .into_inner()
                .expect("Goto file list")
                .into_iter()
                .collect();
            let labels = paths
                .iter()
                .map(|path| {
                    let label = folders
                        .iter()
                        .find_map(|folder| path.strip_prefix(&folder.path).ok())
                        .unwrap_or(path);
                    fuzzy::Candidate::new(label.to_string_lossy())
                })
                .collect();
            let _ = send.send((Arc::new(paths), Arc::new(labels)));
        });
        Self {
            files: None,
            result,
            stop,
        }
    }

    pub(super) fn poll(&mut self) -> bool {
        if self.files.is_some() {
            return false;
        }
        self.files = match self.result.try_recv() {
            Ok(files) => Some(files),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Files::default()),
        };
        self.files.is_some()
    }
}

impl Drop for Filter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
