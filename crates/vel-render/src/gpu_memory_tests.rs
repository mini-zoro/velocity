//! GPU memory must stay flat in a steady state. These run against the real device
//! (Metal on macOS) with wgpu's internal counters, so a frame path that leaks buffers or
//! textures fails here instead of in a user's process hours later.
use super::{
    INSTANCE_SHRINK_FRAMES, MIN_INSTANCE_BYTES, Renderer,
    shared_atlas::{RASTER_BYTES, Raster},
};
use crate::{
    font::Fonts,
    scene::{Layer, Scene, SolidRect, TextRun},
};
use vel_platform::Rect;

/// Hang guard: stops a wait that would otherwise never end. It is not a latency
/// bound; the test checks the condition it waits for.
const HANG_GUARD: std::time::Duration = std::time::Duration::from_secs(120);

#[derive(Clone, Copy, Debug, PartialEq)]
struct Gpu {
    buffers: isize,
    textures: isize,
    buffer_bytes: isize,
    texture_bytes: isize,
}

fn gpu(renderer: &Renderer) -> Gpu {
    // A finished submission is only retired when the device is maintained.
    renderer.wait_idle().unwrap();
    let hal = renderer.gpu.device.get_internal_counters().hal;
    Gpu {
        buffers: hal.buffers.read(),
        textures: hal.textures.read(),
        buffer_bytes: hal.buffer_memory.read(),
        texture_bytes: hal.texture_memory.read(),
    }
}

