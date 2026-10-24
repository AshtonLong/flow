//! CSS timing functions and transitions, for the pill's entrance and fade and
//! the transcribing dots.

use std::time::Instant;

/// `cubic-bezier(x1, y1, x2, y2)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CubicBezier {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
}

impl CubicBezier {
    pub const fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        Self { x1, y1, x2, y2 }
    }

    /// `ease`
    pub const EASE: Self = Self::new(0.25, 0.1, 0.25, 1.0);
    /// `ease-in-out`
    pub const EASE_IN_OUT: Self = Self::new(0.42, 0.0, 0.58, 1.0);

    fn sample(a1: f64, a2: f64, t: f64) -> f64 {
        // B(t) for P0 = 0, P3 = 1.
        let u = 1.0 - t;
        3.0 * u * u * t * a1 + 3.0 * u * t * t * a2 + t * t * t
    }

    fn slope(a1: f64, a2: f64, t: f64) -> f64 {
        let u = 1.0 - t;
        3.0 * u * u * a1 + 6.0 * u * t * (a2 - a1) + 3.0 * t * t * (1.0 - a2)
    }

    /// The eased progress for input progress `x` in 0..1.
    pub fn ease(&self, x: f64) -> f64 {
        if x <= 0.0 {
            return 0.0;
        }
        if x >= 1.0 {
            return 1.0;
        }
        // Newton's method, falling back to bisection.
        let mut t = x;
        for _ in 0..8 {
            let err = Self::sample(self.x1, self.x2, t) - x;
            if err.abs() < 1e-7 {
                return Self::sample(self.y1, self.y2, t);
            }
            let d = Self::slope(self.x1, self.x2, t);
            if d.abs() < 1e-6 {
                break;
            }
            t -= err / d;
        }
        let (mut lo, mut hi) = (0.0, 1.0);
        t = x;
        for _ in 0..40 {
            let v = Self::sample(self.x1, self.x2, t);
            if (v - x).abs() < 1e-7 {
                break;
            }
            if v < x {
                lo = t;
            } else {
                hi = t;
            }
            t = (lo + hi) / 2.0;
        }
        Self::sample(self.y1, self.y2, t)
    }
}

/// A CSS transition of one number.
#[derive(Debug, Clone, Copy)]
pub struct Transition {
    from: f64,
    to: f64,
    start: Instant,
    duration: f64,
    easing: CubicBezier,
}

impl Transition {
    /// Already at `value`.
    pub fn settled(value: f64, easing: CubicBezier, now: Instant) -> Self {
        Self { from: value, to: value, start: now, duration: 0.0, easing }
    }

    pub fn value(&self, now: Instant) -> f64 {
        if self.duration <= 0.0 {
            return self.to;
        }
        let x = now.saturating_duration_since(self.start).as_secs_f64() / self.duration;
        self.from + (self.to - self.from) * self.easing.ease(x)
    }

    pub fn target(&self) -> f64 {
        self.to
    }

    pub fn running(&self, now: Instant) -> bool {
        self.duration > 0.0 && now.saturating_duration_since(self.start).as_secs_f64() < self.duration
    }

    /// Starts towards `to` from wherever the value is now. Reversing a running
    /// transition takes only as long as the part already played (the CSS
    /// "reversing shortening factor").
    pub fn retarget(&mut self, to: f64, duration: f64, now: Instant) {
        if to == self.to {
            return;
        }
        let current = self.value(now);
        let mut duration = duration;
        if self.running(now) && to == self.from && self.to != self.from {
            let done = ((current - self.from) / (self.to - self.from)).clamp(0.0, 1.0);
            duration *= done;
        }
        *self = Self { from: current, to, start: now, duration, easing: self.easing };
    }

    /// Jumps to `value` (transitions off, or the window was hidden).
    pub fn snap(&mut self, value: f64, now: Instant) {
        *self = Self::settled(value, self.easing, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn bezier_matches_known_points() {
        assert_eq!(CubicBezier::EASE.ease(0.0), 0.0);
        assert_eq!(CubicBezier::EASE.ease(1.0), 1.0);
        // ease at 50 % is about 0.8024 (WebKit's UnitBezier).
        assert!((CubicBezier::EASE.ease(0.5) - 0.8024).abs() < 1e-3);
        assert!((CubicBezier::EASE_IN_OUT.ease(0.5) - 0.5).abs() < 1e-6);
        let pop = CubicBezier::new(0.16, 1.0, 0.3, 1.0);
        assert!(pop.ease(0.25) > 0.8);
    }

    #[test]
    fn reversing_takes_the_time_already_played() {
        let t0 = Instant::now();
        let mut t = Transition::settled(0.0, CubicBezier::new(0.0, 0.0, 1.0, 1.0), t0);
        t.retarget(1.0, 0.2, t0);
        let mid = t0 + Duration::from_millis(50);
        assert!((t.value(mid) - 0.25).abs() < 1e-3);
        t.retarget(0.0, 0.2, mid);
        assert!((t.value(mid) - 0.25).abs() < 1e-3);
        assert!(t.running(mid + Duration::from_millis(40)));
        assert!(!t.running(mid + Duration::from_millis(60)));
        assert_eq!(t.value(mid + Duration::from_millis(60)), 0.0);
    }
}
