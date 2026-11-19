//! Paired 100k-path ranking benchmark. Place alongside unmodified
//! fuzzy-before.rs (comparison revision) and fuzzy-after.rs, then build
//! with `rustc --edition 2024 -O round21-fuzzy.rs`.
//! Input: `round21-fuzzy repo|deps|repeated paths.txt [pairs] [ordered|shuffled|both] [fresh|typing|both]`.
#![allow(dead_code)]
#[path = "fuzzy-after.rs"]
mod after;
#[path = "fuzzy-before.rs"]
mod before;
use std::{hint::black_box, path::Path, time::Instant};

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn cpu_ms() -> f64 {
    use std::ffi::{c_int, c_long};
    #[repr(C)]
    struct Timespec {
        sec: c_long,
        nsec: c_long,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: c_int, out: *mut Timespec) -> c_int;
    }
    let mut out = Timespec { sec: 0, nsec: 0 };
    let clock = if cfg!(target_os = "macos") { 12 } else { 2 }; // CLOCK_PROCESS_CPUTIME_ID
    // SAFETY: clock_gettime writes synchronously to a live timespec.
    assert_eq!(unsafe { clock_gettime(clock, &mut out) }, 0);
    out.sec as f64 * 1000. + out.nsec as f64 / 1e6
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn cpu_ms() -> f64 {
    f64::NAN
}

fn paired<T>(pairs: usize, before: impl Fn() -> T, after: impl Fn() -> T) -> Vec<(T, T)> {
    (0..pairs + 6)
        .filter_map(|i| {
            let (before, after) = if i % 2 == 0 {
                (before(), after())
            } else {
                let after = after();
                (before(), after)
            };
            (i >= 6).then_some((before, after))
        })
        .collect()
}

fn report(label: &str, samples: impl Iterator<Item = (f64, f64)>) {
    let (mut a, mut b): (Vec<_>, Vec<_>) = samples.unzip();
    a.sort_by(f64::total_cmp);
    b.sort_by(f64::total_cmp);
    let row = |i: usize| (a[i], b[i], 100. * (b[i] / a[i] - 1.));
    let (a50, b50, d50) = row(a.len() / 2);
    let (a95, b95, d95) = row((a.len() * 95).div_ceil(100) - 1);
    println!(
        "{label:>25} p50 {a50:7.3} -> {b50:7.3} ms ({d50:+6.1}%)   p95 {a95:7.3} -> {b95:7.3} ms ({d95:+6.1}%)"
    );
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let input = std::fs::read_to_string(&args[2]).unwrap();
    let paths: Vec<_> = input.lines().filter(|p| !p.is_empty()).collect();
    let pairs = args.get(3).map_or(51, |n| n.parse::<usize>().unwrap());
    let order = args.get(4).map_or("both", String::as_str);
    let mode = args.get(5).map_or("both", String::as_str);
    assert!(pairs >= 11);
    assert!(matches!(order, "ordered" | "shuffled" | "both"));
    assert!(matches!(mode, "fresh" | "typing" | "both"));
    let mut labels: Vec<_> = (0..100_000)
        .map(|i| {
            let path = Path::new(paths[i % paths.len()]);
            let copy = i / paths.len();
            match args[1].as_str() {
                "repo" => {
                    let dir = path.parent().unwrap().display();
                    let name = path.file_stem().unwrap().to_str().unwrap();
                    let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    // Keep the repository's actual directory distribution. All
                    // basenames, including duplicate stems within a copy, are unique.
                    format!("snapshot/{copy:03}/{dir}/{name}_{i:05}.{extension}")
                }
                "deps" => format!("workspace/{copy:03}/{}", path.display()),
                "repeated" => {
                    let name = [
                        "main.rs",
                        "maintenance.rs",
                        "lib.rs",
                        "callback.rs",
                        "index_service.rs",
                        "search_fuzzy.rs",
                        "AbstractController.rs",
                        "fuzzy_core.rs",
                    ][i % 8];
                    format!("crates/workspace-{}/{i}/src/{name}", i % 997)
                }
                _ => panic!("unknown layout"),
            }
        })
        .collect();
    labels.sort_unstable();
    if order != "shuffled" {
        println!("{}: directory order ({pairs} alternating pairs)", args[1]);
        measure(&labels, &args[1], mode, pairs);
    }
    if order != "ordered" {
        let mut state = 12_409_377_u64;
        for i in (1..labels.len()).rev() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            labels.swap(i, state as usize % (i + 1));
        }
        println!("{}: shuffled ({pairs} alternating pairs)", args[1]);
        measure(&labels, &args[1], mode, pairs);
    }
}

fn measure(labels: &[String], layout: &str, mode: &str, pairs: usize) {
    let old: Vec<_> = labels.iter().map(before::Candidate::new).collect();
    let new: Vec<_> = labels.iter().map(after::Candidate::new).collect();
    macro_rules! fresh {
        ($mod:ident, $items:expr, $query:expr) => {{
            let mut matcher = $mod::Matcher::default();
            let cpu = cpu_ms();
            let start = Instant::now();
            black_box(matcher.rank(black_box($items), black_box($query), 100));
            (start.elapsed().as_secs_f64() * 1000., cpu_ms() - cpu)
        }};
    }
    macro_rules! typed {
        ($mod:ident, $items:expr, $query:expr) => {{
            let mut matcher = $mod::Matcher::default();
            let mut timings = Vec::with_capacity($query.len());
            for end in 1..=$query.len() {
                let key = &$query[..end];
                let start = Instant::now();
                black_box(matcher.rank(black_box($items), black_box(key), 100));
                timings.push(start.elapsed().as_secs_f64() * 1000.);
            }
            timings
        }};
    }
    let prompts: &[&str] = if layout == "deps" {
        &[
            "m",
            "ac",
            "search",
            "index",
            "lib",
            "node_modules/react",
            "src/main",
            "index map",
            "e/",
            "c/",
            "s/te",
        ]
    } else {
        &[
            "m",
            "ac",
            "search",
            "workspace",
            "main",
            "index",
            "lib",
            "src/main",
            "search fuzzy",
            "e/",
            "c/",
            "s/te",
        ]
    };
    if mode != "typing" {
        for &query in prompts {
            let timings = paired(
                pairs,
                || fresh!(before, &old, query),
                || fresh!(after, &new, query),
            );
            report(
                &format!("fresh {query}"),
                timings.iter().map(|(a, b)| (a.0, b.0)),
            );
            report(
                &format!("CPU {query}"),
                timings.iter().map(|(a, b)| (a.1, b.1)),
            );
        }
    }
    if mode != "fresh" {
        let paths: &[&str] = if layout == "deps" {
            &["e/", "c/", "s/te", "src/main", "node_modules/react"]
        } else {
            &["e/", "c/", "s/te", "src/main", "crates/velocity"]
        };
        for (kind, prompts) in [
            (
                "word",
                &["search", "workspace", "index", "ac", "main", "lib"][..],
            ),
            ("path", paths),
        ] {
            let mut all = Vec::new();
            for &query in prompts {
                let timings = paired(
                    pairs,
                    || typed!(before, &old, query),
                    || typed!(after, &new, query),
                );
                for key in 1..=query.len() {
                    let samples = timings.iter().map(|(a, b)| (a[key - 1], b[key - 1]));
                    if matches!(&query[..key], "e/" | "c/" | "s/" | "s/t" | "s/te") {
                        report(&format!("typed {}", &query[..key]), samples.clone());
                    }
                    all.extend(samples);
                }
            }
            report(&format!("typing {kind}"), all.into_iter());
        }
    }
}
