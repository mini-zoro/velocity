# vel-platform

Native AppKit platform layer for Velocity (MIT OR Apache-2.0). Requires macOS 14+;
non-macOS builds expose the coordinate/input API and an unsupported-platform stub.
No winit, raw-window-handle window creation, renderer dependency or vel-buffer dependency.

`Platform::run(app)` / `run_with_options` own NSApplication, a retained delegate,
NSWindow, a layer-hosting NSView, and an observed CAMetalLayer. WindowHandle clones
retain the owner and are main-thread-only. The renderer must retain a handle until
its unsafe CoreAnimationLayer surface is destroyed. App callbacks are serialized;
reentrant native notifications are queued rather than borrowing the app twice.
The renderer/surface is dropped before detaching the view on shutdown.

The regular activation policy supports terminal launches without a bundle. A minimal
App/Edit menu routes About/Hide/Quit to AppKit and editing actions through the first
responder. Window tabbing is disabled; fullscreen is native. Backing/size/screen
changes update frame, contentsScale and drawableSize with implicit animations disabled.
Live resize draws synchronously. Effective appearance changes reach the app. Text
uses an I-beam cursor; pasteboard read/write uses NSPasteboardTypeString.

## Startup and pacing

Both startup frame callbacks run inside applicationDidFinishLaunching:

1. Construct hidden window/layer; initialize fonts, surface and pipeline in on_start.
2. StartupPrepare builds/uploads/submits the complete text frame to an offscreen texture.
3. orderFront, then StartupPresent immediately acquires/copies/presents it in the same callback.
4. Start dirty-only, view-bound CADisplayLink pacing. Pause the link while idle or occluded.

Reentrant resize/updateLayer notifications cannot draw before the explicit startup
present call. If orderFront constrains the size or changes scale, the app rebuilds
the prepared texture immediately before that presentation and logs the extra phase.
Hidden layers need not provide a drawable. No display-link tick or redraw-event round
trip gates the first presentation. Surface acquisition failure uses a one-shot retry,
recorded separately. Native key/text input also flushes a dirty frame immediately;
scroll/animation can request the screen's maximum refresh rate. Timers use common
run-loop modes and do not keep an idle display link running.

ObservedLayer overrides nextDrawable only to attach MTLDrawable.addPresentedHandler.
It calls the superclass and returns the same drawable to wgpu. No private wgpu fields,
swizzling, or alternate drawable acquisition are used. Handler state is Arc/Mutex +
atomic sequence; callbacks never touch AppKit or the editor. Positive presentedTime
is translated from CACurrentMediaTime to Instant; callback receipt is separate. The
queue is bounded. nextDrawable timeout is enabled, and automatic modes have an
independent watchdog even if a driver call blocks.

## NSTextInputClient

All required methods are implemented: insertText with replacementRange, setMarkedText
with relative selectedRange and document replacementRange, unmarkText, hasMarkedText,
markedRange, selectedRange, attributedSubstring/actualRange, validAttributes,
firstRect/actualRange, characterIndexForPoint, and doCommandBySelector. NSString and
NSAttributedString are accepted. NSNotFound stays None. UTF-16 conversions use vel-text;
split-surrogate nonempty ranges expand to scalar boundaries. Queries include inline
marked text. The app implements snapshot, range geometry and hit-testing callbacks.

Each keyDown goes through interpretKeyEvents first. Physical key observers then receive
code, both character strings, raw modifier bits including device-dependent left/right,
repeat, and native/monotonic timestamps. KeyUp and flagsChanged are also delivered.
Standard selectors map deletion, newlines, tabs, selection movement, word/document
movement and clipboard/history actions to neutral commands.

View geometry uses top-left logical points. AppKit convertRect:toView: plus
convertRectToScreen: handle the flipped view, Retina, window origin and multiple
screens. The reverse path handles characterIndexForPoint. No manual Retina multiplier
is used for candidate windows. Offscreen anchors are clamped to the content area.

Kernel process birth, RSS and TASK_VM_INFO REV1 physical footprint need no window
server. Their ABIs and error/short-reply checks are tested headlessly, along with
UTF-16 range handling. Native delivery, press-and-hold, reconversion, resize and
multi-display behavior require the hardware checklist in velocity/MANUAL_TEST.md.


WindowHandle exposes current keyboard focus, effective appearance and the last acquired
drawable sequence. The editor uses these to paint inactive selections, select the initial
scheme before first presentation, and correlate exact highlighted frames with native scanout.
All three have deterministic unsupported-platform stubs.


Round 7 adds `WindowHandle::new_window` on the current AppKit loop, `request_close`/`close`,
`toggle_full_screen` and a separate `AppHandler::on_window_close_requested` hook. Windows own
independent hosts/layers/renderers. Application quit validates every live window before stopping
the loop; a window close releases its editor/worker state while siblings remain alive. Round 14
retains the final closed handler as a reopen/IPC template, as described below. Native
window tabs remain disabled. Unit tests still construct no AppKit or Metal objects; the
multi-window/full-screen bridge is compiled here and requires the velocity human checklist.


