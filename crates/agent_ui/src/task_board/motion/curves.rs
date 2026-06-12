//! The §4.9 timing curves — exact CSS `cubic-bezier()` evaluation via a
//! small bisection solver (COMPLEMENTARY_SOLUTIONS "overshoot mapping": the
//! spec allows lyon_geom's `CubicBezierSegment` or a hand solver; the hand
//! solver avoids a new direct dep for ~30 lines, and CSS timing curves are
//! x-monotonic so bisection is exact to f32 precision), plus the `linear()`
//! exit table and [`MotionCurve`] unifying both families.

/// A CSS `cubic-bezier(x1, y1, x2, y2)` timing function. Control-point xs
/// are clamped to [0,1] by CSS, making x(t) monotonic — y-for-x is solvable
/// by bisection.
#[derive(Debug, Clone, Copy)]
pub struct CubicBezier {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
}

/// `--spatial: cubic-bezier(0.38, 1.21, 0.22, 1.0)` 500ms — geometry morphs,
/// gentle overshoot (peaks ≈ 1.014). Map inside animator closures only.
pub const SPATIAL: CubicBezier = CubicBezier::new(0.38, 1.21, 0.22, 1.0);
/// `--decel-curve: cubic-bezier(0.05, 0.7, 0.1, 1.0)` — hard-landing
/// entrances (edge draw-in, row rise). Stays in [0,1].
pub const DECEL: CubicBezier = CubicBezier::new(0.05, 0.7, 0.1, 1.0);
/// `--effects-curve: cubic-bezier(0.34, 0.8, 0.34, 1.0)` 200ms — opacity /
/// color crossfades. Stays in [0,1].
pub const EFFECTS: CubicBezier = CubicBezier::new(0.34, 0.8, 0.34, 1.0);

/// `--exit-curve` — Caelestia's `emphasized` two-segment spline, carried as
/// the same `linear()` sample table the web ships (app.css): fast middle,
/// no overshoot. Exits are sharper than entries and never symmetric with
/// them (§4.9). Evaluated by piecewise-linear interpolation, exactly like
/// CSS `linear()`.
pub const EXIT_POINTS: [f32; 21] = [
    0.0, 0.021, 0.094, 0.274, 0.635, 0.773, 0.84, 0.883, 0.912, 0.934, 0.95, 0.963, 0.973, 0.981,
    0.987, 0.991, 0.995, 0.997, 0.999, 1.0, 1.0,
];

/// y for progress-x on the exit curve (CSS `linear()` semantics: stops
/// spread evenly across [0,1], segments interpolate linearly).
pub fn exit_eval(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let span = (EXIT_POINTS.len() - 1) as f32;
    let pos = x * span;
    let ix = pos.floor() as usize;
    let frac = pos - ix as f32;
    EXIT_POINTS[ix] + (EXIT_POINTS[ix + 1] - EXIT_POINTS[ix]) * frac
}

/// A motion timing curve from either family — CSS `cubic-bezier()` or the
/// `linear()` exit table. [`AnimatedValue`](super::AnimatedValue) /
/// [`AnimatedColor`](super::AnimatedColor) carry this so a retarget can
/// move an in-flight value ONTO the exit spline (the toast emerge → retract
/// interrupt, §4.9 asymmetric exits).
#[derive(Debug, Clone, Copy)]
pub enum MotionCurve {
    Bezier(CubicBezier),
    Exit,
}

impl MotionCurve {
    /// y for progress-x ∈ [0,1] (clamps outside).
    pub fn eval(self, x: f32) -> f32 {
        match self {
            MotionCurve::Bezier(curve) => curve.eval(x),
            MotionCurve::Exit => exit_eval(x),
        }
    }
}

impl From<CubicBezier> for MotionCurve {
    fn from(curve: CubicBezier) -> Self {
        MotionCurve::Bezier(curve)
    }
}

