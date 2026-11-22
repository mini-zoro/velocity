//! Standalone, process-isolated benchmark runner; see README.md for invocation.
use std::{
    env,
    fs::{self, File},
    hint::black_box,
    io::{self, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Instant,
};
use vel_text::{Edit, Text};

const QUERIES: usize = 1_000_000;
const EDITS: usize = 10_000;

#[cfg(unix)]
fn usage() -> libc::rusage {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage initializes the supplied, correctly aligned rusage on success.
    // OS accounting is necessary here: allocator statistics omit resident pages.
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    assert_eq!(result, 0, "getrusage failed");
    // SAFETY: the successful call above initialized every field of rusage.
    unsafe { usage.assume_init() }
}

#[cfg(unix)]
fn peak_rss(usage: &libc::rusage) -> u64 {
    let bytes = usage.ru_maxrss as u64;
    if cfg!(target_os = "macos") {
        bytes
    } else {
        bytes * 1024
    }
}

#[cfg(target_os = "macos")]
fn resident_rss() -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v2>::uninit();
    // SAFETY: the v2 flavor writes a rusage_info_v2 to this aligned buffer. The
    // queried PID is this process. This distinguishes rest RSS from load's peak.
    let result = unsafe {
        libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V2,
            info.as_mut_ptr().cast(),
        )
    };
    if result != 0 {
        return None;
    }
    // SAFETY: a successful v2 call initialized the complete struct above.
    Some(unsafe { info.assume_init() }.ri_resident_size)
}
#[cfg(all(unix, not(target_os = "macos")))]
fn resident_rss() -> Option<u64> {
    None
}

#[cfg(unix)]
fn loadavg() -> String {
    match Command::new("sysctl").args(["-n", "vm.loadavg"]).output() {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        }
        Ok(output) => format!(
            "unavailable: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(e) => format!("unavailable: {e}"),
    }
}

#[cfg(target_os = "macos")]
fn retired_instructions() -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::uninit();
    // SAFETY: the v4 flavor initializes this aligned v4 buffer on success;
    // getpid has no preconditions and we query only the current process.
    let result = unsafe {
        libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V4,
            info.as_mut_ptr().cast(),
        )
    };
    if result != 0 {
        return None;
    }
    // SAFETY: the successful call above initialized the complete structure.
    Some(unsafe { info.assume_init() }.ri_instructions)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn retired_instructions() -> Option<u64> {
    None
}

fn measure<T: 'static>(
    implementation: &str,
    corpus: &str,
    operation: &str,
    bytes: usize,
    count: usize,
    f: impl FnOnce() -> T,
) -> T {
    #[cfg(unix)]
    let (load, before_instructions, before) = (loadavg(), retired_instructions(), usage());
    let start = Instant::now();
    let result = black_box(f());
    let ns = start.elapsed().as_nanos();
    #[cfg(unix)]
    let after = usage();
    #[cfg(unix)]
    let instructions = retired_instructions()
        .zip(before_instructions)
        .map(|(after, before)| (after - before).to_string())
        .unwrap_or_else(|| "null".to_owned());
    #[cfg(unix)]
    let resident = resident_rss()
        .map(|n| n.to_string())
        .unwrap_or_else(|| "null".to_owned());
    #[cfg(unix)]
    let cpu_ns = |u: &libc::rusage| {
        (u.ru_utime.tv_sec + u.ru_stime.tv_sec) as i128 * 1_000_000_000
            + (u.ru_utime.tv_usec + u.ru_stime.tv_usec) as i128 * 1000
    };
    #[cfg(unix)]
    let cpu_ns = cpu_ns(&after) - cpu_ns(&before);
    // Count reachable allocations after timing and RSS capture. This supplements
    // resident memory on hosts where compression can hide part of the corpus.
    let storage = (&result as &dyn std::any::Any)
        .downcast_ref::<Text>()
        .map(|text| text.estimated_storage_bytes().to_string())
        .unwrap_or_else(|| "null".to_owned());
    let run = env::var("VEL_TEXT_BENCH_RUN").unwrap_or_else(|_| "0".to_owned());
    #[cfg(unix)]
    println!(
        "{{\"implementation\":{implementation:?},\"corpus\":{corpus:?},\"operation\":{operation:?},\"run\":{run},\"total_ns\":{ns},\"cpu_ns\":{cpu_ns},\"retired_instructions\":{instructions},\"minor_faults\":{},\"major_faults\":{},\"count\":{count},\"content_bytes\":{bytes},\"peak_rss_bytes\":{},\"resident_rss_bytes\":{resident},\"estimated_storage_bytes\":{storage},\"loadavg\":{load:?}}}",
        after.ru_minflt - before.ru_minflt,
        after.ru_majflt - before.ru_majflt,
        peak_rss(&after)
    );
    #[cfg(not(unix))]
    println!(
        "{{\"implementation\":{implementation:?},\"corpus\":{corpus:?},\"operation\":{operation:?},\"run\":{run},\"total_ns\":{ns},\"cpu_ns\":null,\"retired_instructions\":null,\"minor_faults\":null,\"major_faults\":null,\"count\":{count},\"content_bytes\":{bytes},\"peak_rss_bytes\":null,\"resident_rss_bytes\":null,\"estimated_storage_bytes\":{storage},\"loadavg\":\"unavailable\"}}"
    );
    result
}

