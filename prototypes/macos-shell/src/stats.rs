//! Deterministic statistics and benchmark scheduling, with no clocks or devices.
use serde::Serialize;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Stats {
    pub count: usize,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    pub mean: f64,
}
/// Linear interpolation between sorted samples, matching Python's helper script.
pub fn summarize(samples: &[f64]) -> Option<Stats> {
    if samples.is_empty() || samples.iter().any(|s| !s.is_finite() || *s < 0.0) {
        return None;
    }
    let mut v = samples.to_vec();
    v.sort_by(f64::total_cmp);
    let q = |p: f64| {
        let rank = (v.len() - 1) as f64 * p;
        let lo = rank.floor() as usize;
        let hi = rank.ceil() as usize;
        v[lo] + (v[hi] - v[lo]) * (rank - lo as f64)
    };
    Some(Stats {
        count: v.len(),
        p50: q(0.50),
        p95: q(0.95),
        p99: q(0.99),
        max: v[v.len() - 1],
        mean: v.iter().sum::<f64>() / v.len() as f64,
    })
}

#[derive(Clone, Debug)]
pub struct TypingSchedule {
    state: u64,
}
impl TypingSchedule {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }
    /// SplitMix64; inclusive randomized interval 30..=90 ms, never zero-seed-stuck.
    pub fn next_delay_ms(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        30 + ((z ^ (z >> 31)) % 61)
    }
}
/// Visit every viewport, including the tail; advancing one screen keeps the sweep bounded.
pub fn sweep_positions(lines: usize, visible_rows: usize) -> Vec<usize> {
    let step = visible_rows.max(1);
    (0..lines.max(1)).step_by(step).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_quantiles_and_invalid_samples() {
        let s = summarize(&[4., 1., 2., 3.]).unwrap();
        assert_eq!(s.p50, 2.5);
        assert!((s.p95 - 3.85).abs() < 1e-9);
        assert_eq!(s.max, 4.);
        assert_eq!(s.mean, 2.5);
        assert!(summarize(&[]).is_none());
        assert!(summarize(&[f64::NAN]).is_none());
        assert!(summarize(&[-1.]).is_none());
        assert_eq!(summarize(&[2.]).unwrap().p99, 2.);
    }
    #[test]
    fn reproducible_jitter_has_both_endpoints() {
        let mut a = TypingSchedule::new(0);
        let mut b = TypingSchedule::new(0);
        let delays: Vec<_> = (0..500)
            .map(|_| {
                let x = a.next_delay_ms();
                assert_eq!(x, b.next_delay_ms());
                x
            })
            .collect();
        assert_eq!(delays.iter().min(), Some(&30));
        assert_eq!(delays.iter().max(), Some(&90));
    }
    #[test]
    fn sweep_covers_all_lines_including_partial_screen() {
        for n in [0, 1, 61, 5000] {
            for rows in [0, 1, 60, 100] {
                let positions = sweep_positions(n, rows);
                assert_eq!(positions[0], 0);
                for line in 0..n {
                    assert!(
                        positions
                            .iter()
                            .any(|p| *p <= line && line < *p + rows.max(1))
                    );
                }
            }
        }
    }
}
