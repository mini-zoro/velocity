//! Device-free full-screen transition selection shared with the native window.
/// Action chosen using known current state, before any AppKit mutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transition {
    /// Restore the saved frame/style, even if the preference changed meanwhile.
    RestoreSimple,
    /// Save geometry and fill the current screen without creating a Space.
    EnterSimple,
    /// Let AppKit enter/exit its ordinary full-screen Space.
    ToggleNative,
}
/// Existing simple state takes precedence; native full screen always exits natively.
pub fn transition(simple_requested: bool, native_active: bool, simple_saved: bool) -> Transition {
    if simple_saved {
        Transition::RestoreSimple
    } else if simple_requested && !native_active {
        Transition::EnterSimple
    } else {
        Transition::ToggleNative
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preference_changes_and_distraction_entry_share_the_fullscreen_transition_table() {
        for requested in [false, true] {
            for native in [false, true] {
                for saved in [false, true] {
                    let expected = if saved {
                        Transition::RestoreSimple
                    } else if requested && !native {
                        Transition::EnterSimple
                    } else {
                        Transition::ToggleNative
                    };
                    assert_eq!(transition(requested, native, saved), expected);
                }
            }
        }
        // Entering distraction free from native full screen must use AppKit to exit;
        // leaving a simple window restores its saved geometry even after a preference reload.
        assert_eq!(transition(true, true, false), Transition::ToggleNative);
        assert_eq!(transition(false, false, true), Transition::RestoreSimple);
    }
}
