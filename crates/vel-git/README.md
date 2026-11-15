# vel-git

Read-only Git discovery, status, folder aggregation and HEAD/index blobs using pure Rust gix.
Engine code: MIT OR Apache-2.0. No commit/stage/index write APIs; gix's index-stat suggestions
are deliberately never written back. Rename tracking is disabled: renames appear as deletion
plus addition. Linked worktrees and unborn/detached HEAD are supported. Symlink paths retain
Git's repository-relative meaning; raw path bytes and blob bytes are preserved.

```rust,no_run
use std::path::{Path, PathBuf};
use vel_git::{GitService, Repository, DiffTarget};
let service = GitService::new(PathBuf::from("/project"), true)?;
// Idle/redraw: returns immediately; the initial snapshot arrives asynchronously.
if let Some(status) = service.try_snapshot() {
    let badge = status.badge(Path::new("src/main.rs"));
    let folder_badge = status.folders.get(Path::new("src"));
    # let _ = (badge, folder_badge);
}
service.refresh_paths([PathBuf::from("src/main.rs")])?;
// On an analysis worker, for git_diff_target:
let repo = Repository::discover("/project")?;
let bytes = repo.blob(Path::new("src/main.rs"), DiffTarget::Index)?;
// Decode valid text, then DocumentDiff::set_reference_document(Text::from(...)).
# let _ = bytes;
# Ok::<(), vel_git::Error>(())
```

Gate service creation with `show_git_status`. `Repository` is synchronous for editor-owned
workers, tests and benchmarks; `GitService` performs discovery/status/watch setup on its own
worker. Read publications via `try_snapshot` and failures via `try_error`. Requests and watcher
paths coalesce; overflow/rescan and `.git`, ignore, config, index or ref changes trigger a full
refresh. Ordinary file changes use literal pathspecs and replace only those cached subtrees.
The worker watches worktree plus actual/common metadata directories, including external linked
worktree metadata. Drop signals cancellation without blocking the UI on an in-flight scan.

`StatusSnapshot.files` contains only changed paths and collapsed ignored directories. A file
can retain multiple statuses. Display and folder aggregation use this ascending precedence:
ignored (de-emphasis), untracked, modified, missing, staged addition, staged modification,
staged deletion, unmerged. This follows the public badge ordering, where later entries win.
Ignored ancestor lookup avoids enumerating every ignored file. `folders[Path::new("")]` is the
root aggregate. Paths passed to the synchronous API must stay within the repository root.

HEAD/index blobs return `None` for absent paths/unborn HEAD, and an error for unmerged index
entries or non-blob objects. The caller controls binary/encoding/size policy. Ahead/behind uses
only cached local refs and a walk bounded to 1024 commits and 2 ms; None means unavailable or
not cheap. No network, fetching, shell git process or repository writes in engine/service code.

`GitService::with_watch_mode(folder, WatchMode::Polling(interval))` is an explicit fallback for
sandboxes/network filesystems without native event delivery. Polling compares contents as well
as timestamps to detect rapid same-second edits. It scans watched trees and costs more than
native events; use native notifications for large repositories. Tests exercise actual polling
worktree/index delivery, coalescing and `.git` canonical path aliases. Native FSEvents delivery
was unavailable in the sandbox and remains an integration check on an unrestricted machine.

Round-seven budget, validation and raw timing evidence: [engine report](../vel-git/ROUND7.md).
