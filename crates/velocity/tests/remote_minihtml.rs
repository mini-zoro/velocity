//! Real anonymous HTTPS and the production codec helper, without an AppKit window.
#![cfg(target_os = "macos")]
#[path = "../src/codec_process.rs"]
mod codec_process;
#[path = "../src/remote_image.rs"]
mod remote_image;
#[path = "../../vel-packages/tests/support/tls.rs"]
mod tls;

use image::ImageEncoder as _;
use std::{
    path::Path,
    time::{Duration, Instant},
};
use vel_minihtml::LayoutOptions;
use vel_platform::Rect;
use vel_render::scene::{Scene, Surface};
use vel_ui::minihtml::{Html, Metrics, enable_remote_images};

/// Hang guard: stops a wait that would otherwise never end. It is not a latency
/// bound; the test checks the condition it waits for.
const HANG_GUARD: Duration = Duration::from_secs(120);

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut data = Vec::new();
    let pixels = [26u8, 51, 77, 255].repeat((width * height) as usize);
    image::codecs::png::PngEncoder::new(&mut data)
        .write_image(&pixels, width, height, image::ExtendedColorType::Rgba8)
        .unwrap();
    data
}

fn scene(html: &mut Html, metrics: &Metrics) -> Scene {
    html.layout(&LayoutOptions::default(), 0, metrics);
    let mut scene = Scene::default();
    html.paint(
        &mut scene,
        Rect {
            x: 0.,
            y: 0.,
            width: 400.,
            height: 200.,
        },
    );
    scene
}

fn placeholder(scene: &Scene) -> bool {
    scene
        .layers
        .iter()
        .flat_map(|layer| &layer.rects)
        .any(|r| r.color == [0.5, 0.5, 0.5, 0.2])
}

fn finish_requests(metrics: &Metrics) {
    let deadline = Instant::now() + HANG_GUARD;
    while metrics.images_pending() {
        metrics.poll_images();
        assert!(Instant::now() < deadline, "image worker did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn r35_loopback_https_image_decodes_paints_and_reuses_cached_pixels() {
    let server = tls::Server::start(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../vel-packages/tests/https-server.py"),
    );
    server.response("pixel", "200 OK", "Content-Type: image/png\r\n", &png(5, 3));
    server.response("large-side", "200 OK", "", &png(2049, 1));
    server.response("too-many-pixels", "200 OK", "", &png(2048, 1025));
    server.response("broken", "200 OK", "", b"not an image");
    let client = vel_packages::https::HttpsClient {
        timeout: Duration::from_secs(30),
        ..server.client()
    };
    enable_remote_images(move |url| {
        remote_image::load(
            url,
            &client,
            Path::new(env!("CARGO_BIN_EXE_velocity")),
            Duration::from_secs(30),
        )
    });
    let metrics = Metrics::default();
    let document = |name| Html::new(&format!("<img src='{}'>", server.url(name)), false).unwrap();
    let mut html = document("pixel");
    assert!(placeholder(&scene(&mut html, &metrics)));
    finish_requests(&metrics);
    for html in [&mut html, &mut document("pixel")] {
        let painted = scene(html, &metrics);
        assert!(!placeholder(&painted));
        assert!(painted.layers.iter().flat_map(|layer| &layer.surfaces).any(
            |s| matches!(s, Surface::Image(q) if q.rect.width == 5. && q.rect.height == 3.
                && q.image.rgba[..4] == [26, 51, 77, 255])
        ));
    }
    for name in ["large-side", "too-many-pixels", "broken", "not-found"] {
        let mut html = document(name);
        assert!(placeholder(&scene(&mut html, &metrics)));
        finish_requests(&metrics);
        assert!(placeholder(&scene(&mut html, &metrics)));
        assert!(placeholder(&scene(&mut document(name), &metrics)));
        assert!(!metrics.images_pending(), "a failed URL was fetched again");
    }
    let requests = std::fs::read_to_string(server.root.path().join("requests.log")).unwrap();
    assert_eq!(
        requests.lines().collect::<Vec<_>>(),
        [
            "pixel",
            "large-side",
            "too-many-pixels",
            "broken",
            "not-found"
        ]
    );
    // The same helper refuses an oversized image before writing a pixel reply.
    assert!(remote_image::helper(png(2049, 1).as_slice(), &mut Vec::new()).is_none());
}
