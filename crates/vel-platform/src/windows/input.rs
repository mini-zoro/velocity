//! Unicode input and Win32 pointer messages.
use super::{Event, Host, WindowHandle, model};
use crate::{
    CursorShape, FrameReason, KeyEvent, KeyState, MouseEvent, MouseKind, ScrollEvent, TextInput,
    Utf16Range,
};
use std::{ptr::null_mut, rc::Rc, time::Instant};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::{ClientToScreen, ScreenToClient},
    UI::{
        HiDpi::GetSystemMetricsForDpi,
        Input::{Ime::*, KeyboardAndMouse::*},
        WindowsAndMessaging::*,
    },
};

impl Host {
    pub(super) fn offer_key(self: &Rc<Self>, message: &MSG) -> bool {
        let down = match message.message {
            WM_KEYDOWN | WM_SYSKEYDOWN => true,
            WM_KEYUP | WM_SYSKEYUP => false,
            _ => return false,
        };
        if !self.ready.get() || self.closing.get() {
            return false;
        }
        crate::memory_guard::note_input();
        let vk = message.wParam as u16;
        if vk == VK_PROCESSKEY || vk == VK_PACKET || self.ime.borrow().active {
            // IMM processes these keys. VK_PACKET carries Unicode input,
            // including the touch keyboard's injected UTF-16.
            return false;
        }
        let scan = (message.lParam >> 16) as u8;
        let extended = message.lParam & (1 << 24) != 0;
        let modifier = model::modifier_key(vk, scan, extended);
        let state = keyboard_state();
        let flags = model::modifiers(&state);
        let alt_graph = model::alt_graph(&state);
        crate::memory_pressure::note_activity();
        if alt_graph
            || self.menu_tracking.get()
            || model::native_menu_key(vk, flags, || self.menu_access_key(vk))
        {
            // Keep the entire key sequence in Win32, including TranslateMessage
            // and its WM_SYSCHAR. A focused sidebar or widget can otherwise
            // swallow a mnemonic even without a matching editor key binding.
            return false;
        }
        let mut ignoring = state;
        for index in [
            VK_CONTROL,
            VK_LCONTROL,
            VK_RCONTROL,
            VK_MENU,
            VK_LMENU,
            VK_RMENU,
        ] {
            ignoring[usize::from(index)] = 0;
        }
        let (characters, characters_ignoring_modifiers) = if modifier.is_some() {
            (String::new(), String::new())
        } else {
            let characters = key_characters(vk, scan, &state);
            let ignoring = key_characters(vk, scan, &ignoring);
            (characters, ignoring)
        };
        let key = KeyEvent {
            physical_code: model::physical_key(vk, scan, extended),
            characters,
            characters_ignoring_modifiers,
            modifiers: flags,
            repeat: down && message.lParam & (1 << 30) != 0,
            timestamp: f64::from(message.time) / 1000.,
            received: Instant::now(),
            state: if modifier.is_some() {
                KeyState::FlagsChanged
            } else if down {
                KeyState::Down
            } else {
                KeyState::Up
            },
        };
        let consumed = self.with_app(|app, w| app.on_key(w, key)).unwrap_or(false);
        self.flush_ime_anchor();
        if modifier.is_some() || !down {
            return false;
        }
        if consumed {
            self.request(FrameReason::Input);
            return true;
        }
        if let Some(command) = model::responder(vk, flags) {
            self.enqueue(Event::Command(command));
            self.request(FrameReason::Input);
            return true;
        }
        // With Ctrl held, an unbound printable control code is not an insert.
        flags.command() && !flags.option()
    }

