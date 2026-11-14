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