impl CubicBezier {
    pub const fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self { x1, y1, x2, y2 }
    }

    /// One-axis cubic Bézier with endpoints 0 and 1.
    fn axis(a: f32, b: f32, t: f32) -> f32 {
        let omt = 1.0 - t;
        3.0 * omt * omt * t * a + 3.0 * omt * t * t * b + t * t * t
    }

    /// y for progress-x (CSS semantics). `x` outside [0,1] clamps.
    pub fn eval(&self, x: f32) -> f32 {
        if x <= 0.0 {
            return 0.0;
        }
        if x >= 1.0 {
            return 1.0;
        }
        let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
        // 32 halvings beat f32 resolution on [0,1].
        for _ in 0..32 {
            let mid = (lo + hi) / 2.0;
            if Self::axis(self.x1, self.x2, mid) < x {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Self::axis(self.y1, self.y2, (lo + hi) / 2.0)
    }

    /// Adapter for `Animation::with_easing`. Only valid for curves whose y
    /// stays in [0,1] (see module GUARD) — debug-asserted here too.
    pub fn easing(self) -> impl Fn(f32) -> f32 {
        debug_assert!(
            self.y1 >= 0.0 && self.y1 <= 1.0 && self.y2 >= 0.0 && self.y2 <= 1.0,
            "overshooting curve used as a direct easing — map it inside the animator instead",
        );
        move |delta| self.eval(delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference values computed with an independent 60-iteration solver
    /// (python, 2026-06-12) at x ∈ {0.1, 0.25, 0.5, 0.75, 0.9}.
    fn assert_curve(curve: CubicBezier, expected: [f32; 5]) {
        for (x, want) in [0.1, 0.25, 0.5, 0.75, 0.9].into_iter().zip(expected) {
            let got = curve.eval(x);
            assert!((got - want).abs() < 1e-4, "eval({x}) = {got}, want {want}");
        }
    }

    #[test]
    fn endpoints_are_exact() {
        for curve in [SPATIAL, DECEL, EFFECTS] {
            assert_eq!(curve.eval(0.0), 0.0);
            assert_eq!(curve.eval(1.0), 1.0);
            assert_eq!(curve.eval(-0.5), 0.0);
            assert_eq!(curve.eval(1.5), 1.0);
        }
    }

    #[test]
    fn spatial_matches_css_reference_and_overshoots() {
        assert_curve(SPATIAL, [0.324396, 0.785264, 1.011864, 1.006597, 1.001133]);
        // The overshoot peak (~1.014) is the visible spring settle (§8.7
        // fluidity gate criterion c).
        let peak = (0..=200)
            .map(|i| SPATIAL.eval(i as f32 / 200.0))
            .fold(0.0_f32, f32::max);
        assert!((peak - 1.013942).abs() < 1e-3, "peak = {peak}");
    }

    #[test]
    fn decel_matches_css_reference_within_unit_range() {
        assert_curve(DECEL, [0.621384, 0.83153, 0.950247, 0.990511, 0.998666]);
        for i in 0..=200 {
            let y = DECEL.eval(i as f32 / 200.0);
            assert!((0.0..=1.0).contains(&y));
        }
    }

    #[test]
    fn effects_matches_css_reference() {
        assert_curve(EFFECTS, [0.239867, 0.58713, 0.905337, 0.985964, 0.998237]);
    }

    #[test]
    fn exit_eval_endpoints_and_clamps_are_exact() {
        assert_eq!(exit_eval(0.0), 0.0);
        assert_eq!(exit_eval(1.0), 1.0);
        assert_eq!(exit_eval(-0.5), 0.0);
        assert_eq!(exit_eval(1.5), 1.0);
    }

    #[test]
    fn exit_eval_matches_css_linear_semantics() {
        // CSS linear(): N stops spread evenly across [0,1] — the i-th stop
        // sits exactly at x = i/(N-1) and segments interpolate linearly, so
        // segment midpoints are exact neighbor averages (app.css:17-18).
        let span = (EXIT_POINTS.len() - 1) as f32;
        for (i, want) in EXIT_POINTS.iter().enumerate() {
            let got = exit_eval(i as f32 / span);
            assert!((got - want).abs() < 1e-5, "stop {i}: {got} vs {want}");
        }
        for i in 0..EXIT_POINTS.len() - 1 {
            let x = (i as f32 + 0.5) / span;
            let want = (EXIT_POINTS[i] + EXIT_POINTS[i + 1]) / 2.0;
            let got = exit_eval(x);
            assert!((got - want).abs() < 1e-5, "midpoint {i}: {got} vs {want}");
        }
    }

    #[test]
    fn exit_eval_is_monotonic_nondecreasing_in_unit_range() {
        let mut prev = 0.0_f32;
        for i in 0..=400 {
            let y = exit_eval(i as f32 / 400.0);
            assert!(y >= prev, "exit curve dipped at x={}", i as f32 / 400.0);
            assert!((0.0..=1.0).contains(&y), "exit curve left [0,1]");
            prev = y;
        }
    }

    #[test]
    fn motion_curve_dispatches_both_families() {
        let bezier: MotionCurve = EFFECTS.into();
        assert!((bezier.eval(0.5) - EFFECTS.eval(0.5)).abs() < 1e-6);
        assert!((MotionCurve::Exit.eval(0.5) - exit_eval(0.5)).abs() < 1e-6);
    }
}
