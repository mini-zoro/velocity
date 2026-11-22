# Windows x64 port

The supported build target for this pass is `x86_64-pc-windows-msvc` on the
`windows-2022` GitHub runner. Build, Clippy, package validation and tests do not
require an interactive desktop or GPU. W1 supplied the portability foundation;
W2 supplied the Win32 host; W3 integrates the editor and packages it.

## W3: editor and distribution

- **Editor:** The graphical Settings sheet, resource menus and accessibility
  model are shared with macOS. Indexing Status has its own DX12 window. The font
  chooser and minihtml measurements use the Windows Swash font catalog. Local
  image previews and HTTPS minihtml images keep the bounded decoder subprocess.
- **Input:** The Windows defaults and user override are both named
  `Default (Windows).sublime-keymap` in their respective Default and User
  packages. The User platform file follows the neutral `Default.sublime-keymap`.
  Physical Ctrl remains the app's logical Command bit. In Windows resources,
  `ctrl` and `primary` translate to that bit, `alt` stays Alt, and `super` denotes
  the OS-reserved Windows key rather than a Ctrl alias. Ctrl+P opens Goto
  Anything; Ctrl+Shift+P opens the palette; F3 / Shift+F3 find next / previous;
  F11 toggles fullscreen. Ctrl+Tab cycles tabs by recency, Ctrl+Page Up/Down
  cycles visually, and Ctrl+F6 / Ctrl+Shift+F6 cycle editor windows. Ctrl+K,
  Ctrl+, opens Settings; Ctrl+Q exits the application. Alt+F4 closes the
  active window through Win32.
- **Menus:** File, Edit, Selection, Find, View, Goto, Tools, Project,
  Preferences and Help. Alt or F10 enters the menu bar. The stable access
  keys are F, E, S, I, V, G, T, P, N and H respectively, including in
  Simplified Chinese. Exit is in File, window actions are under View >
  Window, and About is in Help. Preferences contains Settings, Key Bindings,
  Color Scheme and Theme. Shortcut labels follow the effective Windows keymap.
- **Executables:** `velocity.exe` is the GUI subsystem executable for Explorer
  and shortcuts. In cmd or PowerShell, use adjacent `vel.exe` for files,
  projects, line/column, `--wait`, stdin and redirected output. On a cold start
  the helper starts the GUI and waits for its local pipe, without sending stdin
  or wait state on a command line. The same pipe handles warm requests and
  close replies. Closing sheets completes `--wait`; stdin implies wait and
  returns edited bytes on stdout. Ctrl+C interrupts the waiting helper.