Round 10 adds `WindowHandle::set_title_bar_theme`: appearance is applied to the native title-bar
view, preserving OS appearance notifications for the document. None restores system appearance;
a supplied background uses straight-alpha sRGB. This compiles headlessly; visual title-bar and
multi-display behavior require the native reviewer checklist.

## Finder, Dock, and closed-window lifecycle

`application:openURLs:` and `application:openFiles:` enqueue path batches until a host has
completed `on_start` and startup presentation. All currently queued batches are drained into
one ordered path list before `on_open_paths` runs, so a one-shot LaunchServices proxy can
forward the complete payload and quit after acknowledgement. Requests route to the current key host,
falling back to a live host when all windows are minimized. The content view accepts file
URLs across editor, tabs, and side bar; opening a drop never moves the source. Text drops
are not interpreted as paths. URL-to-path conversion preserves filesystem bytes.

New `AppHandler` callbacks (all default to no-ops):

```rust,ignore
fn on_open_paths(&mut self, window: &WindowHandle, paths: &[std::path::PathBuf]);
fn on_reopen(&mut self, window: &WindowHandle);
fn on_idle(&mut self, window: &WindowHandle);
fn command_visible(&self, name: &str, args: &str) -> bool; // default true
fn command_checked(&self, name: &str, args: &str) -> bool; // default false
```

Closing the last window leaves the process running and retains its handler/handle as a
template. Ordinary input, drawing, and display-link ticks stop for that host. A Dock reopen
or an external open first calls `on_reopen` on the template, which should construct a fresh
window using `window.new_window(...)`; queued paths then reach that new host's
`on_open_paths`. The app owns path validation, file/folder/binary/large-file routing, tab
deduplication, and error presentation. The launcher must preserve LaunchServices events by
running its GUI in-process when launched by LaunchServices.

While no live windows exist, a 100 ms main-thread timer calls `on_idle` without calling
`on_frame` or acquiring drawables. Use this to poll IPC and force incoming requests into new
windows. The timer stops when a live window launches. Explicit `quit` still stops the loop.

Alternatively, register a process-wide factory with
`WindowHandle::set_window_factory(factory)`, where `factory` is
`impl FnMut() -> Result<(WindowOptions, Box<dyn AppHandler>), String> + 'static`.
It is independent of closed handler state and runs if `on_reopen` did not create a window.
No factory or app-state borrow crosses factory invocation. Failed creation retains queued
opens and waits for another request instead of retrying on every idle tick.

## Deferred native context menus and file services

`WindowHandle::show_context_menu(&[MenuEntry]) -> bool` queues a native popup; the return
value indicates acceptance, not a selected command. The platform captures native mouse-down
events, including primary presses for Git status menus. An origin survives `on_mouse`, so
`on_frame` may offer a menu after a spelling worker completes. A later mouse press replaces
the origin and cancels any queued old popup; displaying a popup consumes the origin. Apps
must cancel stale pending worker menus on a subsequent click as well.

`WindowHandle::cancel_context_menu(&self)` clears the retained click origin and any queued
popup when tracking has not started. Call it alongside clearing App's pending menu on key
down, text input, standard selector commands, and scrolling. A selector that was already
scheduled then has no popup to display. Cancellation only changes Rust state and does not
enter AppKit or dismiss an actively tracking menu; the unsupported-platform stub is a no-op.

`MouseEvent::is_context_click()` recognizes secondary/trackpad and Control-primary presses.
Check it before primary caret/selection handling. The app supplies all context in the
existing opaque `MenuEntry::args` JSON string. No `MenuEntry` fields have changed.

Tracking runs on a later default-mode main-run-loop turn, after every host borrow ends.
Enabled, visible, and checked flags are snapshotted using `command_enabled`,
`command_visible`, and `command_checked` with those exact args. Hidden entries and empty
submenus are removed, and separators normalized. Each native target owns its command/args
independently of menu-bar tags. A selection is delivered through `on_named_command` after
native tracking returns. Context popups never call `set_menu`; its owner/content cache and
display-link scroll/mouse-move coalescing are preserved. Menu-bar flags update before a
menu opens, without defeating installation deduplication.

`WindowHandle::trash_file(&Path) -> Result<(), String>` calls
`NSFileManager::trashItemAtURL` with no unlink fallback. Confirmation belongs to the app.
`WindowHandle::reveal_file(&Path) -> Result<(), String>` selects the item in Finder.
`NativeFileOperations` exposes `trash_file`, `reveal_file`, and `copy_text` for injection in
headless file-command tests. Unsupported-platform methods report errors and never delete
items. Existing clipboard methods remain available. `set_title(&str)` now compares its
cached title before allocating native strings or messaging AppKit.

Focused verification: `cargo test --locked -p vel-platform --lib`,
`cargo clippy --locked -p vel-platform --all-targets -- -D warnings`, and
`RUSTDOCFLAGS='-D warnings' cargo doc --locked -p vel-platform --no-deps`.
Native behavior and performance require [MANUAL_TEST.md](MANUAL_TEST.md); unit tests create
no native windows, menus, or Metal resources.
