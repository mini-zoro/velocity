//! Window geometry, monitor policy, and DWM decoration.
use super::{Event, Host, WindowHandle, model};
use crate::{
    Appearance, DisplayStatus, FrameReason, Point, Rect, Size, accessibility,
    anchored_window_frame, clamp_window_frame,
};
use std::{
    mem::size_of,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, POINT, RECT, WPARAM},
    Graphics::{
        Dwm::{
            DWMWA_CAPTION_COLOR, DWMWA_CLOAKED, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
            DwmGetWindowAttribute, DwmSetWindowAttribute,
        },
        Gdi::*,
    },
    System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
    UI::{
        Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW},
        HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow},
        Input::KeyboardAndMouse::SetFocus,
        WindowsAndMessaging::*,
    },
};
use windows_sys::w;

pub(super) fn caret_occluded(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: The caller owns a live HWND. DWMWA_CLOAKED writes a DWORD, and the
    // cached DC is released on this thread after querying its visible region.
    unsafe {
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED as u32,
            (&mut cloaked as *mut u32).cast(),
            size_of::<u32>() as u32,
        ) >= 0
            && cloaked != 0
        {
            return true;
        }
        let dc = GetDCEx(hwnd, null_mut(), DCX_CACHE | DCX_CLIPSIBLINGS);
        if dc.is_null() {
            return false;
        }
        let clipped = GetClipBox(dc, &mut RECT::default());
        ReleaseDC(hwnd, dc);
        clipped == NULLREGION
    }
}

#[derive(Clone, Copy)]
pub(super) struct Fullscreen {
    style: WINDOW_STYLE,
    placement: WINDOWPLACEMENT,
}

pub(super) fn outer_rect(size: Size, dpi: u32, style: WINDOW_STYLE, has_menu: bool) -> RECT {
    let max_extent = i32::MAX / 2;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: model::pixels(size.width, dpi).clamp(1, max_extent),
        bottom: model::pixels(size.height, dpi).clamp(1, max_extent),
    };
    // SAFETY: rect is an initialized writable rectangle, using the style of our window.
    unsafe { AdjustWindowRectExForDpi(&mut rect, style, i32::from(has_menu), 0, dpi.max(1)) };
    rect
}

pub(super) fn system_appearance() -> Appearance {
    let mut light = 1_u32;
    let mut bytes = size_of::<u32>() as u32;
    // SAFETY: Both names are static UTF-16 strings and the DWORD fits the buffer.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&mut light as *mut u32).cast(),
            &mut bytes,
        )
    };
    if status == 0 && light == 0 {
        Appearance::Dark
    } else {
        Appearance::Light
    }
}

pub(super) fn accessibility_preferences() -> accessibility::Preferences {
    let mut animations = 1_i32;
    let mut contrast = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    // SAFETY: The corresponding SPI actions write a BOOL and HIGHCONTRASTW.
    unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            (&mut animations as *mut i32).cast(),
            0,
        );
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            contrast.cbSize,
            (&mut contrast as *mut HIGHCONTRASTW).cast(),
            0,
        );
    }
    accessibility::Preferences {
        reduce_motion: animations == 0,
        increase_contrast: contrast.dwFlags & HCF_HIGHCONTRASTON != 0,
    }
}

impl Host {
    pub(super) fn initialize_geometry(&self, requested: Size) {
        let hwnd = self.hwnd.get();
        // SAFETY: The caller has just created hwnd, which is live but hidden.
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        let bounds = outer_rect(requested, dpi, WS_OVERLAPPEDWINDOW, false);
        // SAFETY: Only the size of the live hidden HWND is changed.
        unsafe {
            SetWindowPos(
                hwnd,
                null_mut(),
                0,
                0,
                bounds.right - bounds.left,
                bounds.bottom - bounds.top,
                SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOZORDER,
            );
        }
        self.sync_geometry(true);
    }

    pub(super) fn sync_geometry(&self, moved: bool) {
        let hwnd = self.hwnd.get();
        if hwnd.is_null() || self.closing.get() || self.minimized.get() {
            return;
        }
        let mut frame = RECT::default();
        let mut client = RECT::default();
        // SAFETY: The window is live and both RECTs are writable for these calls.
        let (dpi, maximized, monitor) = unsafe {
            if GetClientRect(hwnd, &mut client) == 0 || GetWindowRect(hwnd, &mut frame) == 0 {
                return;
            }
            (
                GetDpiForWindow(hwnd),
                IsZoomed(hwnd) != 0,
                MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
            )
        };
        let size = model::logical_size(client.right, client.bottom, dpi);
        let resized = self.size.replace(size) != size;
        let scaled = self.dpi.replace(dpi) != dpi;
        if self.monitor.replace(monitor) != monitor {
            self.refresh.set(refresh_hz(monitor));
        }
        if self.fullscreen.get().is_none() && !maximized {
            self.normal_frame.set(Some(physical_rect(frame)));
        }
        if moved || resized || scaled {
            self.enqueue(Event::Resize);
            self.request(FrameReason::Resize);
            self.ime_anchor_dirty.set(true);
        }
    }

