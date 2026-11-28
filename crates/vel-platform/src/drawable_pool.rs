//! Drawable count hysteresis, independent of AppKit and the display timer.
//!
//! Lowering CAMetalLayer's `maximumDrawableCount` does not free a drawable that is
//! already allocated (runner measurement, 2026-11-28: three window-sized surfaces
//! stayed for as long as the window was idle, and later frames at the lower cap freed
//! the third only sometimes). So when the cap drops the host flushes the layer's free
//! drawables and draws one frame, which leaves exactly two.
use std::time::{Duration, Instant};

const QUIET: Duration = Duration::from_millis(350);
const BURST: Duration = Duration::from_millis(120);

#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum DrawablePool {
    #[default]
    Startup,
    Active(Instant),
    Idle,
    Single(Instant),
}

impl DrawablePool {
    pub(crate) fn limit(self) -> usize {
        match self {
            Self::Startup | Self::Active(_) => 3,
            Self::Idle | Self::Single(_) => 2,
        }
    }

    pub(crate) fn frame(&mut self, now: Instant) {
        *self = match *self {
            Self::Startup | Self::Active(_) => Self::Active(now),
            Self::Single(last) if now.duration_since(last) <= BURST => Self::Active(now),
            _ => Self::Single(now),
        };
    }

    pub(crate) fn deadline(self) -> Option<Instant> {
        match self {
            Self::Active(last) => Some(last + QUIET),
            _ => None,
        }
    }

    /// Settles an expired burst. Returns true when the cap dropped, i.e. when the
    /// layer may still hold a surplus drawable that only a frame at the new cap frees.
    pub(crate) fn timer(&mut self, now: Instant) -> bool {
        if self.deadline().is_some_and(|due| now >= due) {
            let before = self.limit();
            *self = Self::Idle;
            return self.limit() < before;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sustained_fifo_uses_three_and_stale_timer_does_not_shrink() {
        let start = Instant::now();
        let mut pool = DrawablePool::default();
        assert_eq!(pool.limit(), 3);
        assert_eq!(pool.deadline(), None);
        for i in 0..600 {
            let now = start + Duration::from_millis(i * 16);
            pool.frame(now);
            pool.timer(now);
            assert_eq!(pool.limit(), 3);
            assert_eq!(pool.deadline(), Some(now + QUIET));
        }
        let deadline = pool.deadline().unwrap();
        assert!(!pool.timer(deadline - Duration::from_millis(1)));
        assert_eq!(pool.limit(), 3);
        // The cap drops: the host flushes the layer's free drawables and redraws once.
        assert!(pool.timer(deadline));
        assert_eq!(pool.limit(), 2);
        assert_eq!(pool.deadline(), None);
        assert!(!pool.timer(deadline + QUIET));
    }

    #[test]
    fn solitary_frames_do_not_regrow_the_pool_but_a_burst_does() {
        let start = Instant::now();
        let mut pool = DrawablePool::default();
        pool.frame(start);
        let idle = start + QUIET;
        assert!(pool.timer(idle));
        pool.frame(idle);
        assert_eq!(pool.limit(), 2);
        let later = idle + BURST + Duration::from_millis(1);
        pool.frame(later);
        assert_eq!(pool.limit(), 2);
        assert_eq!(pool.deadline(), None);
        pool.frame(later + BURST);
        assert_eq!(pool.limit(), 3);
        assert_eq!(pool.deadline(), Some(later + BURST + QUIET));
    }
}
