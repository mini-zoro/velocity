# Spec: open files from Finder/Dock, native context menus, round-12 native bugs

Status: P4 / lane A, round 14. Locations: lane-A crates (`vel-platform`, `velocity`, `vel-config`, packaging).
Background: `docs/review/round12-native-review.md` (5 bugs), and the reviewer fix commit "Scroll phase 1 + find
panel close" (menu-bar dedupe in `set_menu`, scroll/mouse-move frames coalesced onto the display link, find panel
toggle/Escape/close button). Do not regress any of those; their tests must keep passing.

## 1. Opening files from macOS (Finder "Open With", double-click, Dock drop, drag and drop)

Today `Velocity.app` declares document types but opening a file from Finder does nothing useful.
- The packaged launch re-execs a GUI child (`VELOCITY_GUI_PROCESS=1`) and the parent exits. LaunchServices delivers
  the open request (Apple Event `odoc` / `application:openURLs:`) to the process it launched, so the request is lost.
  When the process is launched by LaunchServices (no path arguments; detect robustly, e.g. parent pid 1 and/or the
  `-psn_*` argument, documented in code), run the GUI in-process instead of re-exec, and keep the CLI path unchanged.
- Implement `NSApplicationDelegate` `application:openURLs:` (and `application:openFiles:` if needed) and
  `applicationShouldHandleReopen:hasVisibleWindows:` (Dock click with no window → new window). Requests that arrive
  before the first window exists are queued and applied once it does; requests on a running instance open in the
  key window. Files open in a tab (focus the existing tab if already open; large files and binaries take their
  existing paths); folders are added to the side bar like `velocity <dir>`.
- Info.plist (`crates/velocity/packaging/bundle.py`): keep the text types with role Editor; add `public.data` with
  `LSHandlerRank` Alternate so any file offers "Open With → Velocity", and `public.folder` (Viewer, Alternate).
- Drag and drop: files/folders dropped on the window (editor, tab bar, side bar) open / add folders
  (`registerForDraggedTypes` with file URLs). Dropping text keeps its current behaviour.
- Tests: headless unit tests of the open-request router (files vs folders, duplicates, missing/unreadable paths with
  an error message, queued-before-window, binary/huge files); MANUAL_TEST.md checklist for Finder Open With,
  double-click (after "Change All…"), Dock drop, window drop, and a running-instance open.

## 2. Native context menus (resource driven, clean room)

Replace the custom in-app right-click popup with native `NSMenu` context menus.
- Resources in `Packages/Default` written from scratch (our own captions; do not copy Sublime's files):
  `Context.sublime-menu` (editor), `Tab Context.sublime-menu`, `Side Bar.sublime-menu` (files and folders). Users and
  packages can extend them through the existing resource layering.
- Show the menu with `NSMenu popUpContextMenu:withEvent:forView:` **after** the mouse event dispatch has returned
  (the menu runs a nested tracking loop; never re-enter the host while it is borrowed). Right button, two-finger
  click and Control-click all open it. Items run named commands with their args plus context (`paths`, `group`,
  `index`, `event` point); `is_enabled`/`is_visible`/`is_checked` follow `command_enabled` per item. Context menus
  never go through `set_menu` (that installs the menu bar and is deduplicated).
- Editor: a right-click outside every selection moves the caret to the click point first; inside a selection the
  selection is kept (today the popup always calls `set_caret`, so "Copy" after a right-click loses the selection).
  Items: Cut, Copy, Paste, Select All; spelling suggestions for the word under the pointer; the existing diff-hunk
  and Git items (from `review_ui.rs`); Copy File Path, Reveal in Side Bar, Reveal in Finder.
- Tab: Close, Close Other Tabs, Close Tabs to the Right, Close Unmodified Tabs, New File, Copy File Path,
  Reveal in Side Bar, Reveal in Finder.
- Side bar (Rust commands, no plugin API): Open, New File…, New Folder…, Rename…, Delete (confirmation, then move to
  the Trash with `NSFileManager trashItemAtURL`; never unlink), Copy Path, Copy Relative Path, Reveal in Finder,
  Find in Folder… (opens Find in Files with the folder as Where), Remove Folder from Side Bar, plus the existing Git
  items. Name input uses the existing input panel. Renaming an open file retargets its tab; deleting an open file
  keeps the buffer and marks the tab as deleted (like an external delete). Errors show in the status bar/dialog.
- Accessibility: menus are native, so VoiceOver works; keep the round-13 AX tree consistent.
- Tests: menu resource parsing and layering, enable/visible/checked evaluation, command routing with context args,
  side bar file operations in a tempdir through an injectable trash/clipboard/reveal trait, tab close variants,
  rename/delete of open files, right-click caret rules.

## 3. Round-12 native bugs (see `docs/review/round12-native-review.md`)

1. `velocity file:line[:col]` must reveal the line (center it, like Sublime), including at startup and over IPC.
2. Paint the strip above the side bar (level with the tab bar) with the theme's colours.
3. Window title: `<file name>` + dirty marker + ` — <project>` when one is open (Sublime convention), never
   "native AppKit".
4. UI font: use the system UI font API (`CTFontCreateUIFontForLanguage` / `NSFont systemFontOfSize:`); never request
   private `.SFNS-*` names (they fall back to Times).
5. Interactive telemetry is off by default. `--log-input` (and any other measurement switch) writes
   `interactive-<runid>-<pid>.jsonl` to `$VELOCITY_LOG_DIR` if set, else `~/Library/Logs/Velocity/`; never to a
   compile-time source path. Lane B's scroll gate relies on exactly this contract.

## 4. Small per-frame costs in `velocity`

- `Workspace::poll` stats every configuration/package stamp each frame: check at most every 500 ms (an explicit
  reload request still runs immediately) and keep existing hot-reload tests green.
- Anything else you find with a real profile that is per-frame and O(files) or O(tabs).

## Rules

- Your worktree: /Users/zoro/dev/velocity-lane-a (branch `lane-a`). Do not commit. Lane-A crates (+ root
  `Cargo.lock`). Lane B works on `vel-render`, `vel-highlight`, `vel-text` and a new `tools/scroll-gate/` in another
  worktree; do not edit those.
- No GUI/Metal in the sandbox: every native piece gets a headless-testable core plus a MANUAL_TEST.md entry.
- Keep typing/scroll within 5% (paired A/B against main). fmt, clippy `-D warnings`, `cargo test --workspace
  --release` with default parallelism, rustdoc `-D warnings`.