- **Distribution:** On Windows, run `./scripts/package-windows.ps1`. It builds
  the pinned x64 MSVC release and stages
  `Velocity-<version>-windows-x64.zip` and
  `Velocity-<version>-windows-x64-setup.exe`. Use `-PortableOnly` for a ZIP
  without Inno Setup. Both executables have the Velocity icon and Cargo version
  resource. The ZIP includes both binaries, bundled resources, the public
  language corpus (not Sublime's Default UI package), licenses and unsigned
  installation guidance. The per-user installer adds a Start menu shortcut,
  alternate Explorer Open with registration for common text formats, and an
  unchecked option to add `vel.exe` to the user's PATH. It does not change the
  user's default handlers. [Inno Setup is installed on the runner image](https://github.com/actions/runner-images/blob/main/images/windows/Windows2022-Readme.md).

## Not on Windows

- Finder Sync, the Finder New Text File command, LaunchServices and Dock
  behavior, the Services menu, application Hide/Hide Others, and the system
  Find pasteboard are macOS-specific. The in-app macOS CLI installer is replaced
  by the setup program's PATH task. Windows does not restore terminal focus
  after a wait reply.
- W2 does not expose the editor's virtual accessibility tree to UI Automation.
  There is no custom Jump List, TSF integration or precision touchpad
  gesture/momentum model; the Windows host uses automatic recent documents,
  IMM32 and legacy pointer messages.
- WebP previews still use macOS ImageIO. On Windows the file opens without a
  raster preview. PNG, JPEG, GIF and TGA file previews use the portable decoder.
- Native timing and scene-generation modes that depend on Metal/AppKit,
  scanout, Darwin footprint or retired-instruction counters remain macOS-only.
  Windows crash capture is panic-only. Windows release artifacts are unsigned;
  there is no Authenticode signing or equivalent of Apple notarization.

## Verification boundary

The local `/tmp/wincheck/check.sh` checks the Windows target and its test
targets from macOS. Its fake C compiler is suitable only for type checking
and Clippy. It cannot link, run or package Windows executables.

`windows.yml` builds the workspace with native MSVC, runs Clippy and release
tests, validates the packaged ZIP and compiles the installer, then uploads
`velocity-windows`. Windows tests check device-free components, the
production executable's headless mode and console forwarding through a real
named pipe to a headless peer. Legacy App unit fixtures remain macOS-only;
they contain macOS-specific key, clipboard, filesystem and lifecycle
assumptions. The tag workflow builds and packages both platforms before
publishing, and hashes every release asset in the combined `SHA256SUMS`.

No Windows desktop or installed copy was tested locally. CI does not exercise
DX12 window creation or presentation, IME/candidate placement, multi-monitor
DPI, keyboard menu tracking, real Shell dialogs/Explorer integration,
SmartScreen, install/uninstall behavior, or helper cold start into a live GUI.
See [the backend handoff](../crates/vel-platform/WINDOWS.md) for the desktop
smoke checklist.

## W1: platform behavior

- **Profile:** `%APPDATA%\Velocity`. `VELOCITY_DATA_DIR` and an explicit `--data-dir`
  still override it. Logs go to `%LOCALAPPDATA%\Velocity\Logs` (or
  `VELOCITY_LOG_DIR`); the shared index cache goes to
  `%LOCALAPPDATA%\Velocity\Cache\index`. Temporary files use the OS temporary
  directory. Display omits verbatim drive and UNC prefixes without changing the
  I/O paths; comparisons for document identity, recent entries and project
  rebasing use case-insensitive path components on Windows.
- **Resources:** Package IDs always use `Packages/<package>/<file>` with `/`,
  including files read from loose packages, archives and the embedded inventory.
  The embedded language inventory is sorted by those IDs for binary lookup.
  The repository checks out its committed bytes on all platforms; this does not
  constrain user files. CRLF settings, keymaps, menus, localization and syntax
  definitions are covered by platform-independent tests. The pinned upstream
  submodule has its own checkout attributes; corpus results use stable relative
  paths independently of its checkout and of the host path separator.
  Windows watches user Preferences paths even before they exist, so a first
  atomic write cannot be missed solely because the directory stamp did not tick.
- **Workspace paths:** Registered Windows folder roots and explicit Find in Files
  roots use the same canonical form as open documents. After a Trash move the
  surviving ancestor still supplies the canonical prefix for project updates.
  Find in Files accepts verbatim drive/UNC roots without treating the prefix's
  `?` as a wildcard. Syntax lookup accepts native separators while corpus and
  package names stay slash-separated.
- **Language and hints:** Follow System uses the user's preferred Windows UI
  languages. Quick panels show Enter, Esc and Ctrl+Enter on Windows; macOS
  keeps its key glyphs. Structural scene tests retain the full headless snapshot
  using fixed metrics and expect the native shortcut and newline labels.
- **CLI:** Forwarding uses a local named pipe keyed by the user SID and
  normalized profile path. Its protected DACL allows the current account and
  rejects remote clients. The client checks the pipe owner before sending.
  The Unix length-prefixed JSON protocol, enqueue/busy replies and separate
  wait/edited-stdin replies are unchanged.
- **Builds:** `shell_cmd` runs under `cmd.exe /D /V:OFF /S /C`, with no login
  script. Explicit argument vectors use Windows quoting and resolve executables
  from the effective PATH/PATHEXT. A process joins a private Job Object at
  creation, before it can start a descendant. Builds create no console window;
  redirected streams retain valid UTF-8 and replace malformed bytes.
- **Files:** Windows uses synced sibling stages and same-volume
  `MoveFileExW` publication. Failed validation, encoding, or publication
  leaves the existing destination intact. Existing document DACLs are copied on
  filesystems with persistent ACLs, and read-only destinations are rejected.
  Validation and Replace in Files recovery handles close before `MoveFileExW`
  replaces the destination; an external handle may still prevent replacement.
  Replace in Files keeps its byte-exact recovery copy on a save error because it
  can no longer verify that the same file is in place. An exclusive rename refuses
  a distinct existing hard-link name, but still permits case-only changes to
  the same directory entry.
  Windows does not run the Unix in-place save journal or preserve hard-link
  identity: it never truncates the live file for a save. Neither cross-volume
  copy nor remove-then-rename is used as a publication fallback.
  `default_line_ending: "system"` means CRLF; detected conventions are retained.
  The existing notify backend selects `ReadDirectoryChangesW` on Windows, with
  the same stat fallback.
- **Binary snapshots:** On Windows the private copy can remain sparse when the
  temporary volume supports it. Zero spans need no disk writes; reads still use
  a checked source descriptor and the mapped copy is independent of truncation
  or later writes to the source. The headless test keeps its real 4 GB tail.
- **Fonts and graphics:** The Windows font catalog scans system and per-user
  font files. The editor default is Consolas; UI and CJK preferences are Segoe UI
  and Microsoft YaHei respectively, with fallback based on the available faces
  and glyph coverage. Swash shapes and rasterizes the selected faces.
  The Windows wgpu backend is DX12 only; Metal remains enabled only on macOS.

## W1: implementation notes

- W2 supplies input, IME, menus, clipboard, dialogs and the native event loop.
  A renderer borrows a live HWND and is destroyed before its window. The
  native Trash adapter calls `vel_fileio::trash`, which asks the Shell
  to recycle and does not suppress a separate confirmation for permanent deletion
  when recycling is unavailable. Legacy Shell path-length restrictions apply.
- Crash reporting is **panic-only** on Windows. It writes the same local
  `.crash` format and keeps the same pending-report handling. Structured
  exceptions (such as access violations) and stack overflows are not captured.
  The Windows build reports process CPU and working set; macOS physical
  footprint and retired-instruction measurements do not have Windows equivalents
  in this pass.
API notes: [named pipe access](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights),
[Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects),
[the Recycle Bin operation](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shfileoperationw).
