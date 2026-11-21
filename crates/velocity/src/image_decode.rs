//! Bounded preview decoding. The macOS host can kill a stuck codec without
//! risking the editor or starving the single image worker.
use std::{
    io::Read,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

const MAX_SIDE: u32 = 8192;
const MAX_PIXELS: u32 = 16 * 1024 * 1024;
const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_TEXTURE: u32 = 2048;

pub(crate) const HELPER: &str = "--internal-decode-image";
pub(crate) const WEBP_FIXTURE: &[u8] = include_bytes!("../tests/fixtures/images/preview.webp");

pub(crate) fn known_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ["png", "jpg", "jpeg", "gif", "webp", "tga"]
                .iter()
                .any(|kind| ext.eq_ignore_ascii_case(kind))
        })
}

pub(crate) struct Pixels {
    source: (u32, u32),
    image: image::RgbaImage,
}

fn valid_size(w: u32, h: u32) -> bool {
    (1..=MAX_SIDE).contains(&w) && (1..=MAX_SIDE).contains(&h) && w * h <= MAX_PIXELS
}

fn load(path: &Path) -> Option<Pixels> {
    let file = vel_fileio::regular::read(path).ok()?;
    if file.metadata().ok()?.len() > MAX_FILE {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_FILE {
        return None;
    }
    decode(&bytes)
}

fn decode(bytes: &[u8]) -> Option<Pixels> {
    let decoded = match image::guess_format(bytes).ok() {
        Some(image::ImageFormat::WebP) => {
            let (w, h) = webp_dimensions(bytes)?;
            #[cfg(target_os = "macos")]
            {
                let rgba = vel_platform::image_io::decode_webp(bytes, w, h)?;
                image::DynamicImage::ImageRgba8(image::RgbaImage::from_raw(w, h, rgba)?)
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (w, h);
                return None;
            }
        }
        Some(
            format @ (image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::Gif),
        ) => read_rust_format(bytes, format)?,
        None if tga_header(bytes) => read_rust_format(bytes, image::ImageFormat::Tga)?,
        _ => return None,
    };
    let source = (decoded.width(), decoded.height());
    if !valid_size(source.0, source.1) {
        return None;
    }
    let image = if source.0.max(source.1) > MAX_TEXTURE {
        decoded.thumbnail(MAX_TEXTURE, MAX_TEXTURE)
    } else {
        decoded
    };
    Some(Pixels {
        source,
        image: image.into_rgba8(),
    })
}

fn read_rust_format(bytes: &[u8], format: image::ImageFormat) -> Option<image::DynamicImage> {
    use image::ImageDecoder as _;
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().ok()?;
    let (w, h) = decoder.dimensions();
    if !valid_size(w, h) || decoder.total_bytes() > 128 * 1024 * 1024 {
        return None;
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(128 * 1024 * 1024);
    decoder.set_limits(limits).ok()?;
    image::DynamicImage::from_decoder(decoder).ok()
}

// TGA has no required magic. Check the fixed header before asking the codec,
// including dimensions and the allowed image/color-map combinations.
fn tga_header(bytes: &[u8]) -> bool {
    let Some(header) = bytes.first_chunk::<18>() else {
        return false;
    };
    let (w, h) = (
        u32::from(u16::from_le_bytes([header[12], header[13]])),
        u32::from(u16::from_le_bytes([header[14], header[15]])),
    );
    if !valid_size(w, h) || header[17] & 0xc0 != 0 {
        return false;
    }
    let entries = usize::from(u16::from_le_bytes([header[5], header[6]]));
    let (map, depth_ok) = match (header[1], header[2]) {
        (1, 1 | 9) if entries > 0 && matches!(header[7], 15 | 16 | 24 | 32) => (
            entries * usize::from(header[7]).div_ceil(8),
            matches!(header[16], 8 | 16),
        ),
        (0, 2 | 10) => (0, matches!(header[16], 16 | 24 | 32)),
        (0, 3 | 11) => (0, matches!(header[16], 8 | 16)),
        _ => return false,
    };
    let offset = 18 + usize::from(header[0]) + map;
    let pixel = usize::from(header[16]).div_ceil(8);
    let minimum = if header[2] >= 9 {
        1 + pixel
    } else {
        (w * h) as usize * pixel
    };
    depth_ok && bytes.len() >= offset + minimum
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    // RIFF canvas and frame fields: https://developers.google.com/speed/webp/docs/riff_container
    let size = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
    if &bytes[..4] != b"RIFF" || bytes.get(8..12)? != b"WEBP" || size != bytes.len() - 8 {
        return None;
    }
    let first = bytes.get(12..16)?;
    let first_size = u32::from_le_bytes(bytes.get(16..20)?.try_into().ok()?) as usize;
    let data = bytes.get(20..20 + first_size)?;
    if first != b"VP8X" {
        return (bytes.len() == 20 + first_size.next_multiple_of(2))
            .then(|| webp_frame(first, data))
            .flatten();
    }
    if data.len() != 10 {
        return None;
    }
    let canvas = (webp_u24(&data[4..]) + 1, webp_u24(&data[7..]) + 1);
    if !valid_size(canvas.0, canvas.1) {
        return None;
    }
    let mut frame = false;
    let mut pixels = 0u32;
    for (kind, payload) in WebpChunks(&bytes[30..]) {
        match kind? {
            b"VP8 " | b"VP8L" if !frame && data[0] & 2 == 0 => {
                if webp_frame(kind?, payload)? != canvas {
                    return None;
                }
                frame = true;
            }
            b"ANMF" if data[0] & 2 != 0 => {
                let header = payload.get(..16)?;
                let (x, y) = (2 * webp_u24(header), 2 * webp_u24(&header[3..]));
                let (w, h) = (1 + webp_u24(&header[6..]), 1 + webp_u24(&header[9..]));
                if !valid_size(w, h) || x + w > canvas.0 || y + h > canvas.1 {
                    return None;
                }
                pixels = pixels.checked_add(w * h)?;
                if pixels > MAX_PIXELS {
                    return None;
                }
                let mut inner_frame = false;
                for (inner, data) in WebpChunks(&payload[16..]) {
                    match inner? {
                        b"ALPH" if !inner_frame => {}
                        b"VP8 " | b"VP8L"
                            if !inner_frame && webp_frame(inner?, data)? == (w, h) =>
                        {
                            inner_frame = true;
                        }
                        _ => return None,
                    }
                }
                if !inner_frame {
                    return None;
                }
                frame = true;
            }
            b"VP8X" | b"VP8 " | b"VP8L" | b"ANMF" => return None,
            _ => {}
        }
    }
    frame.then_some(canvas)
}

fn webp_u24(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0])
}

