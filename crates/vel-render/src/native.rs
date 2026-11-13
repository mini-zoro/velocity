//! CoreText discovery and bitmap fallback, adapted from the MIT/Apache P0 prototype.
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
