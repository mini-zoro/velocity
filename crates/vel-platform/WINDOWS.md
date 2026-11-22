# Windows backend (W2)

The production Windows editor uses the same App as macOS. Tests do not create
windows. The native backend assumes Windows 10/11-era desktop APIs; the build
target is x86_64-pc-windows-msvc. W1 defines the profile, pipe and DX12 surface.

## Native lifecycle

- Each top-level HWND owns one handler. WM_CLOSE runs the app's window-close
  flow; application Quit asks all open handlers. Closing the last window keeps
  a hidden template so the W1 pipe can still open a new window. A renderer is
  dropped on the GUI thread before its HWND is destroyed. The hidden template's
  renderer is retained until the next window opens or the process quits.
- The HWND is per-monitor DPI aware v2. Client input and layout use logical
  points; surface/IME positions use physical pixels. Saved outer frames are
  physical virtual-screen rectangles, clamped to current work areas.
- Frames use a message-aware timer paced at the monitor's reported refresh
  rate, with a 60 Hz fallback and FIFO presentation through W1's DX12 renderer.
  Dirty frames and one-shot deadlines coalesce. Minimized windows skip drawing
  but poll CLI work. A successful DX12 present return starts the remaining app
  services; it is not recorded as scanout. Native measurement modes that
  require Metal presentation observations remain macOS-only.
- The original Windows console subsystem is unchanged for W1 CLI and redirected
  streams. Selecting a packaged GUI/CLI entry point is not part of W2.

## Input and menus (W3 handoff)

- Physical left/right Ctrl becomes logical left/right **command** (the "super"
  bit) in KeyEvent and in mouse modifiers. Shift and Alt become shift and
  option. The Windows key remains reserved for the OS. No event sets the
  separate Mac Control bit as well as the Command bit; Ctrl-click is therefore
  not a secondary click. Windows resource chords for Ctrl must resolve to the
  logical super bit until W3 adds a platform-specific keymap translation.
  Ctrl+Alt synthesized by AltGr is used for text, not a shortcut. Keyboard
  layout and dead keys are left to Windows, with named non-printing keys
  translated to the existing keycode vocabulary.
- The app keymap is offered each eligible key before Win32 text translation.
  Standard editing/navigation commands are the fallback. OS system chords
  such as Alt+F4 remain native. Ctrl+Q and Ctrl+M are native menu fallbacks
  when the app keymap does not handle them.
- IMM32 supplies inline preedit, result and cancellation; the candidate window
  is anchored to the app's caret in the focused window. This is the IMM32
  compatibility path, not a TSF implementation. Pinyin and switching IMEs
  require a desktop smoke test.
- Wheel sub-detents preserve fractional motion. Windows' legacy wheel messages
  do not reliably identify precision touchpads or expose separate gesture and
  momentum phases. Pointer input covers mouse capture, double/triple-clicks,
  I-beam, arrow and standard non-client resize cursors.
- HMENU entries retain their original command name and JSON arguments, validate
  visibility, enabled and checked state on open, and show shortcuts after a
  tab. Mac-only Services/Hide entries are omitted. Window commands use native
  minimize and maximize; Help links use the default browser. Context popups
  track after button release, not while the app callback is borrowed.

## Shell and limits

- Native Unicode and HTML clipboard, file/folder/save dialogs, and localized
  alert buttons. Cancelling an alert does not select a real button. If the
  Common Controls task dialog is unavailable, use a native dialog template
  with the same button labels. Windows has no shared Find pasteboard.
- Explorer file drops go to the receiving window. Startup paths and secondary
  invocations use existing CLI parsing and the profile-scoped W1 named pipe.
  The optional WM_COPYDATA entry point accepts only the tag 0x56454c01 and up to
  64 KiB of NUL-separated, final-NUL-terminated UTF-16LE absolute filesystem
  paths; it accepts no flags or commands. It does not relax Windows' message
  filtering. Use the authenticated named pipe for CLI/profile routing.
- Explorer reveal, Recycle Bin via W1, default-browser URLs, monitor
  fullscreen/restore, and DWM title theme are wired. DWM may ignore custom
  colors on older Windows builds. There is no macOS Finder extension on
  Windows.
- Explorer gets automatic recent-document entries when enabled by the app;
  W2 does not own a custom Jump List or clear other apps' history. A taskbar
  indicator shows an active build, without a fabricated completion percentage.
  High contrast and reduced-motion preferences reach the app, but its virtual
  accessibility tree (including editor text, selection and focus) is not yet
  exposed to UI Automation.

Headless Windows CI can check the pure DPI, frame scheduling, key, IME, mouse,
menu, clipboard and open-message models. It should compile/link the Windows
production binary as well as run tests. A desktop check is still needed for
DX12 startup and resize across two DPIs, click/drag/precision scrolling,
Pinyin commit/cancel, fallback alerts, Shell dialogs, and IPC while minimized
or after closing the last window.

References: [IME composition](https://learn.microsoft.com/en-us/windows/win32/intl/wm-ime-composition),
[keyboard translation](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-tounicodeex),
[taskbar state](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-itaskbarlist3-setprogressstate).
