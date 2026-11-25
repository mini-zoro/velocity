//! GPU memory must stay flat in a steady state. These run against the real device
//! (Metal on macOS) with wgpu's internal counters, so a frame path that leaks buffers or
//! textures fails here instead of in a user's process hours later.
use super::Renderer;
use crate::{
    font::Fonts,
    scene::{Layer, Scene, TextRun},
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
