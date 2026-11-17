//! System mouse preferences, captured outside the pointer hot path.
use std::time::Duration;
/// Preferences whose effects can also be tested without AppKit.
#[derive(Clone, Copy, Debug, Default)]
pub struct MousePreferences {
    /// True jumps the scroller to the clicked position; false pages toward it.
    pub jump_to_click: bool,
    /// Explicit user text-drag delay. Unset retains the editor's distance threshold.
    pub text_drag_delay: Option<Duration>,
}
impl MousePreferences {
    /// Reads macOS defaults without creating a window.
    /// <https://www.sublimetext.com/download> (builds 4151 and 4121).
    pub fn read() -> Self {
        #[cfg(target_os = "macos")]
        {
            use objc2_foundation::{NSString, NSUserDefaults};
            let defaults = NSUserDefaults::standardUserDefaults();
            let delay = NSString::from_str("NSDragAndDropTextDelay");
            let milliseconds = defaults
                .objectForKey(&delay)
                .map(|_| defaults.doubleForKey(&delay));
            Self {
                jump_to_click: defaults
                    .boolForKey(&NSString::from_str("AppleScrollerPagingBehavior")),
                text_drag_delay: milliseconds.and_then(Self::text_delay),
            }
        }
        #[cfg(not(target_os = "macos"))]
        Self::default()
    }
    /// Converts the native preference's milliseconds to a gesture threshold.
    /// Apple Xcode User Default Reference (NSDragAndDropTextDelay):
    /// <https://leopard-adc.pepas.com/documentation/DeveloperTools/Reference/XcodeUserDefaultRef/100-Xcode_User_Defaults/UserDefaultRef.html>
    /// Zero disables the delay in Sublime (negative disables dragging):
    /// <https://forum.sublimetext.com/t/nsdraganddroptextdelay/61519/2>
    pub fn text_delay(milliseconds: f64) -> Option<Duration> {
        milliseconds.is_finite().then(|| {
            if milliseconds < 0. {
                Duration::MAX
            } else {
                Duration::from_secs_f64(milliseconds.min(3_600_000.) / 1000.)
            }
        })
    }
    /// Option temporarily reverses the macOS paging preference.
    pub fn jumps(self, option: bool) -> bool {
        self.jump_to_click ^ option
    }
    /// Applies only a configured time threshold; the existing distance rule still applies.
    pub fn drag_ready(self, held: Duration) -> bool {
        self.text_drag_delay.is_none_or(|delay| held >= delay)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn option_inverts_paging_and_drag_delay_has_an_exact_boundary() {
        for jump_to_click in [false, true] {
            let prefs = MousePreferences {
                jump_to_click,
                text_drag_delay: Some(Duration::from_millis(250)),
            };
            assert_eq!(prefs.jumps(false), jump_to_click);
            assert_eq!(prefs.jumps(true), !jump_to_click);
            assert!(!prefs.drag_ready(Duration::from_millis(249)));
            assert!(prefs.drag_ready(Duration::from_millis(250)));
        }
        assert!(MousePreferences::default().drag_ready(Duration::ZERO));
        assert_eq!(
            MousePreferences::text_delay(300.),
            Some(Duration::from_millis(300))
        );
        assert_eq!(MousePreferences::text_delay(0.), Some(Duration::ZERO));
        assert_eq!(MousePreferences::text_delay(-1.), Some(Duration::MAX));
        assert_eq!(MousePreferences::text_delay(f64::NAN), None);
    }
}