fn positions(n: usize, limit: usize) -> Vec<usize> {
    let mut seed = 0x59e1_0b73_faed_1234_u64;
    (0..n)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as usize % limit
        })
        .collect()
}

fn load_crop(path: &Path) -> io::Result<crop::Rope> {
    let mut builder = crop::RopeBuilder::new();
    let mut reader = BufReader::with_capacity(64 * 1024, File::open(path)?);
    let mut buf = vec![0; 64 * 1024];
    let mut carry = 0;
    loop {
        let n = reader.read(&mut buf[carry..])?;
        if n == 0 {
            assert_eq!(carry, 0);
            break;
        }
        let len = carry + n;
        let valid = match std::str::from_utf8(&buf[..len]) {
            Ok(s) => {
                builder.append(s);
                carry = 0;
                continue;
            }
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(e) => return Err(io::Error::new(io::ErrorKind::InvalidData, e)),
        };
        builder.append(std::str::from_utf8(&buf[..valid]).unwrap());
        carry = len - valid;
        buf.copy_within(valid..len, 0);
    }
    Ok(builder.build())
}

fn vel(corpus: &str, path: &Path, mode: &str) -> io::Result<()> {
    let size = fs::metadata(path)?.len() as usize;
    if matches!(mode, "scan" | "strings" | "arc_strings") {
        let mut reader = File::open(path)?;
        let mut buf = vec![0; 64 * 1024];
        measure("vel-text", corpus, mode, size, 1, || {
            let mut strings = Vec::new();
            let mut arcs = Vec::new();
            let mut bytes = 0;
            loop {
                let n = reader.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                bytes += n;
                let s = std::str::from_utf8(&buf[..n]).unwrap();
                if mode == "scan" {
                    black_box(memchr::memchr_iter(b'\n', s.as_bytes()).count());
                    black_box(s.is_ascii());
                } else {
                    for s in s.as_bytes().chunks(2048) {
                        let s = std::str::from_utf8(s).unwrap().to_owned();
                        if mode == "strings" {
                            strings.push(s);
                        } else {
                            arcs.push(std::sync::Arc::new(s));
                        }
                    }
                }
            }
            assert_eq!(bytes, size);
            // Retain the corpus until measurement ends; allocator reuse must not turn
            // this into an allocate/drop microbenchmark.
            (strings, arcs)
        });
        return Ok(());
    }
    if mode == "load" {
        let text = measure("vel-text", corpus, "load", size, 1, || {
            Text::from_reader(File::open(path).unwrap()).unwrap()
        });
        assert_eq!(text.len_bytes(), size);
        return Ok(());
    }
    let text = Text::from_reader(File::open(path)?)?;
    let q = positions(QUERIES, size);
    macro_rules! conversion {
        ($name:literal, $value:expr) => {
            measure("vel-text", corpus, $name, size, QUERIES, || {
                let mut sum = 0;
                for &i in &q {
                    sum ^= black_box($value(i));
                }
                black_box(sum)
            });
        };
    }
    conversion!("byte_char", |i| text.byte_to_char(i));
    conversion!("char_byte", |i| text.char_to_byte(i));
    conversion!("byte_utf16", |i| text.byte_to_utf16(i));
    conversion!("utf16_byte", |i| text.utf16_to_byte(i));
    conversion!("byte_line", |i| text.byte_to_line(i));
    let rows = positions(QUERIES, text.len_lines());
    measure("vel-text", corpus, "line_byte", size, QUERIES, || {
        rows.iter()
            .fold(0, |sum, &row| sum ^ black_box(text.line_to_byte(row)))
    });
    conversion!("byte_point", |i| black_box(text.byte_to_point(i)).column);
    conversion!("byte_point_utf16", |i| black_box(
        text.byte_to_point_utf16(i)
    )
    .column);
    let points: Vec<_> = q.iter().map(|&i| text.byte_to_point(i)).collect();
    measure("vel-text", corpus, "point_byte", size, QUERIES, || {
        points
            .iter()
            .fold(0, |sum, &p| sum ^ black_box(text.point_to_byte(p)))
    });
    let points16: Vec<_> = q.iter().map(|&i| text.byte_to_point_utf16(i)).collect();
    measure(
        "vel-text",
        corpus,
        "point_utf16_byte",
        size,
        QUERIES,
        || {
            points16
                .iter()
                .fold(0, |sum, &p| sum ^ black_box(text.point_utf16_to_byte(p)))
        },
    );
    drop((points, points16));
    let snapshots = measure("vel-text", corpus, "clone", size, 100_000, || {
        (0..100_000)
            .map(|_| black_box(&text).clone())
            .collect::<Vec<_>>()
    });
    black_box(&snapshots);
    drop(snapshots);
    let edits = positions(EDITS, size - 1);
    let mut changed = text.clone();
    measure("vel-text", corpus, "insert", size, EDITS, || {
        for &at in &edits {
            changed.insert(at, "x");
        }
    });
    assert_eq!(changed.len_bytes(), size + EDITS);
    drop(changed);
    let mut changed = text.clone();
    measure("vel-text", corpus, "delete", size, EDITS, || {
        for &at in &edits {
            let at = at % (changed.len_bytes() - 1);
            changed.delete(at..at + 1);
        }
    });
    assert_eq!(changed.len_bytes(), size - EDITS);
    drop(changed);
    for count in [10_000, 100_000] {
        let edits: Vec<_> = (0..count)
            .map(|line| {
                let at = text.line_to_byte(line);
                Edit {
                    range: at..at,
                    text: "foo",
                }
            })
            .collect();
        let mut changed = text.clone();
        measure(
            "vel-text",
            corpus,
            &format!("batch_lines_{count}"),
            size,
            1,
            || {
                changed.apply_edits(&edits);
                black_box(&changed);
            },
        );
        assert_eq!(changed.len_bytes(), size + count * 3);
        drop(changed);
        let mut changed = text.clone();
        measure(
            "vel-text",
            corpus,
            &format!("sequential_lines_{count}"),
            size,
            1,
            || {
                for edit in edits.iter().rev() {
                    changed.insert(edit.range.start, edit.text);
                }
                black_box(&changed);
            },
        );
        assert_eq!(changed.len_bytes(), size + count * 3);
    }
    let edits: Vec<_> = (0..100_000)
        .map(|i| {
            let at = i * (size / 100_000);
            Edit {
                range: at..at,
                text: "foo",
            }
        })
        .collect();
    let mut changed = text.clone();
    measure("vel-text", corpus, "batch_uniform_100000", size, 1, || {
        changed.apply_edits(&edits);
        black_box(&changed);
    });
    assert_eq!(changed.len_bytes(), size + 300_000);
    drop(changed);
    let mut changed = text.clone();
    measure(
        "vel-text",
        corpus,
        "sequential_uniform_100000",
        size,
        1,
        || {
            for edit in edits.iter().rev() {
                changed.insert(edit.range.start, edit.text);
            }
            black_box(&changed);
        },
    );
    assert_eq!(changed.len_bytes(), size + 300_000);
    drop(changed);
    let count = measure("vel-text", corpus, "chunks", size, 1, || {
        text.chunks().map(|s| black_box(s.len())).sum::<usize>()
    });
    assert_eq!(count, size);
    let count = measure("vel-text", corpus, "lines", size, 1, || {
        text.lines()
            .map(|s| black_box(s.len_bytes()))
            .sum::<usize>()
    });
    assert_eq!(count + text.len_lines() - 1, size);
    Ok(())
}

