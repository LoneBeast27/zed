//! Caelestia motion tokens (PARITY_SPEC §4.9 motion law) — exact CSS
//! `cubic-bezier()` evaluation via a small bisection solver
//! (COMPLEMENTARY_SOLUTIONS "overshoot mapping": the spec allows lyon_geom's
//! `CubicBezierSegment` or a hand solver; the hand solver avoids a new direct
//! dep for ~30 lines, and CSS timing curves are x-monotonic so bisection is
//! exact to f32 precision).
//!
//! GUARD: gpui's `animation.rs:165` debug-asserts easing output ∈ [0,1], so
//! the overshooting [`SPATIAL`] curve must be evaluated INSIDE the animator
//! closure (pass linear delta to `Animation`, call `SPATIAL.eval(delta)` in
//! the animator). [`DECEL`] and [`EFFECTS`] stay within [0,1] and may be used
//! directly via `Animation::with_easing(DECEL.easing())`.

use std::time::{Duration, Instant};

use gpui::{Hsla, Rgba};

/// Interactive-state crossfade duration — the web's
/// `transition: … .15s var(--effects-curve)` on seg-toggle buttons, inbox
/// rows, drawer tabs, and the drawer close button (board.css:23,38,183,198;
/// PARITY_SPEC §0 "state changes 150–250ms").
pub const STATE_FADE: Duration = Duration::from_millis(150);
/// How long after a state flip the animated wrapper stays attached: the fade
/// plus margin. Outside this window elements render bare, so settled states
/// cost nothing per frame and an element-state drop (scroll culling, panel
/// re-show) can never replay a finished fade.
pub const STATE_FADE_WINDOW: Duration = Duration::from_millis(300);

/// Tracks one piece of flip-state for a one-shot effects-curve crossfade:
/// [`bump`](Self::bump) on every state change; [`fresh`](Self::fresh) says
/// whether the crossfade window is still open (attach the animated wrapper
/// only then); [`generation`](Self::generation) keys the animation identity
/// so each flip animates exactly once.
#[derive(Debug, Clone, Default)]
pub struct StateFade {
    generation: usize,
    changed_at: Option<Instant>,
}

impl StateFade {
    /// A tracker whose state changed right now (already mid-fade).
    pub fn begun() -> Self {
        let mut fade = Self::default();
        fade.bump();
        fade
    }

    /// Record a state flip: opens the crossfade window, new generation.
    pub fn bump(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.changed_at = Some(Instant::now());
    }

    /// Whether the crossfade window from the last flip is still open.
    pub fn fresh(&self) -> bool {
        self.changed_at
            .is_some_and(|at| at.elapsed() < STATE_FADE_WINDOW)
    }

    /// Animation-identity key for the latest flip.
    pub fn generation(&self) -> usize {
        self.generation
    }
}

/// Componentwise sRGB mix between two colors — the interpolation a CSS color
/// `transition` performs, used by the 150ms state crossfades.
pub fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (Rgba::from(a), Rgba::from(b));
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
    .into()
}

/// A retargetable eased scalar — the positional half of the §4.9 morph
/// primitive (COMPLEMENTARY_SOLUTIONS B gap 2). gpui's `with_animation`
/// restarts at t=0 with no memory, so retargeting mid-flight would snap;
/// this tracker re-bases `from` to the *interpolated current value* at each
/// retarget, making interrupts continuous (positional retarget only — no
/// velocity carry; a true spring layer stays a Z4 item). Render idiom:
/// attach `with_animation` keyed by [`generation`](Self::generation) while
/// [`animating`](Self::animating), apply [`value_at`](Self::value_at) in
/// the animator; settled values render bare at [`target`](Self::target).
#[derive(Debug, Clone)]
pub struct AnimatedValue {
    from: f32,
    to: f32,
    started: Option<Instant>,
    generation: usize,
    curve: CubicBezier,
    duration: Duration,
}

impl AnimatedValue {
    /// A settled value (no entrance animation).
    pub fn settled(value: f32, curve: CubicBezier, duration: Duration) -> Self {
        Self {
            from: value,
            to: value,
            started: None,
            generation: 0,
            curve,
            duration,
        }
    }

    /// Animate toward `to` from wherever the value is *right now* (mid-
    /// flight retargets are continuous). Same-target retargets are no-ops,
    /// so render-loop calls never restart a settled animation.
    pub fn retarget(&mut self, to: f32) {
        if to == self.to {
            return;
        }
        self.from = self.current();
        self.to = to;
        self.started = Some(Instant::now());
        self.generation = self.generation.wrapping_add(1);
    }

