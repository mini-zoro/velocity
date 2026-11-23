# Windows desktop checks

Use **Windows Desktop** in Actions (`workflow_dispatch`) or push the commit to
the `windows-desktop` branch. GitHub only offers manual dispatch after the
workflow exists on the default branch; the branch push works before that.
The ordinary Windows build and the release workflow are unchanged.

The packaging job runs `package-windows.ps1` once. Both desktop jobs download
and check the SHA-256 sums of the same portable ZIP and per-user installer.
Each check gets fresh `VELOCITY_DATA_DIR` and `VELOCITY_LOG_DIR` directories.
The ZIP is extracted under a test directory with spaces and CJK in its name.
The CLI check also sets a different environment profile to verify that
`--data-dir` takes precedence in both executables.

The driver uses the runner's Windows PowerShell 5.1 in an interactive STA,
Win32 input and screen capture, and the inbox UI Automation assemblies. It
does not install test dependencies or try to unlock an unavailable desktop.
Missing interactive access is a failure, not a successful skip. On a
disposable Windows machine, the same driver can be run with:

```powershell
powershell.exe -NoProfile -STA -File scripts/windows-desktop/run.ps1 `
  -PackageDirectory target/windows-artifacts -ResultDirectory target/desktop-results
```

Use an unlocked English Windows desktop, a US keyboard layout, and a clean
user account. The driver uses the real system clipboard and native shell.
The installer test refuses to run over existing Velocity installation state.
Use a new result directory per run.

## Checks

1. Portable launch, time to HWND and non-blank frame, title, class, DPI, adapter and working set.
2. Python highlighting in the keyword and identifier regions, with a CJK sample.
3. Scan-code typing, Unicode packets, navigation, Backspace, undo, redo and exact saved bytes.
4. Native Save As with a Unicode file name and the Windows default CRLF, without a BOM.
5. Each Win32 menu in English and Simplified Chinese: mnemonics, UIA items, Windows shortcut labels and unchanged English captions.
6. Palette, Goto, Find, Replace, Find in Files, Goto Line, multi-select, comments, sidebar chord, fullscreen and tab lifecycle shortcuts.
7. `vel.exe` help/version in cmd and PowerShell, profile-specific warm forwarding, line/column, wait, stdin/stdout, new window and cold start.
8. Multi-window move, resize, Alt+F4, File > Exit, dirty text and window restore, and no orphan process after Exit.
9. CJK clipboard in both directions.
10. Folder forwarding and keyboard open from the sidebar.
11. Both paired JSON settings panes and a graphical setting write to the User file.
12. Approximately 50 MiB of numbered text: time to the first content frame, working set and three Page Downs.
13. Silent install, shortcut, Explorer and uninstall registration, PATH, installed launch and uninstall cleanup.
14. Force-kill after a durable dirty-buffer autosave, then relaunch and verify recovered text.
15. Microsoft Pinyin candidate position and commit. On a hosted runner with the inbox Pinyin binary, the driver tries adding its user input profile and restores the original list in `finally`. Otherwise, or if activation needs a sign-in, report SKIP. No language-pack downloads or reboot.

## Evidence

Download `velocity-desktop-windows-2022` or `velocity-desktop-windows-2025`.
`report.json` and `summary.md` have timings, outcomes and PNG names. The
Markdown is also posted to the job summary. Screenshots under `screenshots/`
are cropped to the visible app frame (excluding invisible resize borders), widened for native menus, dialogs or the IME
candidate window as needed. The driver saves the entire desktop when the
assertion is that the app is no longer there. Non-visual shell and registry
assertions use process output and report entries. `logs/` contains per-check
telemetry, subprocess output, installer logs, UIA/native menu dumps, any
Velocity crash reports, and matching Windows Application events. A final
Application scan picks up delayed Windows Error Reporting events.

For startup failures, start with `logs/01-launch/portable.jsonl` and
`portable.stderr.log`. `dx12_adapter` records the name, device type and driver
before device creation; `Cpu` with `Microsoft Basic Render Driver` is WARP.
The pixel timings are measurements from the driver, not scanout timings.
Screenshots using `PrintWindow` are labeled as diagnostic fallbacks and are
not used as evidence of a composited frame.

The editor does not expose its rendered surface through UI Automation on
Windows. Its virtual panels are checked by changed pixel regions, with
file bytes, clipboard text or window geometry asserted where possible.
The first hosted run should establish whether both images expose a usable
interactive desktop and WARP, and whether popup UIA and pixel thresholds
need adjustment. Where UIA does not enumerate the menu, the report includes
the Win32 menu captions and records whether the popup opened by mnemonic or
click. A click fallback does not pass a failed mnemonic. Settings item
selection uses visible popup geometry or verifies the native keyboard highlight.
The stdin check handles the owned save prompt without writing a file and still
requires the edited text on stdout. Cold start runs from an isolated profile and
verifies both helper streams reach EOF while the editor is open.
Clipboard copies must advance the sequence and transfer ownership to the tested
window, even for identical text. Clipboard retries are counted per check.
Unsigned installer reputation, multi-monitor DPI changes
and immediate power-loss recovery before autosave are out of scope.