fn crop(corpus: &str, path: &Path, mode: &str) -> io::Result<()> {
    let size = fs::metadata(path)?.len() as usize;
    if mode == "load" {
        let text = measure("crop", corpus, "load", size, 1, || load_crop(path).unwrap());
        assert_eq!(text.byte_len(), size);
        return Ok(());
    }
    let text = load_crop(path)?;
    let q = positions(QUERIES, size);
    measure("crop", corpus, "byte_line", size, QUERIES, || {
        q.iter()
            .fold(0, |sum, &i| sum ^ black_box(text.line_of_byte(i)))
    });
    let rows = positions(QUERIES, text.line_len());
    measure("crop", corpus, "line_byte", size, QUERIES, || {
        rows.iter()
            .fold(0, |sum, &i| sum ^ black_box(text.byte_of_line(i)))
    });
    let snapshots = measure("crop", corpus, "clone", size, 100_000, || {
        (0..100_000)
            .map(|_| black_box(&text).clone())
            .collect::<Vec<_>>()
    });
    drop(snapshots);
    let edits = positions(EDITS, size - 1);
    let mut changed = text.clone();
    measure("crop", corpus, "insert", size, EDITS, || {
        for &at in &edits {
            changed.insert(at, "x");
        }
    });
    drop(changed);
    let mut changed = text.clone();
    measure("crop", corpus, "delete", size, EDITS, || {
        for &at in &edits {
            let at = at % (changed.byte_len() - 1);
            changed.delete(at..at + 1);
        }
    });
    drop(changed);
    for count in [10_000, 100_000] {
        let offsets: Vec<_> = (0..count).map(|line| text.byte_of_line(line)).collect();
        let mut changed = text.clone();
        measure(
            "crop",
            corpus,
            &format!("batch_lines_{count}"),
            size,
            1,
            || {
                for &at in offsets.iter().rev() {
                    changed.insert(at, "foo");
                }
            },
        );
    }
    let mut changed = text.clone();
    measure("crop", corpus, "batch_uniform_100000", size, 1, || {
        for i in (0..100_000).rev() {
            changed.insert(i * (size / 100_000), "foo");
        }
    });
    drop(changed);
    measure("crop", corpus, "chunks", size, 1, || {
        text.chunks().map(|s| black_box(s.len())).sum::<usize>()
    });
    measure("crop", corpus, "lines", size, 1, || {
        text.lines().map(|s| black_box(s.byte_len())).sum::<usize>()
    });
    Ok(())
}

