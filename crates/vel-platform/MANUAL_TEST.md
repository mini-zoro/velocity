# Round 14 native macOS verification

Run on macOS 14+ outside the headless sandbox with the integrated, packaged application.
The platform's unit tests do not construct AppKit or Metal objects. LaunchServices/packaging,
resource content, tab behavior, and file classification are supplied by the application.

## External opens, drop, and lifecycle

- [ ] Cold-start Finder **Open With → Velocity** with a text file and with a folder. Verify
  that the path request reaches `on_open_paths` after `on_start` and the initial window exists.
- [ ] Set Velocity as the default with **Change All…**, then double-click a file. Verify the
  GUI stays in the LaunchServices-launched process and the request is not lost to re-exec.
- [ ] Drop multiple files/folders on the Dock icon, both before launch and while running.
  Include spaces, `%`, `#`, non-ASCII names, a binary, and a huge file. Confirm ordinary app
  validation/status errors for missing or unreadable paths and existing-tab focusing.
- [ ] With an existing primary instance, deliver multiple native open batches to a secondary
  LaunchServices proxy before its window is ready. Its one forwarded callback must contain
  every queued path in arrival order before it acknowledges delivery and quits.
- [ ] With two windows, alternate the key window and issue Finder/Dock opens. Each request
  should reach the current key host. Minimize all windows and verify an open restores a
  live destination rather than creating a duplicate startup host.
- [ ] Drop multiple file URLs on editor, tab bar, and side bar. Verify files open, folders are
  added, and Finder source entries remain in place. Text-only drags keep existing behavior;
  web URLs and ordinary text are not opened as filesystem paths.
- [ ] Close the final window, including an unsaved-change cancellation followed by approval.
  The process should remain alive with no visible windows. Click the Dock icon: `on_reopen`
  should create a fresh host, view, and renderer through App's window constructor.
- [ ] With no windows, use Finder/Dock to open files. Reopen must run first; all queued
  requests should then reach the fresh host. Repeat closing and reopening several times.
- [ ] With no windows, send a CLI/IPC request. Verify `on_idle` polls it within the 100 ms
  timer cadence and creates a new host, without `on_frame` or drawable acquisition on the
  closed template. Verify the idle timer stops after a live host launches, and Quit exits.
- [ ] Exercise a failed reopen/factory construction. Requests must remain queued and a
  failure must not start a repeating modal error/reopen loop on every idle tick.

## Native menus, state, and reentrancy

- [ ] Open editor, tab, and side bar context menus with right button, two-finger trackpad
  click, and Control-primary click. Primary-click the Git status badge for its native popup.
- [ ] Right-click inside a selection and copy; verify the selection stays intact. Right-click
  outside every selection and verify the app moves the caret before constructing the menu.
- [ ] Request a spelling suggestion that finishes 200–250 ms after the click. Verify the
  popup uses the retained original native event/point even after several worker poll frames.
  Click elsewhere before it finishes: the stale menu must be cancelled by App and platform.
- [ ] While a spelling menu is waiting or its native popup selector is queued, type, invoke
  a standard editing selector, or scroll. App must clear its pending result and call
  `cancel_context_menu`; no delayed popup may appear. A subsequent fresh click still works.
  Cancellation must not force an already-tracking native menu to dismiss.
- [ ] Enable/disable, hide/unhide, and check/uncheck commands with different JSON args. Verify
  the correct state, submenu visibility, and separator cleanup, including after reopening a
  menu. Confirm `paths`, `group`, `index`, and `event` arrive unchanged in the chosen command.
- [ ] Keep a menu open while background work updates/redraws the app and menu bar. No
  `RefCell already borrowed` panic, stale menu-bar-tag dispatch, or duplicated command may
  occur. Set a breakpoint at `popUpContextMenu`: no host App borrow may be active, and a
  chosen command must reach App only after tracking returns.
- [ ] Verify application menu installation remains deduplicated for identical owner/content
  requests. Context menus must not rebuild the menu bar. VoiceOver should announce the
  native menu, its checkmarks, disabled items, and submenus; the editor AX tree stays valid.

## File services, title, fonts, and pacing

- [ ] Confirm a side bar deletion, then recover the item from the system Trash. Test files,
  folders, symlinks, Unicode paths, and a permission-denied failure. Cancellation changes
  nothing; failure must never permanently unlink an entry. Test App's open-buffer retention.
- [ ] Copy a file path through the native clipboard bridge and reveal it in Finder. Verify
  clipboard failure and a missing reveal path surface through App's error reporting.
- [ ] Change files, dirty/clean state, and projects. Verify the requested title appears and
  repeated identical frame updates do not call `NSWindow::setTitle` or allocate NSStrings.
- [ ] Once the separate renderer font fix is integrated, verify system UI text has no Times
  substitution. Inspect sidebar/menu-adjacent UI at several sizes with regular, bold, and
  italic styles; no private `.SFNS-*` name should be reconstructed through a name lookup.
- [ ] Repeat the paired typing/scroll measurements against the baseline on real hardware;
  stay within the 5% budget. Scroll and mouse-move/drag events remain display-link coalesced,
  clicks/keys still draw immediately, and idle/occluded display links stay paused.
