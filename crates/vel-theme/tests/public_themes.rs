//! Public, permissively licensed fixtures are fetched into memory at test time, never vendored.
use std::{process::Command, time::Instant};
use vel_theme::{Palette, Theme};
#[test]
fn public_theme_parse_compatibility() {
    if std::env::var_os("VEL_OFFLINE").is_some() {
        eprintln!("public themes: skipped (VEL_OFFLINE)");
        return;
    }
    let palette = Palette::default();
    let mut fetched = 0;
    let mut errors = Vec::new();
    for (name, url) in [
        (
            "ayu Light (MIT)",
            "https://raw.githubusercontent.com/dempfi/ayu/master/ayu-light.sublime-theme",
        ),
        (
            "Spacegray (MIT)",
            "https://raw.githubusercontent.com/SublimeText/Spacegray/main/Spacegray.sublime-theme",
        ),
        (
            "ayu (MIT)",
            "https://raw.githubusercontent.com/dempfi/ayu/master/ayu-dark.sublime-theme",
        ),
    ] {
        let response = Command::new("curl")
            .args([
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                "--connect-timeout",
                "5",
                "--max-time",
                "15",
                url,
            ])
            .output();
        let Ok(response) = response else {
            eprintln!("{name}: skipped (curl unavailable)");
            continue;
        };
        if !response.status.success() {
            eprintln!(
                "{name}: skipped (offline/unavailable: {})",
                String::from_utf8_lossy(&response.stderr)
            );
            continue;
        }
        let source = String::from_utf8(response.stdout).expect("UTF-8 theme");
        if let Err(e) = Theme::parse(&source, &palette) {
            errors.push(format!("{name}: {e}"));
            continue;
        }
        let mut samples = Vec::new();
        for _ in 0..7 {
            let start = Instant::now();
            let theme = Theme::parse(&source, &palette).unwrap_or_else(|e| panic!("{name}: {e}"));
            samples.push(start.elapsed().as_secs_f64() * 1000.);
            assert!(theme.rule_count() > 50, "unexpected fixture: {name}");
        }
        let theme = Theme::parse(&source, &palette).unwrap();
        eprintln!(
            "{name}: {} bytes, {} rules, {} extension diagnostics; compile_ms={samples:?}",
            source.len(),
            theme.rule_count(),
            theme.diagnostics.len()
        );
        fetched += 1;
    }
    eprintln!("public theme fixtures fetched: {fetched}/3");
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}