    pub(super) fn dpi_changed(&self, wparam: WPARAM, lparam: LPARAM) {
        if lparam == 0 || self.closing.get() {
            return;
        }
        self.dpi.set(wparam as u16 as u32);
        // SAFETY: WM_DPICHANGED passes a readable RECT valid until the callback returns.
        let suggested = unsafe { *(lparam as *const RECT) };
        let bounds = if self.fullscreen.get().is_some() {
            // SAFETY: The suggested rectangle is a valid screen rectangle.
            let monitor = unsafe { MonitorFromRect(&suggested, MONITOR_DEFAULTTONEAREST) };
            monitor_info(monitor).map_or(suggested, |info| info.monitorInfo.rcMonitor)
        } else {
            suggested
        };
        self.set_bounds(bounds, 0);
        self.sync_geometry(true);
    }

    pub(super) fn minimum_track_size(&self, hwnd: HWND, lparam: LPARAM) {
        let size = self.minimum_size.get();
        if lparam == 0 || self.fullscreen.get().is_some() || size == Size::default() {
            return;
        }
        // SAFETY: WM_GETMINMAXINFO supplies a writable MINMAXINFO and a live HWND.
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as WINDOW_STYLE;
            let bounds = outer_rect(size, self.dpi.get(), style, !GetMenu(hwnd).is_null());
            let info = &mut *(lparam as *mut MINMAXINFO);
            info.ptMinTrackSize.x = info.ptMinTrackSize.x.max(bounds.right - bounds.left);
            info.ptMinTrackSize.y = info.ptMinTrackSize.y.max(bounds.bottom - bounds.top);
        }
    }

    pub(super) fn appearance_changed(&self) {
        let appearance = system_appearance();
        if self.appearance.replace(appearance) != appearance {
            let (theme, bg) = self.title_theme.get();
            self.apply_title_theme(theme, bg);
            self.enqueue(Event::Appearance);
            self.request(FrameReason::Resize);
        }
    }

    pub(super) fn apply_title_theme(
        &self,
        appearance: Option<Appearance>,
        background: Option<[f32; 4]>,
    ) {
        let hwnd = self.hwnd.get();
        if hwnd.is_null() || self.closing.get() {
            return;
        }
        let dark = i32::from(appearance.unwrap_or(self.appearance.get()) == Appearance::Dark);
        let (caption, text) = if let Some(bg) = background.filter(|c| c[3] >= 1.) {
            let [r, g, b] = [bg[0], bg[1], bg[2]].map(|n| (n.clamp(0., 1.) * 255.).round() as u32);
            let luminance = 0.2126 * bg[0] + 0.7152 * bg[1] + 0.0722 * bg[2];
            (
                r | (g << 8) | (b << 16),
                if luminance > 0.55 { 0 } else { 0x00ff_ffff },
            )
        } else {
            (u32::MAX, u32::MAX) // DWMWA_COLOR_DEFAULT
        };
        // SAFETY: Each selected DWM attribute takes one 32-bit value. Windows
        // versions without caption/text color support can ignore those calls.
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
                (&dark as *const i32).cast(),
                4,
            );
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_CAPTION_COLOR as u32,
                (&caption as *const u32).cast(),
                4,
            );
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_TEXT_COLOR as u32,
                (&text as *const u32).cast(),
                4,
            );
        }
    }

    fn set_bounds(&self, bounds: RECT, flags: SET_WINDOW_POS_FLAGS) {
        let hwnd = self.hwnd.get();
        if hwnd.is_null() {
            return;
        }
        // SAFETY: Both the HWND and physical screen bounds belong to this thread's window.
        unsafe {
            SetWindowPos(
                hwnd,
                null_mut(),
                bounds.left,
                bounds.top,
                bounds.right.saturating_sub(bounds.left),
                bounds.bottom.saturating_sub(bounds.top),
                SWP_NOZORDER | SWP_NOACTIVATE | flags,
            );
        }
    }
}

