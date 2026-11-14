# Spec: workbench — layouts, tab multi-select, windows, sessions & Hot Exit, projects, build systems

Status: P2 / lane A (platform, UI). Locations: `crates/velocity/`, `crates/vel-ui/`, `crates/vel-commands/`,
`crates/vel-config/`, `crates/vel-fileio/`, new `crates/vel-build/`, new `crates/vel-session/`.

References (public docs only): tab_multi-select.html, projects.html, build_systems.html,
distraction_free.html, command_line.html, settings.html (`hot_exit`, `remember_open_files`,
`close_windows_when_empty`, `open_files_in_new_window`, …), the API reference for Window/Sheet
commands (`set_layout`, `focus_group`, `move_to_group`, `new_pane`, `close_pane`).

## 1. Layouts and groups

- `set_layout {cols, rows, cells}` with the documented normalized coordinate format; presets in
  the View menu (Single, Columns 2/3/4, Rows 2/3, Grid 4); draggable splitters; per-group tab
  bars; `focus_group`, `move_to_group`, `focus_neighboring_group`, `move_to_neighboring_group`,
  `new_pane`/`close_pane`; `origami`-free basics only.
- Several views into the same `vel_buffer::Buffer` (shared undo, per-view selections/scroll),
  `clone_file`.

## 2. Tab multi-select (ST4)

- Cmd+click / Shift+click on tabs, Goto Anything Cmd+Enter, side bar multi-select: show the
  selected sheets side by side in the group; the tab connector styling hook; `Tab Selection`
  menu commands; closing/moving multiple sheets; semi-transient (preview) sheets from the side
  bar and Goto Anything.

## 3. Windows

- Multiple windows (`new_window`, `close_window`), each with its own layout, side bar, project;
  macOS window tabs disabled (as today) unless `native_tabs` is set later; full screen;
  **distraction-free mode** (`toggle_distraction_free`, its own settings layer
  `Distraction Free.sublime-settings`).

## 4. Sessions and Hot Exit (`vel-session`)

- Persist the session on change (debounced) and on quit: windows, layouts, open files with
  selections, scroll positions, folds, syntax overrides, find/replace history, recent files and
  folders, side bar state.
- **Hot Exit** (`hot_exit`: `"always"`, `"only_on_quit"`, `"disabled"` — implement the documented
  values): unsaved and untitled buffers are stored with their contents **and undo history**
  (serialize `vel-buffer` history; bounded by the history limits) so quitting never prompts and
  everything comes back on launch. If a file changed on disk since, keep the buffer but drop the
  restored undo history (ST build 4213 behavior: "undo/redo history is no longer restored when
  the file has changed on disk").
- Crash safety: write atomically; a corrupt session file must not prevent startup (start empty,
  keep the corrupt file aside, log it).
- Budgets: session save of 50 open files ≤ 20 ms on a background thread; restore with 50 files
  (10 large) shows the first window ≤ 300 ms with lazy loading of non-visible tabs.

## 5. Projects and workspaces

- `.sublime-project` (folders with `path`, `name`, `folder_exclude_patterns`, `file_exclude_patterns`,
  `binary_file_patterns`, `follow_symlinks`; `settings`; `build_systems`), `.sublime-workspace`
  (the per-project session); Project menu: Open Project, Save Project As, Close Project, Add
  Folder, Edit Project, Recent Projects, **Quick Switch Project** (Ctrl+Cmd+P).
- CLI: `velocity --project file.sublime-project`, `velocity -n/-a/-w` (new window / add to window /
  wait), `velocity -` (stdin) — keep single-instance forwarding over a local socket.

## 6. Build systems (`vel-build`)

- `.sublime-build` with `cmd`, `shell_cmd`, `working_dir`, `env`, `path`, `shell`, `selector`,
  `file_patterns`, `keyfile`, `file_regex`, `line_regex`, `syntax` (for the output panel),
  `variants`, `target` (only the built-in `exec`), `cancel`, `quiet`, `word_wrap`; build variables
  (`$file`, `$file_path`, `$file_name`, `$file_extension`, `$file_base_name`, `$folder`,
  `$project`, `$project_path`, `$project_name`, `$project_base_name`, `$packages`, with
  `${var:default}` and `${var/regex/format/}` substitutions).
- Tools menu: Build System (Automatic + list), Build (Cmd+B), Build With… (Cmd+Shift+B), Cancel
  Build; `exec` runs asynchronously, streams stdout/stderr into the **build output panel**
  (`show_panel {panel: output.exec}`), shows exit code and elapsed time, kills the process tree on
  cancel; F4/Shift+F4 (`next_result`/`prev_result`) and double-click navigate `file_regex` matches;
  errors shown as **inline annotations** at the file location (simple text badges for now; the
  minihtml engine comes later) — `show_errors_inline` setting.
- Ship our own build files for a few languages in our default package only if they are written
  from scratch; the official `third_party/sublime-packages` already contains `.sublime-build`
  files (e.g. `Rust/Cargo.sublime-build`) that may be loaded as-is (permissive license).

## Tests and budgets

- Headless tests: layout cell math and splitter dragging, group/sheet movement, multi-select
  rules, session round trip (incl. undo history, folds, selections) and corruption handling,
  hot-exit dirty/changed-on-disk rules, project file parsing, build variable expansion,
  `file_regex` result parsing with relative paths, process cancellation.
- Existing typing/scroll budgets must hold with 4 groups visible.
- GUI measurement modes and human checklist additions as before (sandbox has no window server).

## Rules

- Modify only lane-A crates (`velocity` except `resources/Packages/Color Scheme - Velocity/`, `vel-ui`,
  `vel-render`, `vel-platform`, `vel-config`, `vel-commands`, `vel-fileio`, `vel-search`, and the new
  `vel-build`, `vel-session`) + root `Cargo.lock`. Lane B is building `vel-index`, `vel-complete`,
  `vel-snippet` and extending `vel-highlight` in parallel — don't touch those; their integration is
  the next lane-A round. If `vel-buffer` needs history serialization support, describe the needed API
  in your final message and implement serialization on your side through the public API if possible.
