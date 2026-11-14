//! CPU-only 100 MB streaming load measurement; file generation is outside timing.
use std::{io::Write, time::Instant};
fn main() {
    let mut generated = tempfile::NamedTempFile::new().expect("temp file");
    let supplied = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let path = if let Some(path) = supplied {
        path
    } else {
        let block = "0123456789 Velocity streaming load benchmark\n".repeat(1500);
        let mut remaining = 100_000_000;
        while remaining > 0 {
            let count = remaining.min(block.len());
            generated
                .write_all(&block.as_bytes()[..count])
                .expect("generate");
            remaining -= count;
        }
        generated.flush().expect("flush");
        generated.path().to_owned()
    };
    let started = Instant::now();
    let task = vel_fileio::load_background(path, "windows-1252".into()).expect("spawn");
    let queued_ms = started.elapsed().as_secs_f64() * 1000.;
    let loaded = task.result.recv().expect("worker").expect("load");
    println!(
        "{{\"kind\":\"fileio_cpu\",\"source_bytes\":{},\"bytes\":{},\"lines\":{},\"queued_ms\":{queued_ms},\"fully_loaded_ms\":{},\"gui_started\":false,\"gpu_started\":false}}",
        loaded.source_bytes,
        loaded.text.len_bytes(),
        loaded.text.len_lines(),
        started.elapsed().as_secs_f64() * 1000.
    );
}
