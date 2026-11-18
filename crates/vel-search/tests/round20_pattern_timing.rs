use std::{
    hint::black_box,
    path::{Path, PathBuf},
    time::Instant,
};
use vel_search::project::Rules;

#[test]
#[ignore = "paired 100k-path filter timing; use the same fixture on both revisions"]
fn round20_pattern_timing() {
    let root = Path::new("/workspace/packages/app");
    let paths: Vec<PathBuf> = (0..100_000)
        .map(|i| {
            root.join(format!(
                "s{}/test/data/work/feature{}/src/impl/module{i}.rs",
                i / 1000,
                i / 10
            ))
        })
        .collect();
    let patterns = [
        "*/node_modules",
        "output/artifacts",
        "/workspace/assets/build",
        "*/vendor/package*",
        "cache/generated",
        "/workspace/profiles/?.tmp",
    ]
    .map(str::to_owned);
    for (label, folders) in [
        ("defaults", &[".git".to_owned(), ".Trash-*".to_owned()][..]),
        ("3 path patterns", &patterns[..3]),
        ("6 path patterns", &patterns[..]),
    ] {
        let rules = Rules::new(
            &["*.pyc".into(), "*.sublime-workspace".into()],
            folders,
            &["*.png".into(), "*.pdf".into()],
        )
        .unwrap();
        let mut samples = [0.; 9];
        for sample in &mut samples {
            let start = Instant::now();
            let kept = paths
                .iter()
                .filter(|path| black_box(&rules).allows(root, path, false, true))
                .count();
            *sample = start.elapsed().as_secs_f64();
            assert_eq!(kept, paths.len());
        }
        samples.sort_by(f64::total_cmp);
        eprintln!(
            "{label}: {:0.4} s median ({:.4} min, {:.4} max)",
            samples[4], samples[0], samples[8]
        );
    }
}
