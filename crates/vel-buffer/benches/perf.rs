//! Process-isolated release measurements of complete editing operations.
use std::{
    env,
    fs::File,
    hint::black_box,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Instant,
};
use vel_buffer::{Buffer, Edit, EditKind, Region, Selection, ops};
use vel_text::Text;
#[cfg(unix)]
fn usage() -> libc::rusage {
    let mut u = std::mem::MaybeUninit::uninit();
    // SAFETY: the correctly aligned rusage is initialized on successful getrusage.
    let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, u.as_mut_ptr()) };
    assert_eq!(status, 0);
    // SAFETY: success above initialized the entire structure.
    unsafe { u.assume_init() }
}
#[cfg(unix)]
fn loadavg() -> String {
    match Command::new("sysctl").args(["-n", "vm.loadavg"]).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().into(),
        Ok(o) => format!("unavailable: {}", String::from_utf8_lossy(&o.stderr).trim()),
        Err(e) => format!("unavailable: {e}"),
    }
}
#[cfg(unix)]
fn measure<T>(op: &str, count: usize, f: impl FnOnce() -> T) -> T {
    let load = loadavg();
    let before = usage();
    let start = Instant::now();
    let value = black_box(f());
    let ns = start.elapsed().as_nanos();
    let after = usage();
    let cpu = |u: libc::rusage| {
        (u.ru_utime.tv_sec + u.ru_stime.tv_sec) as i128 * 1_000_000_000
            + (u.ru_utime.tv_usec + u.ru_stime.tv_usec) as i128 * 1000
    };
    let rss = after.ru_maxrss as u64 * if cfg!(target_os = "macos") { 1 } else { 1024 };
    let run = env::var("VEL_BUFFER_BENCH_RUN").unwrap_or_else(|_| "0".into());
    println!(
        "{{\"operation\":{op:?},\"run\":{run},\"count\":{count},\"total_ns\":{ns},\"cpu_ns\":{},\"minor_faults\":{},\"major_faults\":{},\"peak_rss_bytes\":{rss},\"loadavg\":{load:?}}}",
        cpu(after) - cpu(before),
        after.ru_minflt - before.ru_minflt,
        after.ru_majflt - before.ru_majflt
    );
    value
}
#[cfg(not(unix))]
fn measure<T>(op: &str, count: usize, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let value = black_box(f());
    let ns = start.elapsed().as_nanos();
    let run = env::var("VEL_BUFFER_BENCH_RUN").unwrap_or_else(|_| "0".into());
    println!(
        "{{\"operation\":{op:?},\"run\":{run},\"count\":{count},\"total_ns\":{ns},\"cpu_ns\":null,\"minor_faults\":null,\"major_faults\":null,\"peak_rss_bytes\":null,\"loadavg\":\"unavailable\"}}"
    );
    value
}
fn carets(text: &Text, n: usize) -> Selection {
    let mut offset = 0;
    let regions: Vec<_> = text
        .lines()
        .take(n)
        .map(|line| {
            let r = Region::caret(offset);
            offset += line.len_bytes() + 1;
            r
        })
        .collect();
    assert_eq!(regions.len(), n);
    Selection::from_regions(regions)
}
fn worker(op: &str, path: &Path) -> io::Result<()> {
    let text = Text::from_reader(File::open(path)?)?;
    assert_eq!(text.len_bytes(), 100_000_000);
    let mut b = Buffer::new(text);
    let v = b.new_view();
    match op {
        "type_10000" | "type_100000" => {
            let n = if op == "type_10000" { 10_000 } else { 100_000 };
            b.set_selection(v, carets(b.text(), n));
            ops::insert(&mut b, v, "x");
            let before = b.text().len_bytes();
            measure(op, 1, || ops::insert(&mut b, v, "y"));
            assert_eq!(b.text().len_bytes(), before + n);
            assert_eq!(b.view(v).sel().len(), n);
            assert!(b.undo(v));
            assert_eq!(b.text().len_bytes(), 100_000_000);
        }
        "undo_10000" => {
            b.set_selection(v, carets(b.text(), 10_000));
            for s in ["a", "b", "c"] {
                ops::insert(&mut b, v, s);
            }
            measure(op, 1, || assert!(b.undo(v)));
            assert_eq!(b.text().len_bytes(), 100_000_000);
            assert_eq!(b.view(v).sel().len(), 10_000);
        }
        "delete_50mb" | "undo_50mb" => {
            // A shared 50 MB subdocument, then select all. Fixture construction is outside timing.
            let mut text = b.text().clone();
            text.delete(50_000_000..text.len_bytes());
            b = Buffer::new(text);
            let v = b.new_view();
            b.set_selection(v, Selection::from(Region::new(0, 50_000_000)));
            if op == "delete_50mb" {
                measure(op, 1, || ops::insert(&mut b, v, ""));
                assert!(b.text().is_empty());
                assert!(b.undo(v));
            } else {
                ops::insert(&mut b, v, "");
                measure(op, 1, || assert!(b.undo(v)));
            }
            assert_eq!(b.text().len_bytes(), 50_000_000);
        }
        "map_100000" => {
            let marks = carets(b.text(), 100_000).iter().collect();
            b.add_regions(v, "marks", marks, false);
            b.set_selection(v, carets(b.text(), 10_000));
            measure(op, 1, || ops::insert(&mut b, v, "x"));
            assert_eq!(
                b.view(v).get_regions("marks").unwrap().regions.len(),
                100_000
            );
            assert_eq!(b.view(v).get_regions("marks").unwrap().regions[0].b, 1);
        }
        "find_10000" => {
            // Repeated public calls, retaining the same incremental search state as Cmd+D.
            let mut first = None;
            let mut base = 0;
            for c in b.text().chunks() {
                if let Some(at) = memchr::memmem::find(c.as_bytes(), b"use") {
                    first = Some(base + at);
                    break;
                }
                base += c.len();
            }
            let p = first.expect("corpus has repeated 'use'");
            b.set_selection(v, Selection::from(Region::new(p, p + 3)));
            for _ in 1..9999 {
                assert!(ops::find_under_expand(&mut b, v));
            }
            measure(op, 1, || assert!(ops::find_under_expand(&mut b, v)));
            assert_eq!(b.view(v).sel().len(), 10_000);
        }
        "undo_metadata" => {
            b.set_selection(v, carets(b.text(), 10_000));
            let before = b.view(v).sel().clone();
            let mut regions = Vec::with_capacity(10_000);
            let mut edits = Vec::with_capacity(10_000);
            for (i, r) in before.iter().enumerate() {
                edits.push(Edit {
                    range: r.to_range(),
                    text: "x",
                });
                regions.push(Region::caret(r.b + i + 1));
            }
            b.transact(
                v,
                EditKind::InsertChars,
                &edits,
                Selection::from_regions(regions),
            );
            let bytes = b.last_undo_metadata_bytes();
            let run = env::var("VEL_BUFFER_BENCH_RUN").unwrap();
            println!(
                "{{\"operation\":\"undo_metadata\",\"run\":{run},\"count\":10000,\"bytes\":{bytes},\"loadavg\":{:?}}}",
                loadavg()
            );
            assert!(b.undo(v));
        }
        _ => panic!("unknown scenario"),
    }
    Ok(())
}
fn main() -> io::Result<()> {
    let args: Vec<_> = env::args().collect();
    if args.get(1).is_some_and(|a| a == "--worker") {
        return worker(&args[2], Path::new(&args[3]));
    }
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
    };
    let runs = value("--runs").map_or(5, |s| s.parse::<usize>().expect("integer runs"));
    assert!(runs >= 5, "at least five independent runs");
    let data = value("--data").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spikes/data/text-100mb.txt"),
        PathBuf::from,
    );
    for op in [
        "type_10000",
        "type_100000",
        "find_10000",
        "undo_10000",
        "delete_50mb",
        "undo_50mb",
        "map_100000",
        "undo_metadata",
    ] {
        if value("--only").is_some_and(|s| s != op) {
            continue;
        }
        for run in 1..=runs {
            eprintln!("vel-buffer {op} {run}/{runs}");
            let result = Command::new(env::current_exe()?)
                .args(["--worker", op])
                .arg(&data)
                .env("VEL_BUFFER_BENCH_RUN", run.to_string())
                .stderr(Stdio::inherit())
                .output()?;
            if !result.status.success() {
                return Err(io::Error::other(format!(
                    "{op} worker failed: {}",
                    result.status
                )));
            }
            io::stdout().write_all(&result.stdout)?;
            io::stdout().flush()?;
        }
    }
    Ok(())
}
