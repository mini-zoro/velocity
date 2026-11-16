# Spec: opening, saving, sessions, projects and settings details

Status: P4 / lane B, round 15. Locations: `crates/velocity/src/{workspace.rs,workbench.rs,session.rs,instance.rs,
main.rs}`, the open/save/close paths of `crates/velocity/src/app.rs`, `crates/vel-session/`, `crates/vel-search/src/
project.rs` (walker only), `crates/vel-fileio/` (encodings only, see below), and packaged menus/keymap entries for the
new commands. Lane A edits `panels.rs`, `vel-ui`, `vel-search` text/files and small app.rs hooks this round; keep
your app.rs changes inside the open/save/close/session/project functions.

Background: a reviewer detail audit (2026-11-16) against Sublime Text 4. Each item was located in code; line numbers
are approximate. Already fixed by the reviewer in `vel-fileio` / `vel-config`, do not redo: new files respect the
umask; `atomic_save` defaults to false and falls back to an in-place write when the rename would lose permissions,
ownership, xattrs or hard links; `trim_trailing_white_space_on_save: "all"`; `default_line_ending: "system"`;
project settings no longer override syntax-specific settings.

References: https://www.sublimetext.com/docs/ (settings, projects, command_line), the ST3/ST4 changelogs.
Clean-room: behaviour only.

## 1. Opening

1. **Non-existent paths open as new files** (`velocity new.txt`, `subl`-style): a tab with that path, empty, marked
   new; saving creates the file (a missing parent directory is reported as a save error). Today `workspace.open`
   fails on `metadata()?`.
2. **One failing path must not abort the rest:** collect per-path errors (CLI forwarding in `app.rs`, the startup
   path in `main.rs`, Finder open requests) and report them together; the client still gets its ack.
3. **Directories without `-a` open in a new window** (`velocity ~/proj`); `-a`/`--add` adds to the current window.
4. **`velocity -` writes the edited buffer back to the client's stdout when the tab closes** (`instance.rs` waiter
   currently only acks).
5. **Symlinked folders:** `follow_symlinks` defaults to true for side bar folders (ST3 3033); symlinked directories
   expand as directories in the side bar and in Goto Anything; cycles are detected.

## 2. Saving and closing

1. **Save All saves only dirty tabs**, continues after a cancelled or failed save dialog, and is enabled even when
   the active tab is read-only (`app.rs` `save_all`, `command_enabled`).
2. **Files deleted on disk:** mark the tab as deleted (title marker, dirty for close purposes), so closing prompts;
   saving recreates it. Session restore keeps such tabs as deleted unless `close_deleted_files` is true (ST4 4107),
   and a missing file never aborts restoring the rest of the window (`workbench.rs` `restore_window` uses `?` on
   `prepare_sheet`; lazily restored tabs report errors once, not on every access).
3. **Close prompt** buttons: "Save", "Don't Save", "Cancel"; the file name without the dirty "•".
4. **Untitled tabs** are named after their first line (up to ~50 characters) like `set_unsaved_view_name`; the save
   dialog suggests that name.
5. **Closing the last tab leaves the window empty** (no automatic new Untitled tab); a new file is created on
   demand (typing into an empty window, super+n, double click on the tab bar).
6. **Reopen closed files:** keep a stack of closed file tabs (path, selection, scroll) per window and implement
   `reopen_last_file` on `super+shift+t`; add File › Open Recent (recent files, folders, projects, "Clear Items")
   and "Reopen Closed File" to the menu.
7. **`"hot_exit": false`** (legacy boolean) means disabled.
8. Unreadable files report the path in the error (`vel-fileio/src/progressive.rs`).

## 3. Projects and sessions

1. **Opening a project from a window without a project must not drop untitled/dirty buffers:** with hot exit, save
   the current session first (or open the project in a new window); without hot exit, prompt. Same for CLI
   `--project` on a running instance (`app.rs` open-project path, `session.rs` `publish` overwrites the window).
2. **`open_project` keeps file watching** (`workbench.rs` passes `watch=false` to `Workspace::new`; use the current
   window's setting, as `close_project` does).
3. **Project folders:** support `file_include_patterns` / `folder_include_patterns`; Save Project As keeps relative
   paths relative to the new project file; unknown keys in the project file survive a save (`#[serde(flatten)]`).
4. **Default exclude patterns** match Sublime's documented defaults (version control directories, `.DS_Store`,
   compiled objects, `*.sublime-workspace`, …) — do not hide `target` by default; projects/users can add it.
5. **Goto Anything and Find in Files list git-ignored files** (Sublime applies `.gitignore` to symbol indexing via
   `index_exclude_gitignore`, not to the file list). Keep the side bar as is. Verify against current docs first and
   note the source in a code comment.

## 4. Settings and contexts

1. **Indentation detection runs on load only:** settings or resource reloads and external reloads must not reset a
   tab's manually chosen `tab_size` / `translate_tabs_to_spaces` (`workspace.rs` `needs_detection`).
2. **Encodings:** accept Sublime's display names for `fallback_encoding` / `default_encoding` / `set_encoding`
   ("Cyrillic (Windows 1251)", "UTF-16 LE with BOM", …) with a table mapped to `encoding_rs` labels; an invalid
   fallback reports once and uses Windows-1252; a file whose BOM is followed by invalid data falls back instead of
   failing. Add File › Reopen with Encoding and Save with Encoding menus.
3. **More key binding contexts** (`editor.rs` `ContextProvider::values`): `read_only`, `popup_visible`,
   `last_command`, `last_modifying_command`, `eol_selector`, `is_recording`, `overlay_has_focus`, `group_has_multiselect`
   (as far as the data exists). `panel` and `panel_has_focus` already exist. Unknown keys keep returning no value.

## Rules

- Worktree: /Users/zoro/dev/velocity-lane-b (branch `lane-b`), rebased on main first. Do not commit.
- Headless tests for every item (temporary directories for files/projects/sessions, symlinks, permissions); add the
  native checks (Finder, dialogs, menus) to MANUAL_TEST.md. No regressions; existing session/hot-exit tests keep
  passing.
- fmt, clippy `-D warnings`, `cargo test --workspace --locked --release`, rustdoc `-D warnings`.
