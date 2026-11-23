#![cfg(windows)]

use std::{
    io,
    ptr::null_mut,
    time::{Duration, Instant},
};
use vel_render::{Renderer, font::Fonts, scene::Scene};
use windows_sys::{
    Win32::{Foundation::HWND, UI::WindowsAndMessaging::*},
    w,
};

struct Window(HWND);

impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: This HWND is owned by this test thread; the renderer was dropped first.
        unsafe { DestroyWindow(self.0) };
    }
}

#[test]
fn real_hwnd_prepares_and_presents_a_dx12_frame() {
    const SIZE: i32 = 64;
    // SAFETY: STATIC is an inbox class. No window procedure references test data.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("STATIC"),
            w!("Velocity DX12 test"),
            WS_POPUP,
            0,
            0,
            SIZE,
            SIZE,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
        )
    };
    assert!(
        !hwnd.is_null(),
        "CreateWindowExW: {}",
        io::Error::last_os_error()
    );
    let window = Window(hwnd);
    let cache = tempfile::tempdir().unwrap();
    let mut fonts = Fonts::load().unwrap();
    // SAFETY: window outlives the renderer on the creating test thread.
    let mut renderer = unsafe {
        Renderer::from_hwnd(
            window.0,
            SIZE as u32,
            SIZE as u32,
            false,
            cache.path(),
            |_| {},
            |adapter| {
                eprintln!("Windows frame test adapter: {adapter:?}");
                Ok(())
            },
        )
    }
    .expect("create and configure the real HWND DX12 surface (including a Windows display handle)");
    renderer
        .pre_render(&Scene::default(), &mut fonts, 1.)
        .unwrap();
    // SAFETY: Keep the tiny tool window out of the taskbar and do not take focus.
    unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if renderer
            .present_prepared()
            .expect("present via the real DXGI swap chain")
            .is_some()
        {
            break;
        }
        assert!(Instant::now() < deadline, "no successful DX12 present");
        std::thread::sleep(Duration::from_millis(10));
    }
}