fn ropey(corpus: &str, path: &Path, mode: &str) -> io::Result<()> {
    let size = fs::metadata(path)?.len() as usize;
    if mode == "load" {
        let text = measure("ropey", corpus, "load", size, 1, || {
            ropey::Rope::from_reader(File::open(path).unwrap()).unwrap()
        });
        assert_eq!(text.len_bytes(), size);
        return Ok(());
    }
    let text = ropey::Rope::from_reader(File::open(path)?)?;
    let q = positions(QUERIES, size);
    macro_rules! conversion {
        ($name:literal, $value:expr) => {
            measure("ropey", corpus, $name, size, QUERIES, || {
                q.iter().fold(0, |sum, &i| sum ^ black_box($value(i)))
            });
        };
    }
    conversion!("byte_char", |i| text.byte_to_char(i));
    conversion!("char_byte", |i| text.char_to_byte(i));
    conversion!("byte_utf16", |i| text
        .char_to_utf16_cu(text.byte_to_char(i)));
    conversion!("utf16_byte", |i| text
        .char_to_byte(text.utf16_cu_to_char(i)));
    conversion!("byte_line", |i| text.byte_to_line(i));
    let rows = positions(QUERIES, text.len_lines());
    measure("ropey", corpus, "line_byte", size, QUERIES, || {
        rows.iter()
            .fold(0, |sum, &i| sum ^ black_box(text.line_to_byte(i)))
    });
    let snapshots = measure("ropey", corpus, "clone", size, 100_000, || {
        (0..100_000)
            .map(|_| black_box(&text).clone())
            .collect::<Vec<_>>()
    });
    drop(snapshots);
    let edits = positions(EDITS, size - 1);
    let mut changed = text.clone();
    measure("ropey", corpus, "insert", size, EDITS, || {
        for &at in &edits {
            changed.insert(at, "x");
        }
    });
    drop(changed);
    let mut changed = text.clone();
    measure("ropey", corpus, "delete", size, EDITS, || {
        for &at in &edits {
            let at = at % (changed.len_chars() - 1);
            changed.remove(at..at + 1);
        }
    });
    drop(changed);
    for count in [10_000, 100_000] {
        let offsets: Vec<_> = (0..count).map(|line| text.line_to_char(line)).collect();
        let mut changed = text.clone();
        measure(
            "ropey",
            corpus,
            &format!("batch_lines_{count}"),
            size,
            1,
            || {
                for &at in offsets.iter().rev() {
                    changed.insert(at, "foo");
                }
            },
        );
    }
    let mut changed = text.clone();
    measure("ropey", corpus, "batch_uniform_100000", size, 1, || {
        for i in (0..100_000).rev() {
            changed.insert(i * (size / 100_000), "foo");
        }
    });
    drop(changed);
    measure("ropey", corpus, "chunks", size, 1, || {
        text.chunks().map(|s| black_box(s.len())).sum::<usize>()
    });
    measure("ropey", corpus, "lines", size, 1, || {
        text.lines()
            .map(|s| black_box(s.len_bytes()))
            .sum::<usize>()
    });
    Ok(())
}