impl WindowHandle {
    /// Current effective Windows app color mode.
    pub fn appearance(&self) -> Appearance {
        self.0.appearance.get()
    }

    /// Sets the title bar mode; supported Windows builds also apply the caption color.
    pub fn set_title_bar_theme(
        &self,
        appearance: Option<Appearance>,
        background: Option<[f32; 4]>,
    ) {
        let theme = (appearance, background);
        if self.0.title_theme.replace(theme) != theme {
            self.0.apply_title_theme(appearance, background);
        }
    }

    /// App-drawn overlay scrollbars remain the default.
    pub fn prefers_overlay_scrollers(&self) -> bool {
        true
    }

    /// Reports known visibility limits without claiming to observe DWM occlusion.
    pub fn display_status(&self) -> DisplayStatus {
        DisplayStatus {
            window_occluded: !self.0.is_drawable(),
            ..crate::display_status()
        }
    }

    /// Saves the restored outer frame in physical virtual-screen coordinates.
    pub fn frame(&self) -> Option<Rect> {
        self.0.normal_frame.get()
    }

    /// Places a client point at the desired screen location, within the monitor work area.
    pub fn position_content_point(&self, local: Point, desired: Point) {
        let (Some(hwnd), Some(current)) = (self.hwnd(), self.screen_point(local)) else {
            return;
        };
        let mut frame = RECT::default();
        // SAFETY: Read the current frame of this GUI thread's live HWND.
        if unsafe { GetWindowRect(hwnd.as_ptr(), &mut frame) } == 0 {
            return;
        }
        let point = POINT {
            x: desired.x.round() as i32,
            y: desired.y.round() as i32,
        };
        // SAFETY: A read-only monitor query in physical desktop coordinates.
        let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
        let area = monitor_info(monitor).map(|info| info.monitorInfo.rcWork);
        let to_rect = |r: RECT| Rect {
            x: f64::from(r.left),
            y: f64::from(r.top),
            width: f64::from(r.right - r.left),
            height: f64::from(r.bottom - r.top),
        };
        let clamped = anchored_window_frame(to_rect(frame), current, desired, area.map(to_rect));
        self.0.set_bounds(
            RECT {
                left: clamped.x.round() as i32,
                top: clamped.y.round() as i32,
                right: (clamped.x + clamped.width).round() as i32,
                bottom: (clamped.y + clamped.height).round() as i32,
            },
            0,
        );
    }

    /// Restores the requested outer frame onto a current monitor's work area.
    pub fn restore_frame(&self, frame: Rect) {
        let Some(clamped) = clamp_window_frame(frame, &monitor_work_areas()) else {
            return;
        };
        self.0.set_bounds(
            RECT {
                left: clamped.x.round() as i32,
                top: clamped.y.round() as i32,
                right: (clamped.x + clamped.width).round() as i32,
                bottom: (clamped.y + clamped.height).round() as i32,
            },
            0,
        );
    }

    /// Sets the minimum logical client area.
    pub fn set_minimum_content_size(&self, size: Size) {
        self.0.minimum_size.set(size);
    }

    /// Shows the window and requests foreground focus (subject to Windows focus policy).
    pub fn focus(&self) {
        if let Some(hwnd) = self.hwnd() {
            // SAFETY: The HWND is live and owned by the calling thread.
            unsafe {
                let hwnd = hwnd.as_ptr();
                if IsIconic(hwnd) != 0 {
                    ShowWindow(hwnd, SW_RESTORE);
                }
                if SetForegroundWindow(hwnd) != 0 {
                    SetFocus(hwnd);
                }
            }
        }
    }

    /// Minimizes this top-level window.
    pub fn minimize(&self) {
        if let Some(hwnd) = self.hwnd() {
            // SAFETY: A live HWND owned by this thread.
            unsafe { ShowWindow(hwnd.as_ptr(), SW_MINIMIZE) };
        }
    }

    /// Toggles maximize/restore without changing borderless fullscreen.
    pub fn zoom(&self) {
        if let Some(hwnd) = self.hwnd()
            && !self.is_full_screen()
        {
            // SAFETY: A live HWND owned by this thread.
            unsafe {
                ShowWindow(
                    hwnd.as_ptr(),
                    if IsZoomed(hwnd.as_ptr()) != 0 {
                        SW_RESTORE
                    } else {
                        SW_MAXIMIZE
                    },
                )
            };
        }
    }

    /// Whether this window is borderless and occupies its monitor.
    pub fn is_full_screen(&self) -> bool {
        self.0.fullscreen.get().is_some()
    }

