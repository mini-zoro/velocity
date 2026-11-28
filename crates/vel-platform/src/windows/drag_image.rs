//! The reusable Win32 drag popup; only the initial image upload touches GDI pixels.
use crate::{DragImage, Point};
use std::{
    cell::RefCell,
    mem::size_of,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{HWND, POINT, RECT, SIZE},
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::*,
};
use windows_sys::w;

struct Popup {
    hwnd: HWND,
    grab: Point,
    loaded: bool,
    visible: bool,
}

thread_local! {
    static POPUP: RefCell<Option<Popup>> = const { RefCell::new(None) };
}

impl Popup {
    fn new() -> Option<Self> {
        // SAFETY: The predefined STATIC class needs no custom procedure. The popup
        // is owned by this GUI thread, not by any editor HWND that can close mid-drag.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE
                    | WS_EX_TOPMOST,
                w!("STATIC"),
                null(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            )
        };
        (!hwnd.is_null()).then_some(Self {
            hwnd,
            grab: Point::default(),
            loaded: false,
            visible: false,
        })
    }

    fn follow(&mut self, screen: Point) {
        let at = self.origin(screen);
        // SAFETY: The HWND is live on this thread; no other window is resized or activated.
        unsafe {
            SetWindowPos(
                self.hwnd,
                null_mut(),
                at.x,
                at.y,
                0,
                0,
                SWP_NOSIZE
                    | SWP_NOZORDER
                    | SWP_NOACTIVATE
                    | if self.visible { 0 } else { SWP_SHOWWINDOW },
            );
        }
        self.visible = true;
    }

    fn origin(&self, screen: Point) -> POINT {
        POINT {
            x: (screen.x - self.grab.x).round() as i32,
            y: (screen.y - self.grab.y).round() as i32,
        }
    }
}

impl Drop for Popup {
    fn drop(&mut self) {
        // SAFETY: The TLS owner drops its own popup on the GUI thread.
        unsafe { DestroyWindow(self.hwnd) };
    }
}

/// Uploads the image into the hidden popup; the next `follow` moves and shows it,
/// so the first visible frame does no pixel work.
pub(super) fn load(image: &DragImage, grab: Point) {
    let Some(size) = i32::try_from(image.width)
        .ok()
        .zip(i32::try_from(image.height).ok())
        .map(|(cx, cy)| SIZE { cx, cy })
    else {
        return;
    };
    if size.cx == 0
        || size.cy == 0
        || (image.width as usize)
            .checked_mul(image.height as usize)
            .and_then(|n| n.checked_mul(4))
            != Some(image.rgba.len())
        || !image.scale.is_finite()
        || image.scale <= 0.
    {
        return;
    }
    POPUP.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Popup::new();
        }
        let Some(popup) = slot.as_mut() else { return };
        if popup.visible {
            // SAFETY: This GUI thread owns the HWND.
            unsafe { ShowWindow(popup.hwnd, SW_HIDE) };
            popup.visible = false;
        }
        popup.grab = Point {
            x: grab.x * image.scale,
            y: grab.y * image.scale,
        };
        // A hidden layered window keeps the uploaded bitmap until it is shown.
        popup.loaded = upload(popup.hwnd, size, POINT::default(), &image.rgba);
    });
}

fn upload(hwnd: HWND, size: SIZE, origin: POINT, rgba: &[u8]) -> bool {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size.cx,
            biHeight: -size.cy,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        },
        ..Default::default()
    };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    // SAFETY: The top-down DIB has four bytes per pixel with no row padding.
    // The source slice was checked against that size. GDI owns the writable bits;
    // the selected bitmap is restored before the handles are deleted.
    unsafe {
        let dc = CreateCompatibleDC(null_mut());
        if dc.is_null() {
            return false;
        }
        let mut bits = null_mut();
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        let mut updated = false;
        if !bitmap.is_null() {
            if !bits.is_null() {
                let bgra = std::slice::from_raw_parts_mut(bits.cast::<u8>(), rgba.len());
                for (dst, src) in bgra
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(rgba.as_chunks::<4>().0)
                {
                    *dst = [src[2], src[1], src[0], src[3]];
                }
                let previous = SelectObject(dc, bitmap);
                if !previous.is_null() {
                    updated = UpdateLayeredWindow(
                        hwnd,
                        null_mut(),
                        &origin,
                        &size,
                        dc,
                        &POINT::default(),
                        0,
                        &blend,
                        ULW_ALPHA,
                    ) != 0;
                    SelectObject(dc, previous);
                }
            }
            DeleteObject(bitmap);
        }
        DeleteDC(dc);
        updated
    }
}

pub(super) fn follow(screen: Point) {
    POPUP.with(|slot| {
        if let Some(popup) = slot.borrow_mut().as_mut().filter(|popup| popup.loaded) {
            popup.follow(screen);
        }
    });
}

pub(super) fn hide(clear: bool) {
    POPUP.with(|slot| {
        if let Some(popup) = slot.borrow_mut().as_mut() {
            if popup.visible {
                // SAFETY: This GUI thread owns the HWND.
                unsafe { ShowWindow(popup.hwnd, SW_HIDE) };
                popup.visible = false;
            }
            if clear && popup.loaded {
                // Windows retains the last UpdateLayeredWindow bitmap. Replace it
                // with a transparent pixel so only the reusable HWND stays cached.
                upload(popup.hwnd, SIZE { cx: 1, cy: 1 }, POINT::default(), &[0; 4]);
                popup.loaded = false;
            }
        }
    });
}

pub(super) fn window_at(screen: Point) -> Option<usize> {
    let point = POINT {
        x: screen.x.round() as i32,
        y: screen.y.round() as i32,
    };
    // SAFETY: These calls only query z-order. The layered, transparent popup is
    // normally bypassed by WindowFromPoint; skip it explicitly if necessary.
    let mut hit = unsafe { GetAncestor(WindowFromPoint(point), GA_ROOT) };
    POPUP.with(|slot| {
        if let Some(popup) = slot.borrow().as_ref()
            && hit == popup.hwnd
        {
            // SAFETY: Start at our live popup; only read z-order and window geometry.
            unsafe {
                hit = GetWindow(hit, GW_HWNDNEXT);
                let mut rect = RECT::default();
                while !hit.is_null() {
                    if IsWindowVisible(hit) != 0
                        && IsIconic(hit) == 0
                        && GetWindowRect(hit, &mut rect) != 0
                        && PtInRect(&rect, point) != 0
                    {
                        break;
                    }
                    hit = GetWindow(hit, GW_HWNDNEXT);
                }
            }
        }
    });
    (!hit.is_null()).then_some(hit as usize)
}