    /// Jump without animating (initial paints, offscreen updates).
    pub fn jump(&mut self, value: f32) {
        self.from = value;
        self.to = value;
        self.started = None;
    }

    /// The interpolated value at this instant.
    pub fn current(&self) -> f32 {
        match self.started {
            None => self.to,
            Some(started) => {
                let t = started.elapsed().as_secs_f32() / self.duration.as_secs_f32();
                self.value_at(t)
            }
        }
    }

    /// The value at animation progress `t` ∈ [0,1] — for `with_animation`
    /// animator closures (the eased lerp; overshooting curves overshoot
    /// the segment, which is the visible spring settle).
    pub fn value_at(&self, t: f32) -> f32 {
        self.from + (self.to - self.from) * self.curve.eval(t)
    }

    pub fn target(&self) -> f32 {
        self.to
    }

    /// Whether the last retarget's animation window is still open.
    pub fn animating(&self) -> bool {
        self.started
            .is_some_and(|started| started.elapsed() < self.duration)
    }

    /// Animation-identity key for the latest retarget.
    pub fn generation(&self) -> usize {
        self.generation
    }

    pub fn duration(&self) -> Duration {
        self.duration
    }
}

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
            assert!(
                (got - want).abs() < 1e-4,
                "eval({x}) = {got}, want {want}"
            );
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
    fn state_fade_opens_window_and_advances_generation() {
        let mut fade = StateFade::default();
        assert!(!fade.fresh(), "untouched state is settled");
        assert_eq!(fade.generation(), 0);
        fade.bump();
        assert!(fade.fresh(), "a flip opens the crossfade window");
        assert_eq!(fade.generation(), 1);
        fade.bump();
        assert_eq!(fade.generation(), 2, "each flip is a new animation key");
        assert!(StateFade::begun().fresh());
    }

    #[test]
    fn animated_value_retargets_from_interpolated_current() {
        let mut value = AnimatedValue::settled(10.0, EFFECTS, Duration::from_millis(200));
        assert!(!value.animating());
        assert_eq!(value.current(), 10.0);

        value.retarget(20.0);
        assert!(value.animating());
        assert_eq!(value.generation(), 1);
        // t=0 starts at the old value; t=1 lands on the target; midway is
        // strictly between (no snap — §8.7a).
        assert_eq!(value.value_at(0.0), 10.0);
        assert_eq!(value.value_at(1.0), 20.0);
        let mid = value.value_at(0.5);
        assert!(mid > 10.0 && mid < 20.0, "mid = {mid}");

        // Same-target retarget is a no-op (render loops call this per frame).
        value.retarget(20.0);
        assert_eq!(value.generation(), 1);

        // A mid-flight retarget re-bases from the interpolated current value.
        let current = value.current();
        value.retarget(0.0);
        assert_eq!(value.generation(), 2);
        assert!(
            (value.value_at(0.0) - current).abs() < 0.5,
            "retarget must start from the in-flight value ({current}), got {}",
            value.value_at(0.0)
        );

        value.jump(5.0);
        assert!(!value.animating());
        assert_eq!(value.current(), 5.0);
    }

    #[test]
    fn mix_interpolates_srgb_endpoints_exactly() {
        let a = gpui::Rgba {
            r: 1.0,
            g: 0.0,
            b: 0.5,
            a: 1.0,
        };
        let b = gpui::Rgba {
            r: 0.0,
            g: 1.0,
            b: 0.5,
            a: 0.0,
        };
        // Hsla round-trips cost a little float precision — compare with tolerance.
        let assert_rgba = |got: gpui::Rgba, want: gpui::Rgba| {
            assert!((got.r - want.r).abs() < 1e-4, "r: {got:?} vs {want:?}");
            assert!((got.g - want.g).abs() < 1e-4, "g: {got:?} vs {want:?}");
            assert!((got.b - want.b).abs() < 1e-4, "b: {got:?} vs {want:?}");
            assert!((got.a - want.a).abs() < 1e-4, "a: {got:?} vs {want:?}");
        };
        assert_rgba(gpui::Rgba::from(mix(a.into(), b.into(), 0.0)), a);
        assert_rgba(gpui::Rgba::from(mix(a.into(), b.into(), 1.0)), b);
        assert_rgba(
            gpui::Rgba::from(mix(a.into(), b.into(), 0.5)),
            gpui::Rgba {
                r: 0.5,
                g: 0.5,
                b: 0.5,
                a: 0.5,
            },
        );
    }
}
