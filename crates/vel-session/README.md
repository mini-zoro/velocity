# vel-session

Device-free, versioned workbench sessions, atomic persistence, Hot Exit policy and shareable
project parsing. Original code from the lane-A spec and public [projects](https://www.sublimetext.com/docs/projects.html),
[settings](https://www.sublimetext.com/docs/settings.html) and [Window API](https://www.sublimetext.com/docs/api_reference.html)
documentation. No application-bundled Default files or binaries were read or copied.

`Session` contains shared documents plus window-local sheets, layouts, selections (including
primary/direction/vertical goal), folds, scroll, syntax overrides, sidebar state, find/replace
history, recent files/folders/projects, build selection and distraction/full-screen state.
IDs are validated before startup can use them. `Window.workbench` uses vel-ui's normalized
layout and stable sheet references. Coordinates are UTF-8 bytes; restored positions are clipped
to valid text boundaries. Hidden clean documents retain paths and load on activation. Dirty
and untitled documents retain text and undo; several sheets can reference one document.

`Writer` accepts cheap rope snapshots, coalesces changes, and performs validation, buffered JSON
encoding, temporary-file writes, POSIX file fsync, atomic rename and directory fsync on a worker.
The debounce is 400 ms; application capture is separately coalesced for 300 ms. Quit calls
`flush`, which waits for completion and reports an error instead of claiming durability after
an IO failure. A failed autosave retains its latest snapshot for retry. No session, workspace or
cache directory has to exist at startup. Missing parents are created at save time. Read-only
storage and a file occupying a required directory return recoverable errors.

`load` returns an empty session for absent, unreadable, invalid or corrupt data. Corrupt bytes
are renamed beside the original with a unique `.corrupt-TIMESTAMP` suffix; if that fails the
original is kept and the diagnostic explains why. Normal startup never depends on a writable
quarantine path. The global path is `DATA/Sessions/Session.sublime-session`. Project workspaces
are `PROJECT.sublime-workspace`, beside their shareable `.sublime-project`.

`HotExit::Always` retains dirty closed windows and quit state. `OnlyOnQuit` prompts when a window
is explicitly closed and retains on application quit; the application also saves live dirty
snapshots for crash recovery while that window is open. `Disabled` uses normal Save/Discard/
Cancel handling and persists only paths/view metadata. `remember_open_files: false` suppresses
ordinary clean-file restoration when Hot Exit does not retain the session. Clean windows can
close with storage unavailable; a dirty Hot Exit cannot claim success without a durable save.

The disk stamp is the saved/loaded file's size and nanosecond mtime, not the latest observed
conflict stamp. A changed/missing disk file drops the restored undo stack and retains dirty
buffer contents. Clean hidden files without retained history load the current disk contents.

`project::Project` accepts folders, exclusion/binary patterns, symlink flags, settings, inline
build systems, comments and trailing commas. Relative folders resolve against the project file.
Save As preserves unknown keys and the original folder rules, writes atomically, and creates a
missing destination directory. A workspace write never triggers a project-settings reload via
its containing directory's mtime.

## Complete native history

`History::capture(&buffer)` owns `Buffer::export_history()` plus the current rope snapshot.
There is no command observer or second undo journal on the typing path. The session writer
serializes the backend's opaque versioned payload, including groups/steps, text and soft undo
cursors, selection events, saved revision identity, limits, typing continuation, named regions,
per-view settings and original edit ownership. `restore_views(keep_history, dirty)` calls the
checked `Buffer::restore_history` importer and returns the complete original-to-fresh view map.
Closed historical owners stay hidden. Sheet `ViewState.history_view` binds each clone to that
map; the application does not reapply imported selections and create extra soft events.

Clean files use `History::capture_saved`: their native payload still includes soft selection
history, while current contents are supplied lazily from disk on activation. Dirty and untitled
contents are always retained independently. Disk stamps invalidate history; the backend also
checks the current text digest, including changes that preserve size/mtime. A failed native
import can discard history while retaining dirty contents. Missing clean files report a load
error. No import mutates an existing live buffer, and errors remain recoverable.

The previous round-seven wire journal has a reader for existing profiles. It upgrades the text
undo information those profiles actually retained into a native payload on the next save.
New profiles preserve full native history. Old profiles cannot recover events they never stored.
`restore` and `restore_views` return errors for unreadable or malformed data.

Tests cover 128 randomized two-view histories, each followed by 200 identical future actions,
selection-only clean-file history, typing continuation, saved points, closed owners, legacy
profiles, changed disk files and corrupt payloads. Application tests also exercise clone/lazy
sheet restoration, ownership transfers and continuing a typing group across relaunch.

```sh
cargo test -p vel-session
cargo clippy -p vel-session --all-targets -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc -p vel-session --no-deps
```
