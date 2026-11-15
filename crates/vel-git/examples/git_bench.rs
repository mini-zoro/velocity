//! Release status timing on an existing isolated fixture. Modifies only the selected fixture file.
use std::{
    hint::black_box,
    path::{Path, PathBuf},
    time::Instant,
};
use vel_git::{Badge, Repository};
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let root = PathBuf::from(&args[1]);
    let path = Path::new(&args[2]);
    let start = Instant::now();
    let mut repo = Repository::discover(&root).unwrap();
    black_box(repo.refresh().unwrap());
    let cold_ns = start.elapsed().as_nanos();
    let original = std::fs::read(root.join(path)).unwrap();
    let mut bytes = original.clone();
    bytes.extend_from_slice(b"\nmodified for status benchmark\n");
    std::fs::write(root.join(path), bytes).unwrap();
    let start = Instant::now();
    black_box(repo.refresh_paths(&[path.into()]).unwrap());
    let warm_ns = start.elapsed().as_nanos();
    assert_eq!(repo.snapshot().unwrap().badge(path), Some(Badge::Modified));
    std::fs::write(root.join(path), original).unwrap();
    println!("{{\"cold_ns\":{cold_ns},\"warm_ns\":{warm_ns}}}");
}
