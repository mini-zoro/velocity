//! Opt-in release workloads. Both create only disposable fixture data.
use std::{
    fs,
    io::{BufWriter, Write},
    path::Path,
    time::{Duration, Instant},
};
use vel_search::{
    files::{Event, Line, Request, Results, Task, Where},
    project::{Folder, Rules},
    text::{Options, Search},
};

fn folder(path: &Path) -> Folder {
    Folder {
        path: path.into(),
        name: path.display().to_string(),
        rules: Rules::default(),
        follow_links: false,
    }
}

#[test]
#[ignore = "release benchmark; writes and removes one 300 MB fixture at a time"]
fn find_in_files_300mb_single_file_matrix() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let path = root.join("large.txt");
    for newline in ["\n", "\r\n"] {
        let line = format!("let café = value; return item;{newline}");
        let block = line.repeat((128 * 1024) / line.len());
        let blocks = 300_000_000 / block.len();
        let mut output = BufWriter::new(fs::File::create(&path).unwrap());
        for _ in 0..blocks {
            output.write_all(block.as_bytes()).unwrap();
        }
        write!(output, "tail one{newline}two end{newline}").unwrap();
        output.flush().unwrap();
        drop(output);
        let n = blocks * (block.len() / line.len());
        for (query, regex, expected) in [
            ("zqxjvwk", false, 0),
            ("value", false, n),
            (r"ret\w*n\b", true, n),
            ("tail one\ntwo end", false, 1),
        ] {
            let started = Instant::now();
            let task = Task::start(Request {
                search: Search::new(
                    query,
                    Options {
                        regex,
                        ..Default::default()
                    },
                )
                .unwrap(),
                scope: Where::parse("", root).unwrap(),
                folders: vec![folder(root)],
                open: vec![],
                context: 0,
            })
            .unwrap();
            let mut rows = 0;
            loop {
                match task.events.recv_timeout(Duration::from_secs(120)).unwrap() {
                    Event::Line(_) => rows += 1,
                    Event::Done { matches, files } => {
                        assert_eq!((matches, files), (expected, 1), "{query:?}");
                        println!(
                            "{newline:?} {query:?}: {:?}, {rows} rows",
                            started.elapsed()
                        );
                        break;
                    }
                    Event::Error(error) => panic!("{error}"),
                }
            }
        }
        fs::remove_file(&path).unwrap();
    }
}

#[test]
#[ignore = "release benchmark; writes and removes a 100k-file fixture"]
fn find_in_files_100k_query_matrix() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let dirs: Vec<_> = (0..200)
        .map(|n| {
            let dir = root.join(format!("d{n:03}"));
            fs::create_dir(&dir).unwrap();
            dir
        })
        .collect();
    let template = "    let item = something_with_a_common_value;\n".repeat(16);
    let files = 100_000;
    for i in 0..files {
        let middle = if i % 31 == 0 {
            "    return value;\n"
        } else {
            "    return other;\n"
        };
        fs::write(
            dirs[i % dirs.len()].join(format!("file{i:06}.rs")),
            format!("{template}{middle}{template}"),
        )
        .unwrap();
    }
    for (query, regex, want) in [
        ("return value", false, files.div_ceil(31)),
        (r"return\svalue", true, files.div_ceil(31)),
        (r"return\s+value", true, files.div_ceil(31)),
        ("zqx\nzqy", false, 0),
        ("\n\n\n\n", false, 0),
        (r"^\s*\n\s*$\n", true, 0),
    ] {
        let started = Instant::now();
        let folders = vec![folder(root)];
        let mut results = Results::ordered(&folders);
        let task = Task::start(Request {
            scope: Where::parse("", root).unwrap(),
            search: Search::new(
                query,
                Options {
                    regex,
                    ..Options::default()
                },
            )
            .unwrap(),
            folders,
            open: vec![],
            context: 1,
        })
        .unwrap();
        loop {
            match task.events.recv_timeout(Duration::from_secs(180)).unwrap() {
                Event::Line(line) => {
                    results.push(line);
                }
                Event::Done {
                    matches,
                    files: seen,
                } => {
                    assert_eq!((matches, seen), (want, files), "{query:?}");
                    println!(
                        "{query:?}: {:?}, {} result rows",
                        started.elapsed(),
                        results.rows.len()
                    );
                    break;
                }
                Event::Error(error) => panic!("{error}"),
            }
        }
    }
}

#[test]
#[ignore = "release benchmark; one million distinct files, in App-sized batches"]
fn million_distinct_files_in_256_event_batches() {
    let root = Path::new("/project");
    let mut results = Results::ordered(&[folder(root)]);
    let mut durations = Vec::new();
    let mut order = 0;
    while order < 1_000_000 {
        let batch: Vec<_> = (order..(order + 256).min(1_000_000))
            .map(|i| {
                let n = (i * 61_891 + 123_453) % 1_000_000;
                Line {
                    path: root.join(format!("d{}/src/file{n:06}.rs", n % 201)).into(),
                    number: 1,
                    text: "needle on this line".into(),
                    highlights: std::iter::once(0..6).collect(),
                    column_offset: 0,
                    match_count: 1,
                }
            })
            .collect();
        order += batch.len();
        let start = Instant::now();
        for line in batch {
            results.push(line);
        }
        durations.push(start.elapsed());
    }
    assert_eq!(results.matched_files(), 1_000_000);
    assert_eq!(results.rows.len(), 2_000_000);
    durations.sort_unstable();
    println!(
        "256 events: p50={:?}, p99={:?}, max={:?}; total={:?}",
        durations[durations.len() / 2],
        durations[durations.len() * 99 / 100],
        durations.last().unwrap(),
        durations.iter().sum::<Duration>()
    );
}
