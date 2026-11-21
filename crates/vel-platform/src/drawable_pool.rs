//! Drawable count hysteresis, independent of AppKit and the display timer.
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

    pub(crate) fn timer(&mut self, now: Instant) {
        if self.deadline().is_some_and(|due| now >= due) {
            *self = Self::Idle;
        }
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
        pool.timer(deadline - Duration::from_millis(1));
        assert_eq!(pool.limit(), 3);
        pool.timer(deadline);
        assert_eq!(pool.limit(), 2);
        assert_eq!(pool.deadline(), None);
    }

    #[test]
    fn solitary_frames_do_not_regrow_the_pool_but_a_burst_does() {
        let start = Instant::now();
        let mut pool = DrawablePool::default();
        pool.frame(start);
        let idle = start + QUIET;
        pool.timer(idle);
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