fn webp_frame(kind: &[u8], data: &[u8]) -> Option<(u32, u32)> {
    let (w, h) = match kind {
        b"VP8 " if data.first()? & 1 == 0 && data.get(3..6)? == [0x9d, 0x01, 0x2a] => (
            u32::from(u16::from_le_bytes(data.get(6..8)?.try_into().ok()?) & 0x3fff),
            u32::from(u16::from_le_bytes(data.get(8..10)?.try_into().ok()?) & 0x3fff),
        ),
        b"VP8L" if data.first()? == &0x2f => {
            let dims = u32::from_le_bytes(data.get(1..5)?.try_into().ok()?);
            (1 + (dims & 0x3fff), 1 + ((dims >> 14) & 0x3fff))
        }
        _ => return None,
    };
    valid_size(w, h).then_some((w, h))
}

struct WebpChunks<'a>(&'a [u8]);
impl<'a> Iterator for WebpChunks<'a> {
    type Item = (Option<&'a [u8]>, &'a [u8]);
    fn next(&mut self) -> Option<Self::Item> {
        if self.0.is_empty() {
            return None;
        }
        let input = std::mem::take(&mut self.0);
        let Some(header) = input.first_chunk::<8>() else {
            return Some((None, &[]));
        };
        let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let end = 8 + len.next_multiple_of(2);
        let Some(rest) = input.get(end..) else {
            return Some((None, &[]));
        };
        self.0 = rest;
        Some((Some(&header[..4]), &input[8..8 + len]))
    }
}