    /// Toggles borderless fullscreen on the window's current monitor.
    pub fn toggle_full_screen(&self) {
        self.toggle_full_screen_with_mode(false);
    }

    /// Both variants use the same native borderless implementation on Windows.
    pub fn toggle_full_screen_with_mode(&self, _simple: bool) {
        let Some(hwnd) = self.hwnd() else { return };
        let hwnd = hwnd.as_ptr();
        if let Some(old) = self.0.fullscreen.get() {
            // SAFETY: Restore the saved style and placement for this same HWND.
            unsafe {
                let visible = (GetWindowLongPtrW(hwnd, GWL_STYLE) as u32) & WS_VISIBLE;
                SetWindowLongPtrW(
                    hwnd,
                    GWL_STYLE,
                    ((old.style & !WS_VISIBLE) | visible) as isize,
                );
                if let Some(menu) = self.0.menu.borrow().as_ref() {
                    SetMenu(hwnd, menu.handle());
                }
                SetWindowPlacement(hwnd, &old.placement);
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOZORDER | SWP_FRAMECHANGED,
                );
            }
            self.0.fullscreen.set(None);
        } else {
            let mut placement = WINDOWPLACEMENT {
                length: size_of::<WINDOWPLACEMENT>() as u32,
                ..Default::default()
            };
            // SAFETY: These queries address a live window, with writable placement.
            let (style, monitor) = unsafe {
                if GetWindowPlacement(hwnd, &mut placement) == 0 {
                    return;
                }
                (
                    GetWindowLongPtrW(hwnd, GWL_STYLE) as WINDOW_STYLE,
                    MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                )
            };
            let Some(info) = monitor_info(monitor) else {
                return;
            };
            self.0.fullscreen.set(Some(Fullscreen { style, placement }));
            // SAFETY: Menu stays owned by the host while detached. Preserve
            // hidden/visible state during startup and avoid changing z-order.
            unsafe {
                if style & WS_MAXIMIZE != 0 {
                    ShowWindow(hwnd, SW_RESTORE);
                }
                SetMenu(hwnd, null_mut());
                SetWindowLongPtrW(
                    hwnd,
                    GWL_STYLE,
                    (WS_POPUP | (style & (WS_VISIBLE | WS_CLIPCHILDREN))) as isize,
                );
            }
            self.0
                .set_bounds(info.monitorInfo.rcMonitor, SWP_FRAMECHANGED);
        }
        self.0.sync_geometry(true);
    }
}

fn monitor_info(monitor: HMONITOR) -> Option<MONITORINFOEXW> {
    if monitor.is_null() {
        return None;
    }
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: MONITORINFOEXW starts with MONITORINFO and cbSize requests the extended data.
    (unsafe { GetMonitorInfoW(monitor, &mut info.monitorInfo) } != 0).then_some(info)
}

fn refresh_hz(monitor: HMONITOR) -> u32 {
    let Some(info) = monitor_info(monitor) else {
        return 60;
    };
    let mut mode = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    // SAFETY: szDevice is NUL-terminated; mode is initialized with the proper size.
    if unsafe { EnumDisplaySettingsW(info.szDevice.as_ptr(), ENUM_CURRENT_SETTINGS, &mut mode) }
        != 0
        && (10..=1000).contains(&mode.dmDisplayFrequency)
    {
        mode.dmDisplayFrequency
    } else {
        60
    }
}

fn physical_rect(rect: RECT) -> Rect {
    Rect {
        x: f64::from(rect.left),
        y: f64::from(rect.top),
        width: f64::from(rect.right) - f64::from(rect.left),
        height: f64::from(rect.bottom) - f64::from(rect.top),
    }
}

fn monitor_work_areas() -> Vec<Rect> {
    let mut screens: Vec<Rect> = Vec::new();
    // SAFETY: EnumDisplayMonitors calls back synchronously; screens remains writable.
    unsafe {
        EnumDisplayMonitors(
            null_mut(),
            null(),
            Some(add_monitor),
            (&mut screens as *mut Vec<Rect>) as LPARAM,
        );
    }
    screens
}

unsafe extern "system" fn add_monitor(
    monitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    data: LPARAM,
) -> i32 {
    if let Some(info) = monitor_info(monitor) {
        // SAFETY: This LPARAM was supplied by monitor_work_areas for this call.
        let screens = unsafe { &mut *(data as *mut Vec<Rect>) };
        let bounds = physical_rect(info.monitorInfo.rcWork);
        if info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0 {
            screens.insert(0, bounds);
        } else {
            screens.push(bounds);
        }
    }
    1
}
