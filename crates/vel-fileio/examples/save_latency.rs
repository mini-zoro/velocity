//! Overwrite benchmarks on the caller's temporary filesystem. Setup is not timed.
use std::{env, time::Instant};
use vel_fileio::{Format, save_with_identity};
use vel_text::Text;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bytes: usize = env::args().nth(1).ok_or("specify a byte count")?.parse()?;
    let count: usize = env::args().nth(2).unwrap_or_else(|| "5".into()).parse()?;
    let atomic = env::args().nth(3).as_deref() == Some("atomic");
    let folder = tempfile::tempdir()?;
    let path = folder.path().join("sample.txt");
    let original = Text::from("x".repeat(bytes).as_str());
    let revised = match env::args().nth(4).as_deref() {
        Some("rewrite") => Text::from("y".repeat(bytes).as_str()),
        Some("edit") if bytes > 0 => {
            let mut revised = original.clone();
            revised.replace(bytes / 2..bytes / 2 + 1, "y");
            revised
        }
        _ => original.clone(),
    };
    let format = Format::default();
    save_with_identity(&path, &original, &format, false)?;
    for i in 0..count {
        let text = if i % 2 == 0 { &revised } else { &original };
        let start = Instant::now();
        std::hint::black_box(save_with_identity(&path, text, &format, atomic)?);
        println!("{bytes},{}", start.elapsed().as_secs_f64() * 1000.);
    }
    Ok(())
}
