//! Per-key App-path measurements. Startup and resource loading are outside the counters.
use super::*;
fn process_instructions() -> u64 {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
    // SAFETY: flavor 4 writes the public rusage_info_v4 to this aligned, live
    // allocation. This measures our process and retains no pointer.
    let status =
        unsafe { libc::proc_pid_rusage(std::process::id() as i32, 4, info.as_mut_ptr().cast()) };
    assert_eq!(status, 0);
    // SAFETY: the successful call initialized the complete structure.
    unsafe { info.assume_init().ri_instructions }
}
// ABI facts only, from Apple's public XNU resource_private.h:
// https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/resource_private.h
// THSC_CPI=1 writes instructions/cycles for the calling thread (macOS 12.4+).
#[repr(C)]
#[derive(Default)]
struct ThreadCounts {
    instructions: u64,
    cycles: u64,
}
unsafe extern "C" {
    fn thread_selfcounts(kind: u32, counts: *mut ThreadCounts, size: usize) -> libc::c_int;
}
fn instructions() -> u64 {
    let mut counts = ThreadCounts::default();
    // SAFETY: kind 1 copies the two u64 counters into this live repr(C)
    // allocation of exactly the specified size; the OS retains no pointer.
    let status = unsafe { thread_selfcounts(1, &mut counts, std::mem::size_of::<ThreadCounts>()) };
    assert_eq!(status, 0, "App-thread instruction counters unavailable");
    assert!(counts.instructions > 0);
    counts.instructions
}
fn app() -> App {
    let options = Options::default();
    let mut workspace = Workspace::new(
        vel_text::Text::from(
            format!("{}\n", "some code with spaces and words ".repeat(4)).repeat(10000),
        ),
        None,
        None,
        false,
    )
    .unwrap();
    workspace.wait_resources();
    let mut a = App::new(
        options.clone(),
        Clock::new(Instant::now(), std::time::SystemTime::now()),
        Reporter::for_options(&options, Instant::now()).unwrap(),
        workspace,
        Arc::new(AtomicBool::new(false)),
        std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(None))),
    );
    a.panels.side_visible = false;
    a.run_named(
        None,
        "set_setting",
        &json!({"setting":"animation_enabled","value":false}),
    );
    a
}
#[test]
#[ignore = "paired App-path retired-instruction benchmark; no GUI"]
fn app_key_cost() {
    let mut results = serde_json::Map::new();
    for groups in [1, 2] {
        let mut a = app();
        if groups == 2 {
            a.run_named(None, "set_layout", &json!({"preset":"columns2"}));
            // Populate both panes with independent views of the same buffer.
            // This includes shared-view transfer and remote anchor upkeep per key.
            a.workspace.clone_file().unwrap();
            a.run_named(None, "move_to_group", &json!({"group":1}));
            a.run_named(None, "focus_group", &json!({"group":0}));
        }
        a.run_named(
            None,
            "set_setting",
            &json!({"setting":"word_wrap","value":groups==2}),
        );
        let insert = vel_config::keymap::Key::parse("a").unwrap();
        let delete = vel_config::keymap::Key::parse("backspace").unwrap();
        // Settle resource/analysis workers before the first per-key sample.
        for _ in 0..100 {
            a.workspace.poll();
            let viewport = a.viewport();
            a.panels.poll(&mut a.workspace, viewport);
            std::hint::black_box(a.scene());
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(a.workspace.themes.ui.borrow().metrics.native_enabled());
        if groups == 2 {
            assert!(a.workspace.editor().layout.borrow().wrapped());
            assert!(
                a.view.total(a.workspace.editor()) > a.workspace.editor().text.len_lines(),
                "fixture must contain actual wrapped rows"
            );
        }
        let mut totals = [0u64; 3];
        let mut process_total = 0;
        let mut elapsed = Duration::ZERO;
        for n in 0..1200 {
            let at = Instant::now();
            let process_before = process_instructions();
            let before = instructions();
            let viewport = a.viewport();
            let bound = a.dispatch_key_binding(
                None,
                if n % 2 == 0 { &insert } else { &delete },
                at,
                viewport,
            );
            if !bound {
                a.workspace.editor_mut().text_input(TextInput::Insert {
                    text: "a".into(),
                    replacement: None,
                });
            }
            let dispatched = instructions();
            if !bound {
                a.changed_state(at, true);
            }
            let changed = instructions();
            std::hint::black_box(a.scene());
            let scene = instructions();
            let process_after = process_instructions();
            if n >= 200 {
                process_total += process_after - process_before;
                totals[0] += dispatched - before;
                totals[1] += changed - dispatched;
                totals[2] += scene - changed;
                elapsed += at.elapsed();
            }
        }
        results.insert(groups.to_string(), json!({"keys":1000,"instructions_per_key":totals.iter().sum::<u64>()/1000,
            "process_instructions_per_key":process_total/1000,"dispatch":totals[0]/1000,"changed":totals[1]/1000,"scene":totals[2]/1000,"ms_per_key":elapsed.as_secs_f64()}));
    }
    println!("APP_KEY_COST {}", Value::Object(results));
}