    pub(super) fn input_message(
        &self,
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT> {
        match message {
            WM_CHAR | WM_SYSCHAR | WM_IME_CHAR => {
                if message == WM_SYSCHAR && !model::alt_graph(&keyboard_state()) {
                    return None;
                }
                let text = self.chars.borrow_mut().push(wparam as u16);
                if let Some(text) = text {
                    let repeats = if message == WM_IME_CHAR {
                        1
                    } else {
                        (lparam as u16).max(1)
                    };
                    self.text(TextInput::Insert {
                        text: if repeats == 1 {
                            text
                        } else {
                            text.repeat(usize::from(repeats))
                        },
                        replacement: None,
                    });
                }
            }
            WM_UNICHAR => {
                if wparam == UNICODE_NOCHAR as usize {
                    return Some(1);
                }
                if let Some(ch) = u32::try_from(wparam)
                    .ok()
                    .and_then(char::from_u32)
                    .filter(|c| !c.is_control())
                {
                    self.text(TextInput::Insert {
                        text: ch.to_string(),
                        replacement: None,
                    });
                }
            }
            WM_IME_SETCONTEXT => {
                // SAFETY: Keep the system candidate windows but suppress the
                // default preedit window. Velocity paints the marked text inline.
                return Some(unsafe {
                    DefWindowProcW(
                        hwnd,
                        message,
                        wparam,
                        lparam & !(ISC_SHOWUICOMPOSITIONWINDOW as isize),
                    )
                });
            }
            WM_IME_STARTCOMPOSITION => {
                self.ime.borrow_mut().start();
                self.ime_anchor_dirty.set(true);
                self.flush_ime_anchor();
            }
            WM_IME_COMPOSITION => {
                let flags = lparam as u32;
                if let Some(ime) = Ime::acquire(hwnd) {
                    let result = (flags & GCS_RESULTSTR != 0)
                        .then(|| ime.string(GCS_RESULTSTR))
                        .flatten();
                    let marked = if flags & (GCS_COMPSTR | GCS_CURSORPOS | GCS_COMPATTR) != 0 {
                        ime.string(GCS_COMPSTR).map(|text| (text, ime.cursor()))
                    } else {
                        None
                    };
                    let cancelled = flags
                        & (GCS_RESULTSTR
                            | GCS_COMPSTR
                            | GCS_CURSORPOS
                            | GCS_COMPATTR
                            | GCS_COMPCLAUSE
                            | GCS_COMPREADSTR
                            | GCS_COMPREADATTR
                            | GCS_COMPREADCLAUSE
                            | GCS_DELTASTART
                            | GCS_RESULTCLAUSE
                            | GCS_RESULTREADSTR
                            | GCS_RESULTREADCLAUSE)
                        == 0
                        || marked.as_ref().is_some_and(|(text, _)| text.is_empty());
                    let events = self.ime.borrow_mut().update(result, marked, cancelled);
                    for event in events {
                        self.text(event);
                    }
                }
                self.ime_anchor_dirty.set(true);
            }
            WM_IME_ENDCOMPOSITION => {
                let event = self.ime.borrow_mut().end(false);
                if let Some(event) = event {
                    self.text(event);
                }
            }
            WM_INPUTLANGCHANGE => {
                self.finish_ime(hwnd);
                return None;
            }
            WM_MOUSEACTIVATE => {
                if (lparam as u16) as u32 == HTCLIENT
                    && self
                        .app
                        .try_borrow()
                        .ok()
                        .and_then(|slot| slot.as_ref().map(|app| app.accepts_first_mouse()))
                        == Some(false)
                {
                    return Some(MA_ACTIVATEANDEAT as isize);
                }
                return None;
            }
            WM_SETCURSOR if (lparam as u16) as u32 == HTCLIENT => {
                // SAFETY: Shared cursors are queried from this message's live HWND.
                let cursor = unsafe {
                    if self.pointer_hidden.get() {
                        null_mut()
                    } else {
                        let mut point = POINT::default();
                        let shape = if GetCursorPos(&mut point) != 0
                            && ScreenToClient(hwnd, &mut point) != 0
                        {
                            self.app
                                .try_borrow()
                                .ok()
                                .and_then(|slot| {
                                    slot.as_ref().map(|app| {
                                        app.cursor_at(model::logical_point(
                                            point.x,
                                            point.y,
                                            self.dpi.get(),
                                        ))
                                    })
                                })
                                .unwrap_or_default()
                        } else {
                            CursorShape::Arrow
                        };
                        LoadCursorW(
                            null_mut(),
                            match shape {
                                CursorShape::Arrow => IDC_ARROW,
                                CursorShape::IBeam => IDC_IBEAM,
                                CursorShape::ResizeHorizontal => IDC_SIZEWE,
                                CursorShape::ResizeVertical => IDC_SIZENS,
                            },
                        )
                    }
                };
                // SAFETY: null hides the cursor; otherwise use a borrowed standard cursor.
                unsafe {
                    if GetCursor() != cursor {
                        SetCursor(cursor);
                    }
                }
                return Some(1);
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_LBUTTONUP | WM_RBUTTONDOWN
            | WM_RBUTTONDBLCLK | WM_RBUTTONUP | WM_MBUTTONDOWN | WM_MBUTTONDBLCLK
            | WM_MBUTTONUP | WM_XBUTTONDOWN | WM_XBUTTONDBLCLK | WM_XBUTTONUP | WM_MOUSEMOVE => {
                return Some(self.mouse(hwnd, message, wparam, lparam));
            }
            WM_CAPTURECHANGED | WM_CANCELMODE => {
                if self.pressed.replace(0) != 0 {
                    self.enqueue(Event::PointerCancel);
                }
                return None;
            }
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL | WM_POINTERWHEEL | WM_POINTERHWHEEL => {
                self.wheel(
                    hwnd,
                    wparam,
                    lparam,
                    matches!(message, WM_MOUSEHWHEEL | WM_POINTERHWHEEL),
                );
            }
            // Right button commands are either queued by the app at the click
            // or omitted; do not open a competing default menu.
            WM_CONTEXTMENU => {}
            _ => return None,
        }
        Some(0)
    }

    fn mouse(&self, hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let (x, y) = model::signed_pair(lparam);
        let mut screen = POINT { x, y };
        let position = model::logical_point(x, y, self.dpi.get());
        let modifiers = model::modifiers(&keyboard_state());
        // SAFETY: This reads the timestamp of the message currently being handled.
        let time = unsafe { GetMessageTime() } as u32;
        let pressed = self.pressed.get();
        let mut count = 0;
        let mut at = None;
        let extra_button = matches!(message, WM_XBUTTONDOWN | WM_XBUTTONUP | WM_XBUTTONDBLCLK);
        let button = match message {
            WM_RBUTTONDOWN | WM_RBUTTONUP | WM_RBUTTONDBLCLK => 1,
            WM_MBUTTONDOWN | WM_MBUTTONUP | WM_MBUTTONDBLCLK => 2,
            WM_XBUTTONDOWN | WM_XBUTTONUP | WM_XBUTTONDBLCLK
                if (wparam >> 16) as u16 == XBUTTON1 =>
            {
                3
            }
            WM_XBUTTONDOWN | WM_XBUTTONUP | WM_XBUTTONDBLCLK => 4,
            WM_MOUSEMOVE if pressed != 0 => pressed.trailing_zeros() as usize,
            _ => 0,
        };
        let kind = match message {
            WM_MOUSEMOVE => {
                if self.pointer_hidden.replace(false) {
                    self.input_message(hwnd, WM_SETCURSOR, hwnd as usize, HTCLIENT as isize);
                }
                if pressed == 0 {
                    MouseKind::Move
                } else {
                    MouseKind::Drag
                }
            }
            WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP | WM_XBUTTONUP => {
                self.pressed.set(pressed & !(1 << button));
                MouseKind::Up
            }
            _ => {
                self.pressed.set(pressed | 1 << button);
                // SAFETY: Convert the click to physical screen pixels and capture
                // from its live HWND so drags continue outside the client area.
                unsafe {
                    ClientToScreen(hwnd, &mut screen);
                    let dpi = self.dpi.get().max(1);
                    count = self.clicks.borrow_mut().press(
                        button,
                        time,
                        (screen.x, screen.y),
                        GetDoubleClickTime(),
                        (
                            GetSystemMetricsForDpi(SM_CXDOUBLECLK, dpi),
                            GetSystemMetricsForDpi(SM_CYDOUBLECLK, dpi),
                        ),
                    );
                    if pressed == 0 {
                        SetCapture(hwnd);
                    }
                }
                at = Some(screen);
                MouseKind::Down
            }
        };
        self.enqueue(Event::Mouse(
            MouseEvent {
                kind,
                button,
                position,
                modifiers,
                click_count: count,
                timestamp: f64::from(time) / 1000.,
            },
            at,
        ));
        if kind == MouseKind::Up && self.pressed.get() == 0 {
            // SAFETY: Release only this HWND's capture, after enqueuing the release.
            unsafe {
                if GetCapture() == hwnd {
                    ReleaseCapture();
                }
            }
        }
        self.request(FrameReason::Input);
        LRESULT::from(extra_button)
    }

    fn wheel(&self, hwnd: HWND, wparam: WPARAM, lparam: LPARAM, horizontal: bool) {
        let (x, y) = model::signed_pair(lparam);
        let mut point = POINT { x, y };
        let mut count = if horizontal { 1_u32 } else { 3_u32 };
        let setting = if horizontal {
            SPI_GETWHEELSCROLLCHARS
        } else {
            SPI_GETWHEELSCROLLLINES
        };
        // SAFETY: The screen point and system setting each have a writable output.
        unsafe {
            ScreenToClient(hwnd, &mut point);
            SystemParametersInfoW(setting, 0, (&mut count as *mut u32).cast(), 0);
        }
        let page = if horizontal {
            self.size.get().width
        } else {
            self.size.get().height
        };
        let (delta, precise) =
            model::wheel(i32::from((wparam >> 16) as i16), count, horizontal, page);
        if delta != 0. {
            crate::memory_pressure::note_activity();
            crate::memory_guard::note_input();
            self.enqueue(Event::Scroll(ScrollEvent {
                position: model::logical_point(point.x, point.y, self.dpi.get()),
                delta_x: if horizontal { delta } else { 0. },
                delta_y: if horizontal { 0. } else { delta },
                precise,
                phase: 0,
                momentum_phase: 0,
                modifiers: model::modifiers(&keyboard_state()),
                // SAFETY: Reads this thread's current message timestamp.
                timestamp: f64::from(unsafe { GetMessageTime() } as u32) / 1000.,
            }));
            self.request(FrameReason::Input);
        }
    }

    fn text(&self, input: TextInput) {
        crate::memory_pressure::note_activity();
        crate::memory_guard::note_input();
        self.enqueue(Event::Text(input));
        self.ime_anchor_dirty.set(true);
        self.request(FrameReason::Input);
    }

    pub(super) fn finish_ime(&self, hwnd: HWND) {
        self.chars.replace(model::Utf16Input::default());
        if !self.ime.borrow().active {
            return;
        }
        let inline = self
            .app
            .try_borrow()
            .ok()
            .is_some_and(|slot| slot.as_ref().is_some_and(|app| app.inline_text_input()));
        if let Some(ime) = Ime::acquire(hwnd) {
            // SAFETY: Finish IME composition without holding any borrow: the
            // system may synchronously send the result and end messages.
            unsafe {
                ImmNotifyIME(
                    ime.context,
                    NI_COMPOSITIONSTR,
                    if inline { CPS_COMPLETE } else { CPS_CANCEL },
                    0,
                )
            };
        }
        let end = self.ime.borrow_mut().end(inline);
        if let Some(end) = end {
            self.text(end);
        }
    }

    pub(super) fn flush_ime_anchor(&self) {
        if !self.ime_anchor_dirty.get() || !self.ime.borrow().active || self.closing.get() {
            return;
        }
        let Some(state) = self.app.try_borrow().ok().and_then(|slot| {
            slot.as_ref().map(|app| {
                let state = app.text_input_state();
                let caret = if state.caret.height > 0. {
                    state.caret
                } else {
                    app.text_input_rect(Utf16Range::new(state.selected.end(), 0))
                        .0
                };
                caret.clamped(self.size.get())
            })
        }) else {
            return;
        };
        self.ime_anchor_dirty.set(false);
        let Some(ime) = Ime::acquire(self.hwnd.get()) else {
            return;
        };
        let dpi = self.dpi.get();
        let left = model::pixels(state.x, dpi);
        let top = model::pixels(state.y, dpi);
        let right = model::pixels(state.x + state.width, dpi).max(left.saturating_add(1));
        let bottom = model::pixels(state.y + state.height, dpi).max(top.saturating_add(1));
        // SAFETY: Both forms use physical client pixels. IMM copies the forms
        // synchronously, and the context is still acquired from the same HWND.
        unsafe {
            ImmSetCompositionWindow(
                ime.context,
                &COMPOSITIONFORM {
                    dwStyle: CFS_POINT,
                    ptCurrentPos: POINT { x: left, y: top },
                    rcArea: RECT::default(),
                },
            );
            ImmSetCandidateWindow(
                ime.context,
                &CANDIDATEFORM {
                    dwIndex: 0,
                    dwStyle: CFS_EXCLUDE,
                    ptCurrentPos: POINT { x: left, y: bottom },
                    rcArea: RECT {
                        left,
                        top,
                        right,
                        bottom,
                    },
                },
            );
        }
    }
}

impl WindowHandle {
    /// Hides the client cursor until the next motion without changing the
    /// process-wide cursor visibility count.
    pub fn hide_pointer_until_motion(&self) {
        if self.is_focused() {
            self.0.pointer_hidden.set(true);
            // SAFETY: Hide only when the cursor is actually over our client area.
            unsafe {
                let mut point = POINT::default();
                let mut bounds = RECT::default();
                let hwnd = self.0.hwnd.get();
                if GetCursorPos(&mut point) != 0
                    && ScreenToClient(hwnd, &mut point) != 0
                    && GetClientRect(hwnd, &mut bounds) != 0
                    && point.x >= 0
                    && point.y >= 0
                    && point.x < bounds.right
                    && point.y < bounds.bottom
                {
                    SetCursor(null_mut());
                }
            }
        }
    }
}

fn keyboard_state() -> [u8; 256] {
    let mut state = [0; 256];
    // SAFETY: GetKeyboardState writes exactly 256 bytes for the GUI thread.
    unsafe { GetKeyboardState(state.as_mut_ptr()) };
    state
}

fn key_characters(vk: u16, scan: u8, state: &[u8; 256]) -> String {
    let mut units = [0_u16; 16];
    // SAFETY: This uses the calling thread's layout and accurately sized
    // buffers. Bit 2 makes this a non-mutating probe (including dead keys).
    let count = unsafe {
        ToUnicodeEx(
            u32::from(vk),
            u32::from(scan),
            state.as_ptr(),
            units.as_mut_ptr(),
            units.len() as i32,
            4,
            GetKeyboardLayout(0),
        )
    };
    if count > 0 {
        String::from_utf16_lossy(&units[..(count as usize).min(units.len())])
    } else {
        String::new()
    }
}

struct Ime {
    hwnd: HWND,
    context: HIMC,
}

impl Ime {
    fn acquire(hwnd: HWND) -> Option<Self> {
        if hwnd.is_null() {
            return None;
        }
        // SAFETY: Only a live HWND on the calling GUI thread can reach this method.
        let context = unsafe { ImmGetContext(hwnd) };
        (!context.is_null()).then(|| Self { hwnd, context })
    }

    fn string(&self, kind: IME_COMPOSITION_STRING) -> Option<String> {
        // SAFETY: Passing no bytes queries the required length, in bytes, excluding NUL.
        let bytes = unsafe { ImmGetCompositionStringW(self.context, kind, null_mut(), 0) };
        if !(0..=1024 * 1024).contains(&bytes) || bytes % 2 != 0 {
            return None;
        }
        let mut units = vec![0_u16; bytes as usize / 2];
        if bytes == 0 {
            return Some(String::new());
        }
        // SAFETY: units has exactly bytes writable bytes, and the returned count
        // is checked again before reading. No context data is borrowed past this call.
        let copied = unsafe {
            ImmGetCompositionStringW(self.context, kind, units.as_mut_ptr().cast(), bytes as u32)
        };
        (copied >= 0 && copied <= bytes && copied % 2 == 0)
            .then(|| String::from_utf16_lossy(&units[..copied as usize / 2]))
    }

    fn cursor(&self) -> usize {
        // SAFETY: GCS_CURSORPOS returns a UTF-16 code-unit offset, not buffer data.
        unsafe { ImmGetCompositionStringW(self.context, GCS_CURSORPOS, null_mut(), 0) }.max(0)
            as usize
    }
}

impl Drop for Ime {
    fn drop(&mut self) {
        // SAFETY: This guard owns the matching acquisition for this exact HWND.
        unsafe { ImmReleaseContext(self.hwnd, self.context) };
    }
}
