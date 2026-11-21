//! ImageIO bridge used by the isolated image preview helper (no AppKit or GPU).
use objc2_core_foundation::{CFData, CFRetained, CFType, CGRect};
use objc2_core_graphics::{CGColorSpace, CGContext, CGImage};
use std::ptr::NonNull;

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageSourceCreateWithData(
        data: &CFData,
        options: *const CFType,
    ) -> Option<NonNull<CFType>>;
    fn CGImageSourceCreateImageAtIndex(
        source: &CFType,
        index: usize,
        options: *const CFType,
    ) -> Option<NonNull<CGImage>>;
    fn CGImageSourceGetStatusAtIndex(source: &CFType, index: usize) -> i32;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGBitmapContextCreate(
        data: *mut u8,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        color_space: &CGColorSpace,
        bitmap_info: u32,
    ) -> Option<NonNull<CGContext>>;
}

/// Decode frame zero into straight-alpha RGBA. The caller preflights the container
/// and runs this in a process with a deadline; ImageIO's decode is not interruptible.
pub fn decode_webp(bytes: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let (w, h) = (width as usize, height as usize);
    let stride = w.checked_mul(4)?;
    let len = stride.checked_mul(h)?;
    if w == 0 || h == 0 || w > 8192 || h > 8192 || len > 64 * 1024 * 1024 {
        return None;
    }
    let data = CFData::from_bytes(bytes);
    // SAFETY: the source takes a CFData reference and a null options dictionary.
    // Returned objects use the Create rule and are retained until drawing completes.
    let source =
        unsafe { CFRetained::from_raw(CGImageSourceCreateWithData(&data, std::ptr::null())?) };
    // SAFETY: source is a live CGImageSource, index zero is checked by ImageIO.
    let image = unsafe {
        CFRetained::from_raw(CGImageSourceCreateImageAtIndex(
            &source,
            0,
            std::ptr::null(),
        )?)
    };
    // SAFETY: the same live image source is used; status zero means complete.
    let status = unsafe { CGImageSourceGetStatusAtIndex(&source, 0) };
    if status != 0 || CGImage::width(Some(&image)) != w || CGImage::height(Some(&image)) != h {
        return None;
    }
    let mut rgba = vec![0; len];
    let space = CGColorSpace::new_device_rgb()?;
    // kCGBitmapByteOrder32Big | kCGImageAlphaPremultipliedLast yields RGBA bytes.
    // SAFETY: rgba has exactly stride * h writable bytes and does not move or get
    // accessed while the retained bitmap context borrows its backing storage.
    let context = unsafe {
        CFRetained::from_raw(CGBitmapContextCreate(
            rgba.as_mut_ptr(),
            w,
            h,
            8,
            stride,
            &space,
            0x4001,
        )?)
    };
    CGContext::draw_image(
        Some(&context),
        CGRect::new(
            objc2_core_foundation::CGPoint::new(0., 0.),
            objc2_core_foundation::CGSize::new(w as f64, h as f64),
        ),
        Some(&image),
    );
    drop(context);
    for p in rgba.as_chunks_mut::<4>().0 {
        let a = u16::from(p[3]);
        if a != 0 && a != 255 {
            for c in &mut p[..3] {
                *c = ((u16::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    Some(rgba)
}
