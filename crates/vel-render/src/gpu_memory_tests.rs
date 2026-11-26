//! GPU memory must stay flat in a steady state. These run against the real device
//! (Metal on macOS) with wgpu's internal counters, so a frame path that leaks buffers or
//! textures fails here instead of in a user's process hours later.
use super::{INSTANCE_SHRINK_FRAMES, MIN_INSTANCE_BYTES, RASTER_BYTES, Renderer};
use crate::{
    font::Fonts,
    scene::{Layer, Scene, SolidRect, TextRun},
};
use vel_platform::Rect;

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
    let hal = renderer.device.get_internal_counters().hal;
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
    assert!(renderer.atlas.is_empty());
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
            renderer.rasters.capacity() * 2 * (size_of::<(super::GlyphKey, super::Raster)>() + 1)
                <= RASTER_BYTES
        );
    }
    assert!(!renderer.rasters.is_empty());
    renderer.reset_fonts();
    fonts.clear_document_caches();
    assert_eq!(renderer.rasters.capacity(), 0);
    assert!(renderer.atlas.is_empty());
    assert_eq!(renderer.layouts.storage_bytes(), 0);
    assert_eq!(fonts.document_cache_bytes(), (0, 0));
    assert_eq!(fonts.file_storage_bytes(), files);
}
