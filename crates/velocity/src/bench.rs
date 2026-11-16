//! Headless benchmark transitions: bounded frame counts and timer-driven typing.
use crate::{cli::Mode, stats::TypingSchedule};
use std::time::{Duration, Instant};

pub const SCROLL_FRAMES: usize = 600;
pub const TYPING_INSERTIONS: usize = 500;
pub const PANEL_ACTIONS: usize = 100;
pub const WARMUP_FRAMES: usize = 60;
#[derive(Debug)]
pub struct Bench {
    pub mode: Mode,
    pub warmup: usize,
    pub completed: usize,
    pub next: Option<Instant>,
    pub due: Option<Instant>,
    pub positions: Vec<usize>,
    pub pending: bool,
    schedule: TypingSchedule,
}
impl Bench {
    pub fn new(mode: Mode, seed: u64, positions: Vec<usize>) -> Self {
        Self {
            mode,
            warmup: if mode == Mode::Scroll {
                WARMUP_FRAMES
            } else {
                0
            },
            completed: 0,
            next: None,
            due: None,
            positions,
            pending: false,
            schedule: TypingSchedule::new(seed),
        }
    }
    pub fn schedule_next(&mut self, now: Instant) -> Instant {
        let at = now + Duration::from_millis(self.schedule.next_delay_ms());
        self.next = Some(at);
        at
    }
    pub fn typing_due(&mut self, now: Instant) -> bool {
        if self.pending || !self.next.is_some_and(|at| now >= at) {
            return false;
        }
        self.due = self.next.take();
        self.pending = true;
        true
    }
    pub fn submitted(&mut self) {
        self.pending = false;
        if self.warmup > 0 {
            self.warmup -= 1;
        } else {
            self.completed += 1;
        }
    }
    pub fn finished(&self) -> bool {
        self.warmup == 0
            && self.completed
                >= match self.mode {
                    Mode::Scroll => SCROLL_FRAMES,
                    Mode::Typing => TYPING_INSERTIONS,
                    Mode::Panels | Mode::ThemeUi | Mode::LargeFiles => PANEL_ACTIONS,
                    Mode::Memory => self.positions.len(),
                    _ => 0,
                }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typing_deadlines_are_one_shot_and_retry_does_not_duplicate_input() {
        let now = Instant::now();
        let mut b = Bench::new(Mode::Typing, 0, vec![]);
        let at = b.schedule_next(now);
        assert!(!b.typing_due(now));
        assert!(b.typing_due(at));
        assert!(!b.typing_due(at + Duration::from_secs(1)));
        assert_eq!(b.completed, 0);
        b.submitted();
        assert_eq!(b.completed, 1);
        assert!(!b.typing_due(at));
    }
    #[test]
    fn scroll_warmup_and_memory_coverage_have_exact_completion() {
        let mut b = Bench::new(Mode::Scroll, 0, vec![]);
        for _ in 0..WARMUP_FRAMES + SCROLL_FRAMES - 1 {
            assert!(!b.finished());
            b.submitted();
        }
        assert!(!b.finished());
        b.submitted();
        assert!(b.finished());
        assert_eq!(b.completed, 600);
        let mut memory = Bench::new(Mode::Memory, 0, vec![0, 40, 80]);
        for _ in 0..3 {
            memory.submitted();
        }
        assert!(memory.finished());
    }
}
