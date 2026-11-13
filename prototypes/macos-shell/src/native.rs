//! Narrow macOS observations; these functions never connect to the window server.
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Read the kernel's process birth time, including loader work before Rust main.
pub fn process_birth() -> Result<SystemTime, String> {
    #[cfg(target_os = "macos")]
    {
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
        let size = std::mem::size_of::<libc::proc_bsdinfo>();
        // SAFETY: the kernel writes at most `size` bytes into this valid output allocation.
        let written = unsafe {
            libc::proc_pidinfo(
                std::process::id() as i32,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size as i32,
            )
        };
        if written != size as i32 {
            return Err(format!(
                "proc_pidinfo returned {written}: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: the successful call above initialized the entire proc_bsdinfo structure.
        let info = unsafe { info.assume_init() };
        Ok(UNIX_EPOCH
            + Duration::from_secs(info.pbi_start_tvsec)
            + Duration::from_micros(info.pbi_start_tvusec))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("process birth measurement requires macOS".into())
    }
}
/// Current resident size, not the high-water RSS returned by getrusage.
pub fn rss_bytes() -> Result<u64, String> {
    #[cfg(target_os = "macos")]
    {
        let mut info = std::mem::MaybeUninit::<libc::mach_task_basic_info>::uninit();
        let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
        // SAFETY: task_info receives a valid task port and a correctly sized writable buffer/count.
        // libc still exposes the native ABI; avoid a second Mach binding in this small prototype.
        #[allow(deprecated)]
        let result = unsafe {
            libc::task_info(
                libc::mach_task_self(),
                libc::MACH_TASK_BASIC_INFO,
                info.as_mut_ptr().cast(),
                &mut count,
            )
        };
        if result != libc::KERN_SUCCESS || count != libc::MACH_TASK_BASIC_INFO_COUNT {
            return Err(format!("task_info code={result} count={count}"));
        }
        // SAFETY: KERN_SUCCESS and the checked count confirm the structure is initialized.
        Ok(unsafe { info.assume_init() }.resident_size)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("RSS measurement requires macOS task_info".into())
    }
}

// Stable TASK_VM_INFO revision 1 prefix from the macOS SDK's mach/task_info.h.
// libc exposes task_info but not this flavor's payload. Request only the prefix
// through phys_footprint so newer trailing fields and kernel revisions are irrelevant.
#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Default)]
struct TaskVmInfoRev1 {
    virtual_size: u64,
    region_count: i32,
    page_size: i32,
    resident_size: u64,
    resident_size_peak: u64,
    device: u64,
    device_peak: u64,
    internal: u64,
    internal_peak: u64,
    external: u64,
    external_peak: u64,
    reusable: u64,
    reusable_peak: u64,
    purgeable_volatile_pmap: u64,
    purgeable_volatile_resident: u64,
    purgeable_volatile_virtual: u64,
    compressed: u64,
    compressed_peak: u64,
    compressed_lifetime: u64,
    phys_footprint: u64,
}

#[cfg(target_os = "macos")]
const TASK_VM_INFO: libc::task_flavor_t = 22;
#[cfg(target_os = "macos")]
const TASK_VM_INFO_REV1_COUNT: libc::mach_msg_type_number_t =
    (std::mem::size_of::<TaskVmInfoRev1>() / std::mem::size_of::<libc::natural_t>()) as u32;

/// Current task memory charge, using the footprint accounting shown by Activity Monitor.
/// This differs from RSS and includes charges such as compressed private memory.
pub fn phys_footprint_bytes() -> Result<u64, String> {
    #[cfg(target_os = "macos")]
    {
        let mut info = TaskVmInfoRev1::default();
        let mut count = TASK_VM_INFO_REV1_COUNT;
        // SAFETY: the repr(C) revision-1 prefix matches the SDK ABI. count is in
        // natural_t words and describes exactly this initialized, writable buffer.
        #[allow(deprecated)]
        let result = unsafe {
            libc::task_info(
                libc::mach_task_self(),
                TASK_VM_INFO,
                (&mut info as *mut TaskVmInfoRev1).cast(),
                &mut count,
            )
        };
        checked_footprint(result, count, &info)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("physical footprint measurement requires macOS task_info".into())
    }
}

#[cfg(target_os = "macos")]
fn checked_footprint(
    result: libc::kern_return_t,
    count: libc::mach_msg_type_number_t,
    info: &TaskVmInfoRev1,
) -> Result<u64, String> {
    if result != libc::KERN_SUCCESS || count != TASK_VM_INFO_REV1_COUNT {
        return Err(format!(
            "task_info(TASK_VM_INFO) code={result} count={count}, expected revision-1 count={TASK_VM_INFO_REV1_COUNT}"
        ));
    }
    Ok(info.phys_footprint)
}

#[cfg(target_os = "macos")]
pub fn font_location(name: &str) -> Result<(std::path::PathBuf, String, NativeFont), String> {
    use objc2_core_foundation::{CFString, CFURL};
    use objc2_core_text::{CTFont, kCTFontURLAttribute};
    // SAFETY: the string lives through the call and a null matrix requests the identity transform.
    let font = unsafe { CTFont::with_name(&CFString::from_str(name), 13., std::ptr::null()) };
    // SAFETY: both methods only read a valid retained CTFont; the exported key is immutable.
    let (attribute, postscript) = unsafe {
        (
            font.attribute(kCTFontURLAttribute),
            font.post_script_name().to_string(),
        )
    };
    let url = attribute
        .and_then(|a| a.downcast::<CFURL>().ok())
        .ok_or_else(|| format!("CoreText has no font URL for {name}"))?;
    let path = url
        .to_file_path()
        .ok_or_else(|| format!("invalid font file URL for {name}"))?;
    Ok((path, postscript, font))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kernel_readings_are_plausible_without_gpu() {
        #[cfg(target_os = "macos")]
        {
            let birth = process_birth().unwrap();
            let age = SystemTime::now().duration_since(birth).unwrap();
            assert!(age < Duration::from_secs(3600));
            assert!(rss_bytes().unwrap() > 0);
            assert!(phys_footprint_bytes().unwrap() > 0);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn vm_info_prefix_matches_sdk_and_rejects_short_or_failed_replies() {
        assert_eq!(TASK_VM_INFO, 22);
        assert_eq!(std::mem::offset_of!(TaskVmInfoRev1, phys_footprint), 144);
        assert_eq!(std::mem::size_of::<TaskVmInfoRev1>(), 152);
        assert_eq!(TASK_VM_INFO_REV1_COUNT, 38);
        let info = TaskVmInfoRev1 {
            resident_size: 123,
            phys_footprint: 9_000_000_000,
            ..Default::default()
        };
        assert_eq!(
            checked_footprint(libc::KERN_SUCCESS, 38, &info),
            Ok(9_000_000_000)
        );
        assert!(checked_footprint(libc::KERN_SUCCESS, 36, &info).is_err());
        assert!(checked_footprint(libc::KERN_FAILURE, 38, &info).is_err());
    }
}

/// Rasterize an OS font glyph when swash cannot read that font's outline/bitmap format.
/// The result uses the same top-down placement and straight-alpha contract as swash.
#[cfg(target_os = "macos")]
pub fn raster_glyph(
    original: &objc2_core_text::CTFont,
    glyph: u16,
    px: f32,
    bucket: u8,
) -> Result<swash::scale::image::Image, String> {
    use objc2_core_foundation::CGPoint;
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGColorSpace, CGContext, CGImageAlphaInfo, CGImageByteOrderInfo,
    };
    use objc2_core_text::CTFontOrientation;
    use std::ptr::NonNull;
    use swash::{
        scale::{
            Source,
            image::{Content, Image},
        },
        zeno::Placement,
    };
    // SAFETY: retain the actual resolved font (including private UI faces); a null matrix
    // preserves its transform. Recreating a private PostScript name can silently substitute Times.
    let font = unsafe { original.copy_with_attributes(px as f64, std::ptr::null(), None) };
    // SAFETY: this only reads the retained CTFont's immutable PostScript name.
    let name = unsafe { font.post_script_name().to_string() };
    let mut glyph = glyph;
    // SAFETY: this pointer addresses exactly one initialized glyph and bounds output is optional.
    let bounds = unsafe {
        font.bounding_rects_for_glyphs(
            CTFontOrientation::Horizontal,
            NonNull::from(&mut glyph),
            std::ptr::null_mut(),
            1,
        )
    };
    if !bounds.origin.x.is_finite()
        || !bounds.origin.y.is_finite()
        || !bounds.size.width.is_finite()
        || !bounds.size.height.is_finite()
    {
        return Err("CoreText returned non-finite glyph bounds".into());
    }
    let phase = bucket as f64 / 4.;
    let left = (bounds.origin.x + phase).floor() as i32 - 1;
    let bottom = bounds.origin.y.floor() as i32 - 1;
    let right = (bounds.origin.x + bounds.size.width + phase).ceil() as i32 + 1;
    let top = (bounds.origin.y + bounds.size.height).ceil() as i32 + 1;
    let width = (right - left).max(1) as usize;
    let height = (top - bottom).max(1) as usize;
    if width > 4096 || height > 4096 {
        return Err("CoreText glyph exceeds atlas limit".into());
    }
    let mut data = vec![0u8; width * height * 4];
    let color_space = CGColorSpace::new_device_rgb().ok_or("RGB color space unavailable")?;
    // SAFETY: the Vec allocation has width*height*4 bytes, remains live and unaliased while
    // CGContext draws, and is released only after the context is explicitly dropped below.
    let context = unsafe {
        CGBitmapContextCreate(
            data.as_mut_ptr().cast(),
            width,
            height,
            8,
            width * 4,
            Some(&color_space),
            CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0,
        )
    }
    .ok_or("CGBitmapContextCreate failed")?;
    CGContext::set_rgb_fill_color(Some(&context), 1., 1., 1., 1.);
    CGContext::set_should_antialias(Some(&context), true);
    CGContext::set_should_smooth_fonts(Some(&context), false);
    let mut position = CGPoint::new(-left as f64 + phase, -bottom as f64);
    // SAFETY: glyph and position are initialized one-element arrays, with a live bitmap context.
    unsafe {
        font.draw_glyphs(
            NonNull::from(&mut glyph),
            NonNull::from(&mut position),
            1,
            &context,
        )
    };
    drop(context);
    // Quartz exposes bitmap storage in top-down row order even though glyph user
    // coordinates use a bottom-left origin. The F-orientation regression checks this.
    let is_color = name.contains("Emoji");
    let data = if is_color {
        unpremultiply(&mut data);
        data
    } else {
        data.as_chunks::<4>().0.iter().map(|p| p[3]).collect()
    };
    if !data.iter().any(|b| *b != 0) {
        return Err(format!(
            "CoreText produced empty pixels for {name} glyph {glyph}"
        ));
    }
    Ok(Image {
        source: Source::Outline,
        content: if is_color {
            Content::Color
        } else {
            Content::Mask
        },
        placement: Placement {
            left,
            top,
            width: width as u32,
            height: height as u32,
        },
        data,
    })
}

fn unpremultiply(data: &mut [u8]) {
    for pixel in data.as_chunks_mut::<4>().0 {
        let a = pixel[3] as u32;
        for value in &mut pixel[..3] {
            *value = (*value as u32 * 255 + a / 2)
                .checked_div(a)
                .unwrap_or(0)
                .min(255) as u8;
        }
    }
}

/// Retained native face. Preserve it instead of resolving private UI font names again.
#[cfg(target_os = "macos")]
pub type NativeFont = objc2_core_foundation::CFRetained<objc2_core_text::CTFont>;

#[cfg(test)]
mod raster_tests {
    use super::*;
    #[test]
    fn rgba_unpremultiplication_preserves_alpha_and_handles_transparency() {
        let mut pixels = [64, 32, 0, 128, 0, 0, 0, 0, 255, 128, 1, 255];
        unpremultiply(&mut pixels);
        assert_eq!(pixels, [128, 64, 0, 128, 0, 0, 0, 0, 255, 128, 1, 255]);
    }
    #[test]
    fn coretext_bitmap_is_top_down_with_correct_glyph_placement() {
        let mut fonts = crate::font::Fonts::load().unwrap();
        let glyph = fonts.shape("F", 40.)[0];
        let (_, _, font) = font_location("Menlo-Regular").unwrap();
        let image = raster_glyph(&font, glyph.id, 40., 0).unwrap();
        let width = image.placement.width as usize;
        let height = image.placement.height as usize;
        let third = (height / 3).max(1);
        let upper: u64 = image.data[..third * width].iter().map(|v| *v as u64).sum();
        let lower: u64 = image.data[(height - third) * width..]
            .iter()
            .map(|v| *v as u64)
            .sum();
        assert!(
            upper > lower,
            "letter F must have its long horizontal bar at the top: upper={upper}, lower={lower}"
        );
        assert!(image.placement.top > 0);
    }
}
