# Windows x64 portability (W1)

The supported build target for this pass is `x86_64-pc-windows-msvc` on the
`windows-2022` GitHub runner. Build, Clippy and tests require neither a window nor a
GPU. The native window backend and the installer/release job are separate work.

## Platform behavior

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

## Handoff and limits

- Lane C owns input, IME, menus, the clipboard, dialogs, accessibility and the
  native window/event loop. Pass a live HWND and a physical pixel size to
  `vel_render::Renderer::from_hwnd`; destroy the renderer before the window.
  While a font fallback is pending, the event loop should request another frame.
  The native Trash adapter can call `vel_fileio::trash`, which asks the Shell
  to recycle and does not suppress a separate confirmation for permanent deletion
  when recycling is unavailable. Legacy Shell path-length restrictions apply.
- Crash reporting is **panic-only** on Windows. It writes the same local
  `.crash` format and keeps the same pending-report handling. Structured
  exceptions (such as access violations) and stack overflows are not captured.
  The Windows build reports process CPU and working set; macOS physical
  footprint and retired-instruction measurements do not have Windows equivalents
  in this pass.
- Native Windows presentation and Shell behavior are not tested in W1. First CI
  checks should cover named-pipe ownership and close replies, Job Object
  assignment under the runner's own jobs, NTFS sharing and rename behavior,
  Windows font availability, and DX12 dependency linking. No package is built or
  published until the native integration round.

API notes: [named pipe access](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights),
[Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects),
[the Recycle Bin operation](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shfileoperationw).
