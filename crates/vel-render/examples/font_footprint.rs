//! Windowless font memory probe; run ASCII and CJK in fresh processes.
#[cfg(target_os = "macos")]
fn main() -> Result<(), String> {
    use std::collections::HashSet;
    use std::time::{Duration, Instant};
    use vel_platform::metrics::{phys_footprint_bytes, rss_bytes};
    use vel_render::font::Fonts;

    let mut args = std::env::args().skip(1);
    let usage = "usage: font_footprint ascii|cjk [FONT_FILE POSTSCRIPT_NAME]";
    let sample = match args.next().as_deref() {
        Some("ascii") => "Rain cleared and the bookshop turned on its lights.",
        Some("cjk") => "小雨停了，街角的书店亮起了灯。路过的人推开门，在窗前读完这一页。",
        _ => return Err(usage.into()),
    };
    let before = phys_footprint_bytes()?;
    let mut fonts = match (args.next(), args.next(), args.next()) {
        (None, None, None) => Fonts::load()?,
        (Some(path), Some(name), None) => Fonts::load_file(path.as_ref(), &name)?,
        _ => return Err(usage.into()),
    };
    let glyphs = fonts.shape(sample, 26.);
    let deadline = Instant::now() + Duration::from_secs(10);
    while fonts.fallback_pending() {
        if Instant::now() >= deadline {
            return Err("font fallback did not settle".into());
        }
        std::thread::sleep(Duration::from_millis(1));
        fonts.poll_fallbacks();
    }
    let glyphs = if glyphs.iter().any(|g| g.id == 0) {
        fonts.shape(sample, 26.)
    } else {
        glyphs
    };
    let mut seen = HashSet::new();
    for g in glyphs {
        if g.id == 0 {
            return Err("sample contains .notdef after fallback".into());
        }
        // A space has no image. Rasterize every applicable unique glyph like
        // the renderer's atlas, without creating a device, surface or window.
        if seen.insert((g.face, g.id)) {
            std::hint::black_box(fonts.rasterize(g.face, g.id, 26., 0));
        }
    }
    println!(
        "{}",
        serde_json::json!({
            "phys_footprint_before_bytes": before,
            "phys_footprint_bytes": phys_footprint_bytes()?,
            "rss_bytes": rss_bytes()?,
            "unique_glyphs": seen.len(),
            "fonts": fonts.metadata(),
        })
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("font_footprint requires macOS");
}
