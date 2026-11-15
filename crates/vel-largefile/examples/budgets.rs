//! Process-isolated A/B budget probes; orchestrate with scripts/measure.py.
use serde_json::json;
use std::{
    env,
    fs::{self, File},
    hint::black_box,
    io::{self, Read, Write},
    path::Path,
    time::{Duration, Instant},
};
use unicode_width::UnicodeWidthChar;
use vel_largefile::{LoadOptions, LongLineIndex, load_utf8};
use vel_text::Text;
fn rss() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    assert_eq!(
        // SAFETY: getrusage initializes the aligned structure on a successful call.
        unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
        0
    );
    // SAFETY: the successful call initialized the structure.
    unsafe { usage.assume_init() }.ru_maxrss as u64
        * if cfg!(target_os = "macos") { 1 } else { 1024 }
}
fn record(operation: &str, side: &str, samples: &[u128], extra: serde_json::Value) {
    println!(
        "{}",
        json!({"operation":operation,"side":side,"ns":samples,"peak_rss_bytes":rss(),"extra":extra})
    );
}
fn prepare(path: &Path, bytes: usize) -> io::Result<()> {
    if fs::metadata(path).is_ok_and(|m| m.len() == bytes as u64) {
        return Ok(());
    }
    let mut file = File::create(path)?;
    let block = vec![b'a'; 1024 * 1024];
    let mut left = bytes;
    while left > 0 {
        let n = left.min(block.len());
        file.write_all(&block[..n])?;
        left -= n;
    }
    file.sync_all()
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().collect();
    let operation = &args[1];
    let side = &args[2];
    if operation == "prepare" {
        prepare(Path::new(side), args[3].parse()?)?;
        return Ok(());
    }
    if operation == "load" || operation == "first" {
        let path = Path::new(&args[3]);
        let start = Instant::now();
        if side == "A" {
            let text = Text::from_reader(File::open(path)?)?;
            let elapsed = start.elapsed().as_nanos();
            record(
                operation,
                side,
                &[elapsed],
                json!({"bytes":text.len_bytes(),"snapshots":0,"load_peak_rss_bytes":rss(),"estimated_storage_bytes":text.estimated_storage_bytes()}),
            );
            black_box(text);
        } else {
            let task = load_utf8(path, LoadOptions::default())?;
            let mut snapshots = 0;
            let mut first = None;
            loop {
                if let Some(snapshot) = task.try_snapshot() {
                    snapshots += 1;
                    first.get_or_insert(start.elapsed().as_nanos());
                    if operation == "first" {
                        task.cancel();
                        record(
                            operation,
                            side,
                            &[first.unwrap()],
                            json!({"prefix_bytes":snapshot.text.len_bytes(),"total":task.total}),
                        );
                        let _ = task.result.recv_timeout(Duration::from_secs(30));
                        return Ok(());
                    }
                }
                match task.result.try_recv() {
                    Ok(loaded) => {
                        let loaded = loaded?;
                        let elapsed = start.elapsed().as_nanos();
                        record(
                            operation,
                            side,
                            &[elapsed],
                            json!({"first_ns":first,"snapshots":snapshots,"bytes":loaded.text.len_bytes(),"load_peak_rss_bytes":rss(),"estimated_storage_bytes":loaded.text.estimated_storage_bytes()}),
                        );
                        black_box(loaded);
                        return Ok(());
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => std::thread::yield_now(),
                    Err(e) => return Err(e.into()),
                }
            }
        }
        return Ok(());
    }
    let text = Text::from_reader(io::repeat(b'a').take(50_000_000))?;
    let mut index = if side == "B" {
        Some(LongLineIndex::new(text.clone(), 0, 4))
    } else {
        None
    };
    let mut samples = Vec::new();
    let count = if operation == "word" || operation == "window" {
        8
    } else {
        200
    };
    for i in 0..count {
        let at = 25_000_000 + i;
        if let Some(index) = &mut index {
            let snapshot = index.text().clone(); // retain COW input for typing, as in Buffer history
            let start = Instant::now();
            match operation.as_str() {
                "insert" => index.replace(at..at, "x"),
                "word" => {
                    black_box(index.next_word_boundary(at));
                }
                "end" => {
                    black_box(index.len_bytes());
                }
                "window" => {
                    black_box(index.visible_window(at, 200));
                }
                _ => panic!("unknown operation"),
            }
            samples.push(start.elapsed().as_nanos());
            if operation == "insert" {
                index.replace(at..at + 1, "");
            }
            black_box(snapshot);
        } else {
            let mut edited = text.clone();
            let start = Instant::now();
            match operation.as_str() {
                "insert" => edited.insert(at, "x"),
                "word" => {
                    black_box(
                        text.slice(at..text.len_bytes())
                            .chars()
                            .take_while(|c| c.is_alphanumeric() || *c == '_')
                            .count(),
                    );
                }
                "end" => {
                    black_box(text.line_len_bytes(0));
                }
                "window" => {
                    // Reference layout scans the prefix, retaining only 200 visible cells.
                    let mut column = 0;
                    let mut first = None;
                    for (byte, c) in text.chars().enumerate() {
                        if column >= at && first.is_none() {
                            first = Some(byte);
                        }
                        column += c.width().unwrap_or(0);
                        if column >= at + 200 {
                            black_box((first.unwrap(), byte + 1));
                            break;
                        }
                    }
                }
                _ => panic!("unknown operation"),
            }
            samples.push(start.elapsed().as_nanos());
            black_box(edited);
        }
    }
    record(
        operation,
        side,
        &samples,
        json!({"bytes":50_000_000,"index_included_in_typing":true}),
    );
    Ok(())
}