/// Helper process entry: four little-endian dimensions followed by packed RGBA8.
pub(crate) fn helper(path: &Path, mut output: impl std::io::Write) -> Option<()> {
    let Pixels {
        source: (w, h),
        image,
    } = load(path)?;
    for n in [w, h, image.width(), image.height()] {
        output.write_all(&n.to_le_bytes()).ok()?;
    }
    output.write_all(image.as_raw()).ok()
}

#[cfg(any(test, not(unix)))]
pub(crate) fn preview(path: &Path, cancelled: &AtomicBool) -> Option<Pixels> {
    let at = std::time::Instant::now();
    let pixels = load(path)?;
    (!cancelled.load(Ordering::Acquire) && at.elapsed() <= std::time::Duration::from_secs(2))
        .then_some(pixels)
}

#[cfg(all(unix, not(test)))]
pub(crate) fn preview(path: &Path, cancelled: &AtomicBool) -> Option<Pixels> {
    let exe = std::env::current_exe().ok()?;
    let mut command = std::process::Command::new(exe);
    command.arg(HELPER).arg(path);
    let bytes = bounded_reply(&mut command, cancelled, std::time::Duration::from_secs(2))?;
    let header = bytes.first_chunk::<16>()?;
    let n = |i: usize| u32::from_le_bytes(header[i..i + 4].try_into().expect("dimension"));
    let (w, h, tw, th) = (n(0), n(4), n(8), n(12));
    if !valid_size(w, h)
        || !(1..=w.min(MAX_TEXTURE)).contains(&tw)
        || !(1..=h.min(MAX_TEXTURE)).contains(&th)
        || bytes.len() != 16 + (tw * th * 4) as usize
    {
        return None;
    }
    Some(Pixels {
        source: (w, h),
        image: image::RgbaImage::from_raw(tw, th, bytes[16..].to_vec())?,
    })
}