/// A screenful of editor text; `shift` moves it like scrolling so instance data changes.
fn page(shift: usize) -> Scene {
    let clip = Rect {
        x: 0.,
        y: 0.,
        width: 800.,
        height: 600.,
    };
    let text = (0..40)
        .map(|row| TextRun {
            family: None,
            style: Default::default(),
            text: format!(
                "{:>5} let gateway = request(\"/v1/messages?beta=true\") 网关 {}",
                row + shift,
                (row + shift) % 97
            ),
            x: 8.,
            baseline: 14. + row as f32 * 15.,
            size: 12.,
            color: [0.9, 0.9, 0.9, 1.],
            highlight: None,
            clip,
        })
        .collect();
    Scene {
        layers: vec![Layer {
            text,
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn renderer() -> Option<(Renderer, Fonts)> {
    match Renderer::offscreen(800, 600) {
        Ok(renderer) => Some((renderer, Fonts::load().unwrap())),
        Err(error) => {
            eprintln!("skipped: no GPU adapter ({error})");
            None
        }
    }
}

#[test]
fn offscreen_clients_share_device_pipeline_and_release_context() {
    let first = Renderer::offscreen(48, 32).unwrap();
    let second = Renderer::offscreen(64, 48).unwrap();
    let shared = std::rc::Rc::downgrade(&first.gpu);
    assert!(std::rc::Rc::ptr_eq(&first.gpu, &second.gpu));
    assert_eq!(first.gpu.device, second.gpu.device);
    assert_eq!(first.pipeline, second.pipeline);
    assert_ne!(first.instances, second.instances);
    assert_ne!(first.screen, second.screen);
    drop(first);
    assert!(shared.upgrade().is_some());
    drop(second);
    assert!(shared.upgrade().is_none());
}

#[test]
fn device_loss_is_seen_by_all_offscreen_clients() {
    let first = Renderer::offscreen(48, 32).unwrap();
    let second = Renderer::offscreen(64, 48).unwrap();
    first.gpu.device.destroy();
    // wgpu delivers the callback on maintenance once the queue is empty.
    let _ = first.gpu.device.poll(wgpu::PollType::Poll);
    let first_error = first.gpu.check_device().unwrap_err();
    assert_eq!(second.wait_idle().unwrap_err(), first_error);
    assert!(Renderer::offscreen(48, 32).is_err());
}

#[test]
fn two_offscreen_renderers_allocate_cjk_pages_once() {
    let mut first = Renderer::offscreen(800, 600).unwrap();
    let mut second = Renderer::offscreen(800, 600).unwrap();
    let mut fonts = Fonts::load_shared().unwrap();
    let mut other_fonts = Fonts::load_shared().unwrap();
    fonts.shape("网关", 12.);
    let deadline = std::time::Instant::now() + HANG_GUARD;
    loop {
        fonts.poll_fallbacks();
        if !fonts.fallback_pending() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "CJK fallback timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(fonts.shape("网关", 12.).iter().all(|g| g.id != 0));

    let scene = page(0);
    let pixels = first.render_rgba(&scene, &mut fonts, 1.).unwrap();
    let atlas = first.atlas_info();
    assert!(atlas["allocated_array_layers"].as_u64().unwrap() > 0);
    let usage = gpu(&first);
    assert_eq!(
        second.render_rgba(&scene, &mut other_fonts, 1.).unwrap(),
        pixels
    );
    assert_eq!(second.atlas_info()["glyph_keys"], atlas["glyph_keys"]);
    assert_eq!(
        second.atlas_info()["allocated_array_layers"],
        atlas["allocated_array_layers"]
    );
    assert_eq!(gpu(&second).textures, usage.textures);

    // A different scale is a different raster key even at the same physical
    // size (6 pt @ 2x versus 12 pt @ 1x).
    let mut retina = page(0);
    for run in &mut retina.layers[0].text {
        run.size *= 0.5;
    }
    second.render_rgba(&retina, &mut other_fonts, 2.).unwrap();
    assert!(second.atlas_info()["glyph_keys"].as_u64() > atlas["glyph_keys"].as_u64());

    let generation = atlas["generation"].as_u64().unwrap();
    assert_eq!(first.prepared_generation, Some(generation));
    second.reset_fonts();
    assert!(first.atlas_info()["generation"].as_u64().unwrap() > generation);
    assert_ne!(
        first.prepared_generation,
        first.atlas_info()["generation"].as_u64()
    );
    assert_eq!(first.render_rgba(&scene, &mut fonts, 1.).unwrap(), pixels);

    let usage = gpu(&first);
    assert!(second.trim_memory(true).0 > 0);
    assert_eq!(first.memory_cache_bytes().0, 0);
    assert_eq!(first.trim_memory(true).0, 0);
    // The R8 mask page array and the (placeholder) RGBA array.
    assert_eq!(gpu(&first).textures, usage.textures - 2);
    assert_eq!(first.render_rgba(&scene, &mut fonts, 1.).unwrap(), pixels);
}

#[test]
fn small_transparent_readback_preserves_the_framebuffer_extent() {
    let Ok(mut renderer) = Renderer::offscreen(48, 32) else {
        return;
    };
    let mut fonts = Fonts::load().unwrap();
    let scene = Scene {
        background: [0.; 4],
        layers: vec![Layer {
            rects: vec![SolidRect {
                rect: Rect {
                    x: 2.,
                    y: 2.,
                    width: 6.,
                    height: 6.,
                },
                clip: Rect {
                    x: 0.,
                    y: 0.,
                    width: 12.,
                    height: 10.,
                },
                radius: 2.5,
                color: [1., 0., 0., 0.5],
            }],
            ..Default::default()
        }],
    };
    let small = renderer
        .render_rgba_at_size(&scene, &mut fonts, 2., [24, 20])
        .unwrap();
    assert_eq!(small.len(), 24 * 20 * 4);
    let corner = (4 * 24 + 4) * 4;
    assert_eq!(&small[corner..corner + 4], &[0, 0, 0, 0]);
    let center = (10 * 24 + 10) * 4;
    assert_eq!(&small[center..center + 4], &[128, 0, 0, 128]);
    assert_eq!(renderer.physical_size(), [48, 32]);
    let full = renderer.render_rgba(&scene, &mut fonts, 2.).unwrap();
    let center_full = (10 * 48 + 10) * 4;
    assert_eq!(
        &full[center_full..center_full + 4],
        &small[center..center + 4]
    );
}

#[test]
fn frames_dropped_without_a_drawable_release_their_staged_writes() {
    let Some((mut renderer, mut fonts)) = renderer() else {
        return;
    };
    for shift in 0..4 {
        renderer.render_rgba(&page(shift), &mut fonts, 2.).unwrap();
    }
    let base = gpu(&renderer);
    assert!(base.buffers > 0, "wgpu counters are enabled for tests");
    // A minimized window polls every 200 ms: each poll stages a frame and finds no
    // drawable. Before the fix every staged write stayed alive until the window was
    // shown again (megabytes per second; 2.5 GB observed).
    for shift in 0..600 {
        renderer.prepare(&page(shift), &mut fonts, 2.).unwrap();
        renderer.discard_staged_writes();
    }
    let hidden = gpu(&renderer);
    assert_eq!(
        (hidden.buffers, hidden.textures),
        (base.buffers, base.textures),
        "dropped frames keep no GPU objects: {base:?} -> {hidden:?}"
    );
    assert!(
        hidden.buffer_bytes <= base.buffer_bytes,
        "dropped frames keep no buffer memory: {base:?} -> {hidden:?}"
    );
}

#[test]
fn steady_rendering_keeps_gpu_objects_and_memory_flat() {
    let Some((mut renderer, mut fonts)) = renderer() else {
        return;
    };
    for shift in 0..4 {
        renderer.render_rgba(&page(shift), &mut fonts, 2.).unwrap();
    }
    let base = gpu(&renderer);
    for shift in 0..300 {
        renderer
            .render_rgba(&page(shift % 7), &mut fonts, 2.)
            .unwrap();
    }
    let after = gpu(&renderer);
    assert_eq!(after, base, "300 frames of the same content grow nothing");
}

#[test]
fn huge_scene_instance_bytes_retire_after_sustained_small_frames() {
    let Some((mut renderer, mut fonts)) = renderer() else {
        return;
    };
    let area = Rect {
        x: 0.,
        y: 0.,
        width: 8.,
        height: 8.,
    };
    let rect = SolidRect {
        rect: area,
        clip: area,
        radius: 0.,
        color: [1.; 4],
    };
    let small = Scene {
        layers: vec![Layer {
            rects: vec![rect.clone()],
            ..Default::default()
        }],
        ..Default::default()
    };
    renderer.prepare(&small, &mut fonts, 1.).unwrap();
    renderer.discard_staged_writes();
    let baseline = gpu(&renderer);
    let baseline_capacity = renderer.capacity;
    let huge = Scene {
        layers: vec![Layer {
            rects: vec![rect; 60_000],
            ..Default::default()
        }],
        ..Default::default()
    };
    renderer.prepare(&huge, &mut fonts, 1.).unwrap();
    renderer.discard_staged_writes();
    // Metal's HAL does not report buffer memory (the counter stays 0), so growth is read
    // from the renderer's own instance capacity; object counts still come from wgpu.
    let capacity = renderer.capacity;
    assert!(
        capacity > baseline_capacity,
        "{capacity} > {baseline_capacity}"
    );
    for _ in 0..INSTANCE_SHRINK_FRAMES - 1 {
        renderer.prepare(&small, &mut fonts, 1.).unwrap();
        renderer.discard_staged_writes();
        assert_eq!(renderer.capacity, capacity);
    }
    renderer.prepare(&small, &mut fonts, 1.).unwrap();
    renderer.discard_staged_writes();
    assert_eq!(renderer.capacity, MIN_INSTANCE_BYTES);
    assert_eq!(gpu(&renderer), baseline);
    for _ in 0..INSTANCE_SHRINK_FRAMES {
        renderer.prepare(&small, &mut fonts, 1.).unwrap();
        renderer.discard_staged_writes();
    }
    assert_eq!(gpu(&renderer), baseline);
}

#[test]
fn memory_trim_right_sizes_instances_and_rebuilds_the_atlas_on_draw() {
    let Some((mut renderer, mut fonts)) = renderer() else {
        return;
    };
    let area = Rect {
        x: 0.,
        y: 0.,
        width: 8.,
        height: 8.,
    };
    let huge = Scene {
        layers: vec![Layer {
            rects: vec![
                SolidRect {
                    rect: area,
                    clip: area,
                    radius: 0.,
                    color: [1.; 4]
                };
                60_000
            ],
            ..Default::default()
        }],
        ..Default::default()
    };
    renderer.prepare(&huge, &mut fonts, 1.).unwrap();
    renderer.discard_staged_writes();
    let peak = renderer.capacity;
    let small = page(0);
    // Use just the ASCII part of this fixture so asynchronous fallback is not a
    // variable in the pixel comparison.
    let mut small = small;
    for run in &mut small.layers[0].text {
        run.text.truncate(run.text.find(" 网关").unwrap());
    }
    let before = renderer.render_rgba(&small, &mut fonts, 1.).unwrap();
    assert!(peak > MIN_INSTANCE_BYTES);
    assert_eq!(renderer.capacity, peak);
    assert!(renderer.memory_cache_bytes().0 > 0);

    renderer.trim_memory(true);
    assert_eq!(renderer.capacity, MIN_INSTANCE_BYTES);
    assert!(renderer.data.capacity() <= MIN_INSTANCE_BYTES as usize / size_of::<super::Instance>());
    assert_eq!(renderer.memory_cache_bytes().0, 0);
    assert!(renderer.gpu.atlas.borrow().is_empty());
    assert_eq!(
        renderer.render_rgba(&small, &mut fonts, 1.).unwrap(),
        before
    );
    eprintln!("GPU instance bytes {peak} -> {}", renderer.capacity);
}

#[test]
fn cjk_scroll_and_document_close_release_cpu_glyph_caches() {
    let Some((mut renderer, mut fonts)) = renderer() else {
        return;
    };
    let files = fonts.file_storage_bytes();
    let clip = Rect {
        x: 0.,
        y: 0.,
        width: 800.,
        height: 600.,
    };
    let mut cjk = (0x3400..=0x9fff)
        .chain(0x20000..)
        .filter_map(char::from_u32)
        .take(30_000);
    for _ in 0..300 {
        let text: String = cjk.by_ref().take(100).collect();
        let scene = Scene {
            layers: vec![Layer {
                text: vec![TextRun {
                    text,
                    clip,
                    x: 0.,
                    baseline: 30.,
                    size: 9.,
                    color: [1.; 4],
                    style: Default::default(),
                    family: None,
                    highlight: None,
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        renderer.prepare(&scene, &mut fonts, 1.).unwrap();
        renderer.discard_staged_writes();
        assert!(
            renderer.gpu.atlas.borrow().rasters.capacity()
                * 2
                * (size_of::<(super::GlyphKey, Raster)>() + 1)
                <= RASTER_BYTES
        );
    }
    assert!(!renderer.gpu.atlas.borrow().rasters.is_empty());
    renderer.reset_fonts();
    fonts.clear_document_caches();
    assert_eq!(renderer.gpu.atlas.borrow().rasters.capacity(), 0);
    assert!(renderer.gpu.atlas.borrow().is_empty());
    assert_eq!(renderer.gpu.layouts.borrow().storage_bytes(), 0);
    assert_eq!(fonts.document_cache_bytes(), (0, 0));
    assert_eq!(fonts.file_storage_bytes(), files);
}
