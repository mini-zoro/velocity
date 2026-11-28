//! Common-source CPU probe, also compiled against main for paired review. No GUI.
use crate::{editor::Editor, language::Assets, stats::summarize, view::View};
use std::{
    hint::black_box,
    sync::Arc,
    time::{Duration, Instant},
};
use vel_buffer::{Region, Selection};
use vel_config::{json, settings::Layer};
use vel_platform::Size;
use vel_text::Text;
fn ready(e: &mut Editor, v: &mut View) {
    let until = Instant::now() + crate::test_timing::HANG_GUARD;
    loop {
        e.poll_language();
        black_box(v.scene(e));
        if v.exact(e)
            && e.language
                .borrow()
                .as_ref()
                .is_some_and(|l| l.ready() && l.highlight_complete())
        {
            return;
        }
        assert!(Instant::now() < until);
        std::thread::park_timeout(Duration::from_micros(250));
    }
}
#[test]
#[ignore = "paired CPU probe; run explicitly without concurrent compilation"]
fn hot_scenes() {
    let source = "fn f() {\n    let value = value;\n}\n".repeat(1000);
    let mut e = Editor::new(Text::from(source.as_str()));
    e.attach_language(
        Assets::bundled().unwrap(),
        "Rust",
        Arc::new(vel_color_scheme::ColorScheme::velocity_dark()),
    )
    .unwrap();
    e.settings.set(Layer::View, "word_wrap", json!(false));
    e.sync();
    let mut v = View::new(
        Size {
            width: 1280.,
            height: 800.,
        },
        8.,
        12.,
    );
    ready(&mut e, &mut v);
    if std::env::var_os("R16_PERF_IDLE").is_some() {
        let at = Instant::now();
        let mut publications = 0;
        for _ in 0..2000 {
            publications += usize::from(e.poll_language());
            std::thread::park_timeout(Duration::from_millis(1));
        }
        println!(
            "R16_PERF {}",
            json!({"idle_ms":at.elapsed().as_secs_f64()*1000.,"publications":publications})
        );
        return;
    }
    let mut result = serde_json::Map::new();
    for scenario in [
        "plain_scroll",
        "folded_scroll",
        "occurrence_scroll",
        "folded_occurrence_scroll",
    ] {
        e.buffer.erase_regions(e.id, crate::layout::FOLDS);
        e.set_caret(0, false);
        if scenario.contains("folded") {
            let folds = (1..1000)
                .map(|n| {
                    let row = n * 3;
                    Region::new(
                        e.text.line_to_byte(row + 1) - 1,
                        e.text.line_to_byte(row + 2),
                    )
                })
                .collect();
            e.buffer
                .add_regions(e.id, crate::layout::FOLDS, folds, true);
        }
        if scenario.contains("occurrence") {
            e.set_selection(Selection::from_regions([Region::new(3, 4)]));
        }
        let mut samples = Vec::new();
        let mut instructions = Vec::new();
        for n in 0..300 {
            v.scroll_y = ((n * 97) % 2500) as f64 * v.line_height;
            v.clamp_scroll(&e);
            ready(&mut e, &mut v);
            let before = counters::instructions();
            let at = Instant::now();
            black_box(v.scene(&e));
            samples.push(at.elapsed().as_secs_f64() * 1000.);
            instructions.push(counters::instructions() - before);
        }
        result.insert(
            scenario.into(),
            json!({"stats_ms":summarize(&samples),"samples_ms":samples,"instructions":instructions}),
        );
    }
    e.buffer.erase_regions(e.id, crate::layout::FOLDS);
    e.set_caret(10, false);
    let mut samples = Vec::new();
    let mut instructions = Vec::new();
    for _ in 0..200 {
        ready(&mut e, &mut v);
        let before = counters::instructions();
        let at = Instant::now();
        e.run("insert", &json!({"characters":"x"})).unwrap();
        black_box(v.scene(&e));
        samples.push(at.elapsed().as_secs_f64() * 1000.);
        instructions.push(counters::instructions() - before);
    }
    result.insert(
        "typing_churn".into(),
        json!({"stats_ms":summarize(&samples),"samples_ms":samples,"instructions":instructions}),
    );
    let output = json!(result);
    if let Some(path) = std::env::var_os("R16_PERF_OUTPUT") {
        std::fs::write(path, serde_json::to_string_pretty(&output).unwrap()).unwrap();
    }
    println!("R16_PERF {output}");
}

