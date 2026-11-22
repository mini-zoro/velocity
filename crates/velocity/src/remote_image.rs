//! Untrusted minihtml raster images use the anonymous HTTPS client and a killable codec child.
use std::io::Read;
#[cfg(any(target_os = "macos", windows))]
use std::path::Path;

pub(crate) const REMOTE_HELPER: &str = "--internal-decode-remote-image";
const REMOTE_BYTES: usize = 4 * 1024 * 1024;
const REMOTE_SIDE: u32 = 2048;
const REMOTE_PIXELS: u32 = 2 * 1024 * 1024;

fn valid_remote_size(w: u32, h: u32) -> bool {
    (1..=REMOTE_SIDE).contains(&w) && (1..=REMOTE_SIDE).contains(&h) && w * h <= REMOTE_PIXELS
}

pub(crate) fn helper(input: impl Read, output: impl std::io::Write) -> Option<()> {
    use image::ImageDecoder as _;
    let mut bytes = Vec::new();
    input
        .take((REMOTE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > REMOTE_BYTES {
        return None;
    }
    let format = image::guess_format(&bytes).ok()?;
    if !matches!(
        format,
        image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::Gif
    ) {
        return None;
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(REMOTE_SIDE);
    limits.max_image_height = Some(REMOTE_SIDE);
    limits.max_alloc = Some(32 * 1024 * 1024);
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    reader.limits(limits.clone());
    let mut decoder = reader.into_decoder().ok()?;
    let (w, h) = decoder.dimensions();
    if !valid_remote_size(w, h) || decoder.total_bytes() > 16 * 1024 * 1024 {
        return None;
    }
    decoder.set_limits(limits).ok()?;
    let image = image::DynamicImage::from_decoder(decoder)
        .ok()?
        .into_rgba8();
    let mut output = output;
    for dimension in [w, h] {
        output.write_all(&dimension.to_le_bytes()).ok()?;
    }
    output.write_all(image.as_raw()).ok()
}

#[cfg(any(target_os = "macos", windows))]
pub(crate) fn load(
    url: &str,
    client: &vel_packages::https::HttpsClient,
    executable: &Path,
    decode_budget: std::time::Duration,
) -> Option<std::sync::Arc<vel_render::scene::Image>> {
    use std::{
        hash::{DefaultHasher, Hash, Hasher},
        io::{Seek, Write},
        process::{Command, Stdio},
    };
    let download = client.fetch_anonymous(url, REMOTE_BYTES as u64).ok()?;
    let mut hash = DefaultHasher::new();
    download.hash(&mut hash);
    // An unlinked input file lets both sides use ordinary blocking reads/writes without
    // a pipe-feeding thread or a persistent download. Only stdout needs a watchdog.
    let mut input = tempfile::tempfile().ok()?;
    input.write_all(&download).ok()?;
    input.rewind().ok()?;
    let mut command = Command::new(executable);
    command.arg(REMOTE_HELPER).stdin(Stdio::from(input));
    let bytes = crate::codec_process::bounded_reply(
        &mut command,
        &std::sync::atomic::AtomicBool::new(false),
        decode_budget,
        8 + (REMOTE_PIXELS * 4) as usize,
    )?;
    let header = bytes.first_chunk::<8>()?;
    let n = |i: usize| u32::from_le_bytes(header[i..i + 4].try_into().expect("dimension"));
    let (w, h) = (n(0), n(4));
    if !valid_remote_size(w, h) || bytes.len() != 8 + (w * h * 4) as usize {
        return None;
    }
    Some(std::sync::Arc::new(vel_render::scene::Image {
        logical_size: [w as f32, h as f32],
        id: hash.finish(),
        width: w,
        height: h,
        rgba: bytes[8..].into(),
    }))
}