#[cfg(unix)]
fn bounded_reply(
    command: &mut std::process::Command,
    cancelled: &AtomicBool,
    budget: std::time::Duration,
) -> Option<Vec<u8>> {
    use std::{io::ErrorKind, os::fd::AsRawFd, process::Stdio, time::Instant};
    let deadline = Instant::now() + budget;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut pipe = child.stdout.take().expect("preview stdout");
    let fd = pipe.as_raw_fd();
    let mut read = || {
        // SAFETY: pipe owns fd; only its status flags are read and changed.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        // SAFETY: fd is still owned by the same pipe, which is only used here.
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return None;
        }
        let mut data = Vec::new();
        let mut chunk = [0u8; 16 * 1024];
        let mut eof = false;
        while !cancelled.load(Ordering::Acquire) && Instant::now() < deadline {
            if eof {
                if let Some(status) = child.try_wait().ok()? {
                    return status.success().then_some(data);
                }
            } else {
                match pipe.read(&mut chunk) {
                    Ok(0) => eof = true,
                    Ok(n) if data.len() + n <= 16 + (MAX_TEXTURE * MAX_TEXTURE * 4) as usize => {
                        data.extend_from_slice(&chunk[..n]);
                        continue;
                    }
                    Err(e)
                        if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                    _ => return None,
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        None
    };
    let result = read();
    if result.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

impl Pixels {
    pub(crate) fn into_scene(self, id: u64) -> vel_render::scene::Image {
        vel_render::scene::Image {
            logical_size: [self.source.0 as f32, self.source.1 as f32],
            id,
            width: self.image.width(),
            height: self.image.height(),
            rgba: self.image.into_raw().into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Two top-origin pixels, red then blue, stored in BGR order.
    const TGA: &[u8] = &[
        0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 1, 0, 24, 32, 0, 0, 255, 255, 0, 0,
    ];

    #[test]
    fn r33_tga_preview_and_bounded_bad_headers() {
        let valid = decode(TGA).expect("TGA");
        assert_eq!(valid.source, (2, 1));
        assert_eq!(valid.image.as_raw(), &[255, 0, 0, 255, 0, 0, 255, 255]);
        let mut rle = [TGA[..18].to_vec(), vec![0x81, 0, 255, 0]].concat();
        rle[2] = 10;
        assert_eq!(
            decode(&rle).unwrap().image.as_raw(),
            &[0, 255, 0, 255, 0, 255, 0, 255]
        );
        let mut oversized = TGA.to_vec();
        oversized[12..16].copy_from_slice(&[0, 0x20, 1, 0x20]);
        assert!(!tga_header(&oversized));
        oversized[12..16].copy_from_slice(&[0, 0x20, 0, 0x20]); // each side fits, area does not
        assert!(!tga_header(&oversized));
        assert!(decode(&oversized).is_none());
        let mut malicious = TGA.to_vec();
        malicious[1..8].copy_from_slice(&[1, 1, 0xff, 0xff, 0xff, 0xff, 32]);
        assert!(decode(&malicious).is_none());
        for len in 0..TGA.len() {
            assert!(decode(&TGA[..len]).is_none(), "truncated TGA at {len}");
        }
    }

    #[test]
    fn r33_webp_preview_and_bounded_bad_headers() {
        assert_eq!(webp_dimensions(WEBP_FIXTURE), Some((240, 144)));
        #[cfg(target_os = "macos")]
        {
            let valid = decode(WEBP_FIXTURE).expect("WebP");
            assert_eq!(valid.source, (240, 144));
            assert_eq!(valid.image.get_pixel(0, 0).0[3], 0);
            let top = valid.image.get_pixel(183, 48).0;
            assert!(top[0] > 230 && top[1] > 160 && top[3] == 255, "{top:?}");
        }
        for len in 0..WEBP_FIXTURE.len() {
            assert!(webp_dimensions(&WEBP_FIXTURE[..len]).is_none());
        }
        let mut oversized = WEBP_FIXTURE.to_vec();
        let dims = u32::from_le_bytes(oversized[21..25].try_into().unwrap());
        oversized[21..25].copy_from_slice(&((dims & !0x0fff_ffff) | 0x0fff_ffff).to_le_bytes());
        assert!(webp_dimensions(&oversized).is_none());
        oversized[16..20].fill(0xff);
        assert!(decode(&oversized).is_none(), "fake RIFF chunk length");
        assert!(decode(b"8BPS\0\x01\0\0\0\0\0\0\0\x03\0\0\0\x01\0\0\0\x01\0\x08\0\x03").is_none());
    }

    #[test]
    fn r33_fuzz_preview_headers_and_truncated_packets() {
        let mut rng = 0x072b_0349u32;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            rng as usize
        };
        for (seed, first) in [(TGA, 0), (WEBP_FIXTURE, 12)] {
            for _ in 0..640 {
                let mut sample = seed.to_vec();
                for _ in 0..=(next() % 3) {
                    let i = first + next() % (sample.len() - first);
                    sample[i] = next() as u8;
                }
                if next().is_multiple_of(4) {
                    sample.truncate(next() % sample.len());
                }
                if let Some(decoded) = decode(&sample) {
                    assert!(valid_size(decoded.source.0, decoded.source.1));
                    assert!(decoded.image.width().max(decoded.image.height()) <= MAX_TEXTURE);
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn r33_preview_worker_kills_a_stalled_child() {
        use std::{
            process::Command,
            time::{Duration, Instant},
        };
        let at = Instant::now();
        assert!(
            bounded_reply(
                Command::new("/bin/sleep").arg("5"),
                &AtomicBool::new(false),
                Duration::from_millis(40),
            )
            .is_none()
        );
        assert!(at.elapsed() < Duration::from_secs(1));
        let flag = AtomicBool::new(true);
        assert!(
            bounded_reply(
                Command::new("/bin/sleep").arg("5"),
                &flag,
                Duration::from_secs(5)
            )
            .is_none()
        );
    }
}
