//! Device-free decisions for a click that `acceptsFirstMouse:` lets through.
//!
//! AppKit documents a mouse-down in a window that is not key as bringing the window
//! forward and making it key; `acceptsFirstMouse:` only decides whether that same click
//! also reaches `mouseDown:`:
//! <https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/EventOverview/HandlingMouseEvents/HandlingMouseEvents.html>
//! <https://developer.apple.com/documentation/appkit/nsview/acceptsfirstmouse(for:)>
//! AppKit asks every mouse-down whether to delay window ordering, and otherwise "normal ordering
//! and activation occur"; even an accessory-policy app "may be activated … by clicking on one
//! of its windows":
//! <https://developer.apple.com/documentation/appkit/nsview/shoulddelaywindowordering(for:)>
//! <https://developer.apple.com/documentation/appkit/nsapplication/activationpolicy-swift.enum/accessory>
//! That activation is only a request which "doesn't guarantee app activation" and may lag:
//! <https://developer.apple.com/documentation/appkit/nsapplication/activate()>
//! GUI runs saw an accepted tab click switch tabs while the clicked app stayed behind, so the
//! view finishes the activation itself, exactly once, for the event it told AppKit to deliver.
use crate::{AppHandler, Point};

/// The `acceptsFirstMouse:` answer: the app's global opt-in, or the chrome under the click.
pub fn accepts(app: &dyn AppHandler, point: Option<Point>) -> bool {
    app.accepts_first_mouse() || point.is_some_and(|p| app.accepts_first_mouse_at(p))
}

/// A mouse-down as the view receives it, with the application state at that moment.
#[derive(Clone, Copy, Debug)]
pub struct Down {
    pub timestamp: f64,
    /// Only the primary button activates; AppKit does not even deliver a right mouse-down of
    /// an inactive app to the window:
    /// <https://developer.apple.com/documentation/appkit/nswindow/sendevent(_:)>
    pub left: bool,
    /// Command-click is the macOS gesture for acting in a background window without raising
    /// it, so that click stays entirely AppKit's decision.
    pub command: bool,
    pub app_active: bool,
}

/// The mouse-down that `acceptsFirstMouse:` last answered yes to, by event timestamp.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FirstMouse(Option<f64>);

impl FirstMouse {
    /// Records `acceptsFirstMouse:`'s answer for the mouse-down stamped `timestamp`.
    pub fn answered(&mut self, accepted: bool, timestamp: f64) {
        self.0 = accepted.then_some(timestamp);
    }
    /// Consumes the answer. True only for that same left click while another app is active:
    /// an already active Velocity keeps AppKit's own key-window change, and any later
    /// mouse-down (double-click, drag, an unrelated click) never requests activation again.
    pub fn take(&mut self, down: Down) -> bool {
        self.0.take() == Some(down.timestamp) && down.left && !down.command && !down.app_active
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FrameContext;

    /// A window whose top 30 points are a tab strip, optionally with `click_through`.
    struct Tabs(bool);
    impl AppHandler for Tabs {
        fn accepts_first_mouse(&self) -> bool {
            self.0
        }
        fn accepts_first_mouse_at(&self, point: Point) -> bool {
            point.y < 30.
        }
        fn on_frame(&mut self, _: &mut FrameContext<'_>) -> Result<(), String> {
            Ok(())
        }
    }
    const TAB: Point = Point { x: 40., y: 12. };
    const TEXT: Point = Point { x: 40., y: 200. };
    fn down(timestamp: f64) -> Down {
        Down {
            timestamp,
            left: true,
            command: false,
            app_active: false,
        }
    }
    fn click(app: &dyn AppHandler, point: Point, down: Down) -> bool {
        let mut first = FirstMouse::default();
        first.answered(accepts(app, Some(point)), down.timestamp);
        first.take(down)
    }

    #[test]
    fn a_tab_click_in_an_inactive_app_requests_activation_once_and_text_does_not() {
        let tabs = Tabs(false);
        assert!(click(&tabs, TAB, down(5.)));
        // The text only activates through AppKit, so the view never sees that click.
        assert!(!click(&tabs, TEXT, down(5.)));
        // `click_through` opts the whole view in, including its activation.
        assert!(click(&Tabs(true), TEXT, down(5.)));
        // Without an event AppKit gets only the global answer.
        assert!(!accepts(&tabs, None) && accepts(&Tabs(true), None));

        let mut first = FirstMouse::default();
        first.answered(true, 5.);
        assert!(first.take(down(5.)));
        // The second click of a double-click, or the same event seen again, does not re-request.
        assert!(!first.take(down(5.)));
        first.answered(true, 5.);
        assert!(
            !first.take(down(6.)),
            "a stale answer belongs to another event"
        );
        assert_eq!(first, FirstMouse::default(), "and is discarded by it");
    }

    #[test]
    fn active_app_right_button_and_command_click_keep_appkit_behavior() {
        let tabs = Tabs(false);
        for down in [
            Down {
                app_active: true,
                ..down(1.)
            },
            Down {
                left: false,
                ..down(1.)
            },
            Down {
                command: true,
                ..down(1.)
            },
        ] {
            assert!(!click(&tabs, TAB, down), "{down:?}");
        }
        let mut first = FirstMouse::default();
        first.answered(false, 1.);
        assert!(!first.take(down(1.)), "a refused click is AppKit's alone");
    }
}
