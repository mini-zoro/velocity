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
  Windows does not run the Unix in-place save journal or preserve hard-link
  identity: it never truncates the live file for a save. Neither cross-volume
  copy nor remove-then-rename is used as a publication fallback.
  `default_line_ending: "system"` means CRLF; detected conventions are retained.
  The existing notify backend selects `ReadDirectoryChangesW` on Windows, with
  the same stat fallback.
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