fn unicode_corpus() -> io::Result<PathBuf> {
    let path = env::temp_dir().join("velocity-vel-text-unicode-100mb.txt");
    const LEN: usize = 100_000_000;
    if fs::metadata(&path).is_ok_and(|m| m.len() == LEN as u64) {
        return Ok(path);
    }
    // 30.2% of bytes are non-ASCII: CJK, astral emoji and combining marks.
    let line = "let value = 123456789; // sample text abc 世界中😀🦀e\u{301}\n";
    let block = line.repeat(1024);
    let mut file = BufWriter::new(File::create(&path)?);
    let mut left = LEN;
    while left >= block.len() {
        file.write_all(block.as_bytes())?;
        left -= block.len();
    }
    let valid = block.floor_char_boundary(left);
    file.write_all(&block.as_bytes()[..valid])?;
    file.write_all(&vec![b' '; left - valid])?;
    file.flush()?;
    Ok(path)
}

fn main() -> io::Result<()> {
    let args: Vec<_> = env::args().collect();
    if args.get(1).is_some_and(|s| s == "--worker") {
        return match args[2].as_str() {
            "vel-text" => vel(&args[3], Path::new(&args[5]), &args[4]),
            "crop" => crop(&args[3], Path::new(&args[5]), &args[4]),
            "ropey" => ropey(&args[3], Path::new(&args[5]), &args[4]),
            _ => panic!("unknown implementation"),
        };
    }
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
    };
    let runs = value("--runs").map_or(5, |s| s.parse::<usize>().expect("integer run count"));
    assert!(runs >= 5, "at least five independent runs are required");
    let data = value("--data").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spikes/data"),
        PathBuf::from,
    );
    let unicode = unicode_corpus()?;
    let corpora = [
        ("ascii-100mb", data.join("text-100mb.txt")),
        ("ascii-1gb", data.join("text-1gb.txt")),
        ("unicode-100mb", unicode),
    ];
    let executable = env::current_exe()?;
    let profile = args.iter().any(|a| a == "--profile");
    let modes: &[&str] = if profile {
        &["scan", "strings", "arc_strings"]
    } else {
        &["load", "ops"]
    };
    for implementation in ["vel-text", "crop", "ropey"] {
        if value("--impl").is_some_and(|s| s != implementation) {
            continue;
        }
        for (corpus, path) in &corpora {
            if profile && *corpus != "ascii-1gb" {
                continue;
            }
            for &mode in modes {
                if mode == "ops" && *corpus != "ascii-1gb" {
                    continue;
                }
                if value("--only").is_some_and(|s| s != mode) {
                    continue;
                }
                for run in 1..=runs {
                    eprintln!("{implementation} {corpus} {mode} run {run}/{runs}");
                    let output = Command::new(&executable)
                        .arg("--worker")
                        .arg(implementation)
                        .arg(corpus)
                        .arg(mode)
                        .arg(path)
                        .env("VEL_TEXT_BENCH_RUN", run.to_string())
                        .stderr(Stdio::inherit())
                        .output()?;
                    if !output.status.success() {
                        return Err(io::Error::other(format!(
                            "worker failed: {}",
                            output.status
                        )));
                    }
                    io::stdout().write_all(&output.stdout)?;
                    io::stdout().flush()?;
                }
            }
        }
    }
    Ok(())
}
