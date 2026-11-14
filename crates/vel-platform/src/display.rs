//! Display/session preflight without constructing AppKit or Metal objects.

/// Reasons why an actual scanout cannot be measured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DisplayStatus {
    /// The hosting display is asleep.
    pub display_asleep: bool,
    /// WindowServer reports a locked login session.
    pub screen_locked: bool,
    /// No current console session is available.
    pub session_unavailable: bool,
    /// AppKit reports that the window is not visible.
    pub window_occluded: bool,
}
impl DisplayStatus {
    /// Returns an explicit preflight failure, with all observed reasons.
    pub fn check(self) -> Result<(), String> {
        let mut reasons = Vec::new();
        if self.display_asleep {
            reasons.push("display asleep");
        }
        if self.screen_locked {
            reasons.push("screen locked");
        }
        if self.session_unavailable {
            reasons.push("console session unavailable");
        }
        if self.window_occluded {
            reasons.push("window occluded");
        }
        if reasons.is_empty() {
            Ok(())
        } else {
            Err(format!("display preflight failed: {}", reasons.join(", ")))
        }
    }
}
/// Checks the primary display and console session without opening a window.
pub fn display_status() -> DisplayStatus {
    status_for_display(None)
}

#[cfg(target_os = "macos")]
pub(crate) fn status_for_display(display: Option<u32>) -> DisplayStatus {
    use objc2::{msg_send, runtime::AnyObject};
    use objc2_core_graphics::{CGDisplayIsAsleep, CGMainDisplayID, CGSessionCopyCurrentDictionary};
    use objc2_foundation::NSString;
    let session = CGSessionCopyCurrentDictionary();
    let locked = session.as_ref().is_some_and(|dictionary| {
        let key = NSString::from_str("CGSSessionScreenIsLocked");
        // SAFETY: CFDictionary is toll-free bridged to NSDictionary. The returned
        // NSNumber is borrowed only while the owning session dictionary is alive.
        unsafe {
            let object = (&**dictionary as *const _ as *const AnyObject)
                .as_ref()
                .expect("dictionary");
            let value: *mut AnyObject = msg_send![object, objectForKey: &*key];
            !value.is_null() && msg_send![value, boolValue]
        }
    });
    DisplayStatus {
        display_asleep: CGDisplayIsAsleep(display.unwrap_or_else(|| CGMainDisplayID())),
        screen_locked: locked,
        session_unavailable: session.is_none(),
        window_occluded: false,
    }
}
#[cfg(not(target_os = "macos"))]
pub(crate) fn status_for_display(_: Option<u32>) -> DisplayStatus {
    DisplayStatus::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failure_names_the_observed_blockers() {
        assert!(DisplayStatus::default().check().is_ok());
        let error = DisplayStatus {
            display_asleep: true,
            screen_locked: true,
            window_occluded: true,
            ..Default::default()
        }
        .check()
        .unwrap_err();
        for reason in ["display asleep", "screen locked", "window occluded"] {
            assert!(error.contains(reason));
        }
    }
}