/// Mirrors the CLI feature scene and fixture, with counters outside its timer.
#[test]
#[ignore = "paired 10 MB scene counters; run explicitly without concurrent builds"]
fn real_highlighted_scenes() {
    let path = std::env::var_os("R16_REAL_FIXTURE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spikes/data/rust-10mb.rs")
        });
    let source = std::fs::read_to_string(path).unwrap();
    let mut e = Editor::new(Text::from(source.as_str()));
    e.attach_language(
        Assets::bundled().unwrap(),
        "Rust",
        Arc::new(vel_color_scheme::ColorScheme::velocity_dark()),
    )
    .unwrap();
    let mut v = View::new(
        Size {
            width: 3840.,
            height: 2160.,
        },
        7.8,
        12.,
    );
    let mut samples = Vec::new();
    let mut instructions = Vec::new();
    for n in 0..600 {
        v.scroll_y = ((n * 97) % e.text.len_lines()) as f64 * v.line_height;
        v.clamp_scroll(&e);
        let until = Instant::now() + Duration::from_secs(120);
        loop {
            e.poll_language();
            black_box(v.scene(&e));
            if v.exact(&e)
                && e.language
                    .borrow()
                    .as_ref()
                    .is_some_and(|l| l.ready() && l.highlight_complete())
            {
                break;
            }
            assert!(Instant::now() < until);
            std::thread::park_timeout(Duration::from_micros(250));
        }
        let before = counters::instructions();
        let at = Instant::now();
        black_box(v.scene(&e));
        samples.push(at.elapsed().as_secs_f64() * 1000.);
        instructions.push(counters::instructions() - before);
    }
    let result = json!({"real_scroll":{"stats_ms":summarize(&samples),
        "samples_ms":samples,"instructions":instructions}});
    if let Some(path) = std::env::var_os("R16_REAL_OUTPUT") {
        std::fs::write(path, serde_json::to_string_pretty(&result).unwrap()).unwrap();
    }
    println!("R16_REAL {result}");
}

// Public macOS ABI: https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/resource.h
// Own-process counters are read outside the timed windows. Other platforms skip them.
#[cfg(target_os = "macos")]
mod counters {
    #[repr(C)]
    #[derive(Default)]
    struct Info {
        ri_uuid: [u8; 16],
        ri_user_time: u64,
        ri_system_time: u64,
        ri_pkg_idle_wkups: u64,
        ri_interrupt_wkups: u64,
        ri_pageins: u64,
        ri_wired_size: u64,
        ri_resident_size: u64,
        ri_phys_footprint: u64,
        ri_proc_start_abstime: u64,
        ri_proc_exit_abstime: u64,
        ri_child_user_time: u64,
        ri_child_system_time: u64,
        ri_child_pkg_idle_wkups: u64,
        ri_child_interrupt_wkups: u64,
        ri_child_pageins: u64,
        ri_child_elapsed_abstime: u64,
        ri_diskio_bytesread: u64,
        ri_diskio_byteswritten: u64,
        ri_cpu_time_qos_default: u64,
        ri_cpu_time_qos_maintenance: u64,
        ri_cpu_time_qos_background: u64,
        ri_cpu_time_qos_utility: u64,
        ri_cpu_time_qos_legacy: u64,
        ri_cpu_time_qos_user_initiated: u64,
        ri_cpu_time_qos_user_interactive: u64,
        ri_billed_system_time: u64,
        ri_serviced_system_time: u64,
        ri_logical_writes: u64,
        ri_lifetime_max_phys_footprint: u64,
        ri_instructions: u64,
        ri_cycles: u64,
        ri_billed_energy: u64,
        ri_serviced_energy: u64,
        ri_interval_max_phys_footprint: u64,
        ri_runnable_time: u64,
    }
    #[link(name = "proc")]
    unsafe extern "C" {
        fn proc_pid_rusage(pid: i32, flavor: i32, info: *mut Info) -> i32;
    }
    pub fn instructions() -> u64 {
        let mut info = Info::default();
        // SAFETY: flavor 4 writes the public rusage_info_v4 layout above into
        // this live, correctly aligned buffer; the pid is this test process.
        let status = unsafe { proc_pid_rusage(std::process::id() as i32, 4, &mut info) };
        assert_eq!(status, 0);
        info.ri_instructions
    }
}
#[cfg(not(target_os = "macos"))]
mod counters {
    pub fn instructions() -> u64 {
        0
    }
}
