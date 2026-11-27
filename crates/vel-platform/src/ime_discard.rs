//! Ending a native input-method conversion, independent of AppKit.
//!
//! When the application throws a preedit away (focus moved to another tab,
//! window or panel), the system input method must be told as well, or its
//! candidate window outlives the conversion. An input method handling that
//! request asks the client whether marked text still exists and clears it
//! itself before closing its session; a client that already answers "no
//! marked text" leaves it nothing to end. So the request is sent while the
//! snapshot the input method queries still shows the preedit, and the
//! application's new snapshot is published only afterwards. Text the input
//! method sends back for that dead conversion is not applied: the
//! application has already settled the preedit.
use std::cell::{Cell, RefCell};

use crate::input::TextInputState;

/// A requested end of the native conversion and the callback window around it.
#[derive(Debug, Default)]
pub(crate) struct NativeDiscard {
    pending: Cell<bool>,
    running: Cell<bool>,
}

impl NativeDiscard {
    /// Asks for the native conversion to end at the next settle point.
    pub(crate) fn request(&self) {
        self.pending.set(true);
    }

    /// Whether input-method text belongs to a conversion being thrown away.
    pub(crate) fn blocks_text(&self) -> bool {
        self.pending.get() || self.running.get()
    }

    /// Whether the native request is executing, so callbacks are reentrant.
    pub(crate) fn running(&self) -> bool {
        self.running.get()
    }

    /// Runs `end` once for a pending request, with callbacks blocked throughout.
    pub(crate) fn flush(&self, end: impl FnOnce()) {
        if self.pending.replace(false) {
            self.running.set(true);
            end();
            self.running.set(false);
        }
    }

    /// Ends a pending conversion while `published` still shows its preedit,
    /// then publishes `next`. Returns the snapshot `next` replaced.
    pub(crate) fn settle(
        &self,
        published: &RefCell<TextInputState>,
        next: TextInputState,
        end: impl FnOnce(),
    ) -> TextInputState {
        self.flush(end);
        published.replace(next)
    }

    /// Ends a closing window's conversion before forgetting what it published.
    pub(crate) fn retire(&self, published: &RefCell<TextInputState>, end: impl FnOnce()) {
        if published.borrow().marked.is_some() {
            self.request();
        }
        self.flush(end);
        published.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Utf16Range;
    use vel_text::Text;

    fn composing() -> TextInputState {
        TextInputState {
            text: Text::from("ab zhongwen"),
            selected: Utf16Range::new(11, 0),
            marked: Some(Utf16Range::new(3, 8)),
            ..TextInputState::default()
        }
    }

    fn other_tab() -> TextInputState {
        TextInputState {
            text: Text::from("second"),
            selected: Utf16Range::new(6, 0),
            ..TextInputState::default()
        }
    }

    #[test]
    fn input_method_sees_the_discarded_preedit_until_the_request_returns() {
        let discard = NativeDiscard::default();
        let published = RefCell::new(composing());
        discard.request();
        let mut seen = None;
        let previous = discard.settle(&published, other_tab(), || {
            let state = published.borrow();
            seen = Some((state.marked, state.text.to_string(), discard.blocks_text()));
        });
        // hasMarkedText / markedRange answered during discardMarkedText.
        assert_eq!(
            seen,
            Some((Some(Utf16Range::new(3, 8)), "ab zhongwen".into(), true))
        );
        assert_eq!(previous.marked, Some(Utf16Range::new(3, 8)));
        assert_eq!(published.borrow().marked, None);
        assert_eq!(published.borrow().text.to_string(), "second");
        assert!(!discard.blocks_text());
        assert!(!discard.running());
    }

    #[test]
    fn settling_without_a_request_only_publishes() {
        let discard = NativeDiscard::default();
        let published = RefCell::new(composing());
        let mut ended = false;
        let previous = discard.settle(&published, other_tab(), || ended = true);
        assert!(!ended);
        assert_eq!(previous.marked, Some(Utf16Range::new(3, 8)));
        assert_eq!(published.borrow().text.to_string(), "second");
    }

    #[test]
    fn text_is_blocked_from_request_until_the_native_request_returns() {
        let discard = NativeDiscard::default();
        assert!(!discard.blocks_text());
        discard.request();
        assert!(discard.blocks_text());
        assert!(!discard.running());
        let mut inside = None;
        discard.flush(|| inside = Some((discard.blocks_text(), discard.running())));
        assert_eq!(inside, Some((true, true)));
        assert!(!discard.blocks_text());
        let mut again = false;
        discard.flush(|| again = true);
        assert!(!again, "one request ends the conversion once");
    }

    #[test]
    fn closing_window_ends_a_visible_preedit_before_forgetting_it() {
        let discard = NativeDiscard::default();
        let published = RefCell::new(composing());
        let mut seen = None;
        discard.retire(&published, || seen = Some(published.borrow().marked));
        assert_eq!(seen, Some(Some(Utf16Range::new(3, 8))));
        assert_eq!(published.borrow().marked, None);
        assert_eq!(published.borrow().text.to_string(), "");

        let published = RefCell::new(other_tab());
        let mut ended = false;
        discard.retire(&published, || ended = true);
        assert!(!ended, "no preedit and no request: nothing to end");

        let published = RefCell::new(other_tab());
        discard.request();
        let mut ended = false;
        discard.retire(&published, || ended = true);
        assert!(ended, "an earlier request is still honoured");
    }
}
