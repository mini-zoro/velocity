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

## Public-buffer history adapter and requested API

`History::before_command` and `observe` journal the application's primitive commands using
O(1) rope roots, selection snapshots and public history limits. Encoding runs on the writer
and stores one base plus UTF-8-safe deltas. Restore replays user-visible groups through public
`Buffer::transact`, then restores the redo cursor and saved state. Tests cover normal typing,
selection changes, before/after selections, shared-view undo, save points, redo and bounded
retention. Changed-disk restoration uses a pruned no-op transaction to preserve dirty state
without leaving an undo entry.

The public Buffer API does not export its private history. This adapter cannot reproduce
transaction-level soft undo/selection events, change-map history, or original edit-owner ViewIds
after a process restart; restored text undo is replayed through the first restored buffer view.
Generic commands with internal grouping not observable at command boundaries may also differ.
The live buffer continues to use its native shared history. These are explicit Hot Exit fidelity
limits, not a claim of lossless serialization of Buffer internals.

Requested vel-buffer API (no vel-buffer sources were modified):

```text
Buffer::export_history() -> owned, versioned HistorySnapshot
Buffer::restore_history(text, HistorySnapshot, view_id_map) -> Result<Buffer, HistoryError>
```

The payload should preserve bounded groups/steps, forward/inverse edits, owner view IDs,
before/after selections, undo/redo cursor, saved-state identity, limits and per-view soft history.
Import must validate UTF-8 ranges/identities and remap old view IDs to newly allocated views.
An owned O(1)-snapshot export lets the caller serialize on a worker; a stable public step iterator
plus checked import is also sufficient. See velocity/RUNBOOK.md for timing and reviewer commands.

```sh
cargo test -p vel-session
cargo clippy -p vel-session --all-targets -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc -p vel-session --no-deps
```
