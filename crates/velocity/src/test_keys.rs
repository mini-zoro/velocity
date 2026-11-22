//! Physical chords for tests that drive the active platform's resource keymap.
use vel_config::keymap::Key;

pub(crate) const fn chord(macos: &'static str, windows: &'static str) -> &'static str {
    if cfg!(windows) { windows } else { macos }
}

pub(crate) fn parse(chord: &str) -> Key {
    if cfg!(windows) {
        Key::parse_windows(chord).unwrap()
    } else {
        Key::parse(chord).unwrap()
    }
}
